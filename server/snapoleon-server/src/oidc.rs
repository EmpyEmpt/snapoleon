//! OpenID Connect single sign-on: authorization code flow with PKCE.
//!
//! Hand-rolled on `reqwest` + `jsonwebtoken`: discovery, JWKS caching, ID-token
//! verification (signature, issuer, audience, expiry, nonce) and user provisioning.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use snap_core::now_ms;

use crate::{
    auth::{SESSION_COOKIE, SESSION_TTL_MS, cookie_value, set_cookie},
    db::{NewUser, Role},
    error::{AppError, PageResult},
    state::{AppState, Shared},
    util::{ct_eq, new_secret},
};

const FLOW_COOKIE: &str = "snap_oidc";
const FLOW_TTL_SECS: i64 = 600;
const CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}

#[derive(Default)]
pub struct Oidc {
    discovery: Mutex<Option<(Instant, Arc<Discovery>)>>,
    jwks: Mutex<Option<(Instant, Arc<JwkSet>)>>,
}

/// Claims we use from a verified ID token.
#[derive(Debug)]
pub struct Identity {
    pub sub: String,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub groups: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Flow {
    state: String,
    nonce: String,
    verifier: String,
    exp_ms: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

impl Oidc {
    async fn discovery(&self, shared: &Shared) -> Result<Arc<Discovery>> {
        if let Some((at, d)) = self.discovery.lock().clone()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(d);
        }
        let url = format!(
            "{}/.well-known/openid-configuration",
            shared.cfg.oidc.issuer
        );
        let d: Discovery = shared
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("reading OIDC discovery document")?;
        if d.issuer.trim_end_matches('/') != shared.cfg.oidc.issuer {
            bail!("issuer mismatch: discovery says {}", d.issuer);
        }
        let d = Arc::new(d);
        *self.discovery.lock() = Some((Instant::now(), d.clone()));
        Ok(d)
    }

    async fn jwks(&self, shared: &Shared, d: &Discovery, force: bool) -> Result<Arc<JwkSet>> {
        if !force
            && let Some((at, k)) = self.jwks.lock().clone()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(k);
        }
        let set: JwkSet = shared
            .http
            .get(&d.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("reading JWKS")?;
        let set = Arc::new(set);
        *self.jwks.lock() = Some((Instant::now(), set.clone()));
        Ok(set)
    }

    /// Build the provider redirect URL and the signed flow cookie value.
    async fn begin(&self, shared: &Shared) -> Result<(String, String)> {
        let d = self.discovery(shared).await?;
        let flow = Flow {
            state: new_secret(32),
            nonce: new_secret(32),
            verifier: new_secret(64),
            exp_ms: now_ms() + FLOW_TTL_SECS * 1000,
        };
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(flow.verifier.as_bytes()));
        let mut url = reqwest::Url::parse(&d.authorization_endpoint)?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &shared.cfg.oidc.client_id)
            .append_pair("redirect_uri", &shared.url("/auth/oidc/callback"))
            .append_pair("scope", &shared.cfg.oidc.scopes)
            .append_pair("state", &flow.state)
            .append_pair("nonce", &flow.nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&flow)?);
        let cookie = format!("{payload}.{}", shared.sign("oidc-flow", &payload));
        Ok((url.into(), cookie))
    }

    /// Exchange the code, verify the ID token and return the identity.
    async fn finish(
        &self,
        shared: &Shared,
        code: &str,
        state_param: &str,
        cookie: &str,
    ) -> Result<Identity> {
        let (payload, sig) = cookie
            .split_once('.')
            .ok_or_else(|| anyhow!("malformed flow cookie"))?;
        if !ct_eq(sig, &shared.sign("oidc-flow", payload)) {
            bail!("flow cookie signature mismatch");
        }
        let flow: Flow = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?;
        if flow.exp_ms < now_ms() || !ct_eq(&flow.state, state_param) {
            bail!("state mismatch or expired login attempt");
        }

        let d = self.discovery(shared).await?;
        let oidc = &shared.cfg.oidc;
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", shared.url("/auth/oidc/callback")),
            ("code_verifier", flow.verifier.clone()),
        ];
        let basic = !d.token_endpoint_auth_methods_supported.is_empty()
            && !d
                .token_endpoint_auth_methods_supported
                .iter()
                .any(|m| m == "client_secret_post")
            && d.token_endpoint_auth_methods_supported
                .iter()
                .any(|m| m == "client_secret_basic");
        let mut req = shared.http.post(&d.token_endpoint);
        if basic {
            req = req.basic_auth(&oidc.client_id, Some(&oidc.client_secret));
        } else {
            form.push(("client_id", oidc.client_id.clone()));
            if !oidc.client_secret.is_empty() {
                form.push(("client_secret", oidc.client_secret.clone()));
            }
        }
        let resp = req.form(&form).send().await?;
        if !resp.status().is_success() {
            bail!("token endpoint returned HTTP {}", resp.status().as_u16());
        }
        let tokens: TokenResponse = resp.json().await.context("reading token response")?;
        let claims = self.verify_id_token(shared, &d, &tokens.id_token).await?;
        if claims
            .get("nonce")
            .and_then(Value::as_str)
            .is_none_or(|n| !ct_eq(n, &flow.nonce))
        {
            bail!("nonce mismatch");
        }
        Ok(identity_from_claims(&claims, &oidc.groups_claim))
    }

    async fn verify_id_token(&self, shared: &Shared, d: &Discovery, token: &str) -> Result<Value> {
        let header = decode_header(token)?;
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            bail!("symmetric ID token algorithms are not accepted");
        }
        let mut set = self.jwks(shared, d, false).await?;
        let mut jwk = header.kid.as_deref().and_then(|kid| set.find(kid)).cloned();
        if jwk.is_none() {
            // Keys may have rotated since the cache was filled.
            set = self.jwks(shared, d, true).await?;
            jwk = match header.kid.as_deref() {
                Some(kid) => set.find(kid).cloned(),
                None if set.keys.len() == 1 => set.keys.first().cloned(),
                None => None,
            };
        }
        let key = DecodingKey::from_jwk(&jwk.ok_or_else(|| anyhow!("no matching signing key"))?)?;
        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[&shared.cfg.oidc.client_id]);
        validation.set_issuer(&[&d.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        Ok(decode::<Value>(token, &key, &validation)?.claims)
    }
}

