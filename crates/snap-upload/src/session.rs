//! Bearer-token session shared by the OAuth-backed cloud destinations.

use parking_lot::Mutex;
use snap_core::{Error, Result, secrets::SecretStore, tf};

use crate::{
    http::Response,
    oauth::{self, Provider, Tokens},
};

pub(crate) struct Session {
    state: Mutex<State>,
}

enum State {
    /// A plain access token with no way to refresh it.
    Bare(String),
    Oauth(Tokens),
}

impl Session {
    /// Resolve the destination's secret: OAuth tokens (refreshed and written
    /// back when expired) or a bare access token.
    pub fn load(provider: Provider, secret_ref: Option<&str>, secrets: &dyn SecretStore, client_id: &str) -> Result<Self> {
        let missing = || Error::permission(tf("upload.error.missing_secret", &[("name", provider.name())]));
        let key = secret_ref.filter(|k| !k.is_empty()).ok_or_else(missing)?;
        let state = match oauth::load_fresh(key, secrets)? {
            Some(mut tokens) => {
                if tokens.client_id.is_empty() {
                    tokens.client_id = client_id.to_string();
                }
                State::Oauth(tokens)
            }
            None => State::Bare(secrets.get(key).filter(|s| !s.trim().is_empty()).ok_or_else(missing)?.trim().to_string()),
        };
        Ok(Self { state: Mutex::new(state) })
    }

    pub fn token(&self) -> String {
        match &*self.state.lock() {
            State::Bare(t) => t.clone(),
            State::Oauth(t) => t.access_token.clone(),
        }
    }

    /// Run a request; on `401` refresh the token (in memory) and retry once.
    pub fn send(&self, f: impl Fn(&str) -> Result<Response>) -> Result<Response> {
        let resp = f(&self.token())?;
        if resp.status == 401 && self.refresh()? {
            return f(&self.token());
        }
        Ok(resp)
    }

    fn refresh(&self) -> Result<bool> {
        let current = match &*self.state.lock() {
            State::Oauth(t) if t.refresh_token.is_some() => t.clone(),
            _ => return Ok(false),
        };
        tracing::debug!(provider = current.provider.name(), "access token rejected, refreshing");
        let fresh = current.refreshed()?;
        *self.state.lock() = State::Oauth(fresh);
        Ok(true)
    }
}
