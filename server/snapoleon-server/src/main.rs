//! Snapoleon team server: uploads, short links, viewer pages, workspaces.

mod admin;
mod api;
mod app;
mod auth;
mod config;
mod db;
mod error;
mod i18n;
mod og;
mod oidc;
mod state;
mod storage;
mod sweeper;
mod util;
mod viewer;
mod views;
mod webhooks;

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use crate::i18n::{t, tf};
use anyhow::{Context, Result, bail};
use tracing_subscriber::EnvFilter;

use crate::{
    config::Config,
    db::{Db, NewUser, Role},
    state::{RateLimiter, Shared},
    storage::Blobs,
};

enum Command {
    Serve,
    Token { username: String },
    Healthcheck,
    Help,
    Version,
}

struct Args {
    command: Command,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut config = None;
    let mut command = Command::Serve;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-c" | "--config" => config = Some(PathBuf::from(it.next().context("--config needs a path")?)),
            "-h" | "--help" => command = Command::Help,
            "-V" | "--version" => command = Command::Version,
            "serve" => command = Command::Serve,
            "healthcheck" => command = Command::Healthcheck,
            "token" => command = Command::Token { username: it.next().context("token needs a username")? },
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { command, config })
}

const USAGE: &str = "snapoleon-server [--config FILE] [serve]
snapoleon-server [--config FILE] token <username>   print a new API token for a user
snapoleon-server [--config FILE] healthcheck        exit 0 when the local server answers /healthz
snapoleon-server --version

Configuration comes from snapoleon-server.toml and SNAPOLEON_* environment variables.";

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    match args.command {
        Command::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Command::Version => {
            println!("snapoleon-server {}", snap_core::VERSION);
            return Ok(());
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    let cfg = Config::load(args.config.as_deref())?;
    if matches!(args.command, Command::Healthcheck) {
        return healthcheck(&cfg).await;
    }
    snap_core::i18n::set_language(&cfg.language);
    std::fs::create_dir_all(&cfg.data_dir).with_context(|| format!("creating {}", cfg.data_dir.display()))?;
    let db = Db::open(&cfg.data_dir.join("snapoleon.sqlite"))?;

    if let Command::Token { username } = args.command {
        let user = db
            .user_by_username(username.clone())
            .await
            .map_err(|e| anyhow::anyhow!(e.message()))?
            .with_context(|| format!("no such user: {username}"))?;
        let token = db.create_token(user.id, "cli".into()).await.map_err(|e| anyhow::anyhow!(e.message()))?;
        println!("{token}");
        return Ok(());
    }

    let secret = installation_secret(&db).await?;
    let blobs = Blobs::open(&cfg)?;
    let http = reqwest::Client::builder()
        .user_agent(format!("snapoleon-server/{}", snap_core::VERSION))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;
    let state: state::AppState =
        Arc::new(Shared { cfg, db, blobs, secret, limiter: RateLimiter::default(), http, oidc: Default::default() });

    bootstrap(&state).await?;
    sweeper::spawn(state.clone());

    let addr: SocketAddr = state.cfg.bind.parse().with_context(|| format!("invalid bind address {}", state.cfg.bind))?;
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("{}", tf("server.log.listening", &[("addr", &addr.to_string()), ("url", &state.cfg.public_url)]));
    axum::serve(listener, app::router(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Probe the local `/healthz`; used as a container health check (distroless images have no curl).
async fn healthcheck(cfg: &Config) -> Result<()> {
    let addr: SocketAddr = cfg.bind.parse().with_context(|| format!("invalid bind address {}", cfg.bind))?;
    let host = if addr.ip().is_unspecified() { "127.0.0.1".to_string() } else { addr.ip().to_string() };
    let url = format!("http://{host}:{}/healthz", addr.port());
    let resp = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?.get(&url).send().await?;
    if resp.status().is_success() { Ok(()) } else { bail!("{url} answered HTTP {}", resp.status().as_u16()) }
}

/// Per-installation secret for signing cookies and hashing visitors; created on first run.
async fn installation_secret(db: &Db) -> Result<Vec<u8>> {
    if let Some(s) = db.meta_get("secret").await.map_err(|e| anyhow::anyhow!(e.message()))? {
        return Ok(s.into_bytes());
    }
    let secret = util::new_secret(64);
    db.meta_set("secret", secret.clone()).await.map_err(|e| anyhow::anyhow!(e.message()))?;
    Ok(secret.into_bytes())
}

/// First run: create the default workspace and an admin account, and print its credentials once.
async fn bootstrap(state: &state::AppState) -> Result<()> {
    let db = &state.db;
    let err = |e: error::AppError| anyhow::anyhow!(e.message());
    if db.list_workspaces().await.map_err(err)?.is_empty() {
        db.create_workspace(t("server.bootstrap.workspace").into_owned(), None).await.map_err(err)?;
    }
    if db.count_users().await.map_err(err)? > 0 {
        return Ok(());
    }
    let workspace = db.default_workspace().await.map_err(err)?;
    let password = util::new_secret(16);
    let hash = auth::hash_password(password.clone()).await.map_err(err)?;
    let admin = db
        .create_user(NewUser {
            username: "admin".into(),
            display_name: t("server.bootstrap.admin_name").into_owned(),
            email: String::new(),
            password_hash: Some(hash),
            oidc_sub: None,
            is_admin: true,
        })
        .await
        .map_err(err)?
        .context("creating the bootstrap admin")?;
    db.set_member(workspace.id, admin.id.clone(), Role::Owner).await.map_err(err)?;
    let token = db.create_token(admin.id, "bootstrap".into()).await.map_err(err)?;

    let rule = "=".repeat(64);
    println!("{rule}");
    println!("{}", t("server.bootstrap.title"));
    println!("{}", tf("server.bootstrap.admin_url", &[("url", &state.url("/admin"))]));
    println!("{}", tf("server.bootstrap.username", &[("username", "admin")]));
    println!("{}", tf("server.bootstrap.password", &[("password", &password)]));
    println!("{}", tf("server.bootstrap.token", &[("token", &token)]));
    println!("{}", t("server.bootstrap.once"));
    println!("{rule}");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("{}", t("server.log.shutdown"));
}