fn identity_from_claims(claims: &Value, groups_claim: &str) -> Identity {
    let text = |k: &str| {
        claims
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let email = text("email");
    let mut username = text("preferred_username");
    if username.is_empty() {
        username = email.split('@').next().unwrap_or_default().to_string();
    }
    let username: String = username
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(40)
        .collect();
    let groups = match claims.get(groups_claim) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => s
            .split([',', ' '])
            .filter(|g| !g.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let sub = text("sub");
    Identity {
        username: if username.is_empty() {
            format!("user-{}", sub.chars().take(8).collect::<String>())
        } else {
            username
        },
        display_name: text("name"),
        email,
        groups,
        sub,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/oidc/login", get(login))
        .route("/auth/oidc/callback", get(callback))
}

async fn login(State(state): State<AppState>) -> PageResult<Response> {
    if !state.cfg.oidc.enabled() {
        return Err(AppError::NotFound.into());
    }
    let (url, cookie) = state.oidc.begin(&state).await.map_err(AppError::Internal)?;
    let mut resp = Redirect::to(&url).into_response();
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(
            FLOW_COOKIE,
            &cookie,
            FLOW_TTL_SECS,
            "/auth/oidc",
            state.cfg.secure_cookies(),
        ),
    );
    Ok(resp)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> PageResult<Response> {
    if !state.cfg.oidc.enabled() {
        return Err(AppError::NotFound.into());
    }
    if let Some(err) = q.error {
        tracing::warn!("OIDC provider returned an error: {err}");
        return Err(AppError::Forbidden.into());
    }
    let (Some(code), Some(state_param), Some(cookie)) =
        (q.code, q.state, cookie_value(&headers, FLOW_COOKIE))
    else {
        return Err(
            AppError::bad_request(snap_core::t("server.error.sso_failed").into_owned()).into(),
        );
    };
    let identity = state
        .oidc
        .finish(&state, &code, &state_param, &cookie)
        .await
        .map_err(|e| {
            tracing::warn!("OIDC login failed: {e:#}");
            AppError::bad_request(snap_core::t("server.error.sso_failed").into_owned())
        })?;

    let user = provision(&state, &identity).await?;
    let session = state
        .db
        .create_session(user.id.clone(), SESSION_TTL_MS)
        .await?;
    let mut resp = Redirect::to("/admin").into_response();
    let secure = state.cfg.secure_cookies();
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(SESSION_COOKIE, &session, SESSION_TTL_MS / 1000, "/", secure),
    );
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(FLOW_COOKIE, "", 0, "/auth/oidc", secure),
    );
    Ok(resp)
}

/// Find or create the local account for an SSO identity and apply group-based admin rights.
async fn provision(state: &AppState, id: &Identity) -> Result<crate::db::User, AppError> {
    let oidc = &state.cfg.oidc;
    let admin_by_group =
        !oidc.admin_group.is_empty() && id.groups.iter().any(|g| g == &oidc.admin_group);
    if let Some(user) = state.db.user_by_oidc_sub(id.sub.clone()).await? {
        if user.disabled {
            return Err(AppError::Forbidden);
        }
        let admin = if oidc.admin_group.is_empty() {
            user.is_admin
        } else {
            admin_by_group
        };
        let name = if id.display_name.is_empty() {
            user.display_name.clone()
        } else {
            id.display_name.clone()
        };
        let email = if id.email.is_empty() {
            user.email.clone()
        } else {
            id.email.clone()
        };
        state
            .db
            .sync_oidc_profile(user.id.clone(), name, email, admin)
            .await?;
        return state
            .db
            .user_by_id(user.id)
            .await?
            .ok_or(AppError::NotFound);
    }
    if !oidc.auto_provision {
        return Err(AppError::Forbidden);
    }
    let workspace = state.db.default_workspace().await?;
    for n in 0..20 {
        let username = if n == 0 {
            id.username.clone()
        } else {
            format!("{}-{}", id.username, n + 1)
        };
        let created = state
            .db
            .create_user(NewUser {
                username,
                display_name: id.display_name.clone(),
                email: id.email.clone(),
                password_hash: None,
                oidc_sub: Some(id.sub.clone()),
                is_admin: admin_by_group,
            })
            .await?;
        if let Some(user) = created {
            state
                .db
                .set_member(
                    workspace.id.clone(),
                    user.id.clone(),
                    if admin_by_group {
                        Role::Admin
                    } else {
                        Role::Member
                    },
                )
                .await?;
            return Ok(user);
        }
    }
    Err(AppError::Internal(anyhow!(
        "could not allocate a username for {}",
        id.sub
    )))
}
