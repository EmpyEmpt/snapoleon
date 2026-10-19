//! Links on a team server: look one up (views, password, expiry), delete it, share again with a password or expiry,
//! and poll for views and comments.

use std::path::Path;

use snap_core::{CaptureId, Error, Result, settings::DestinationKind, t, tf};
use snap_upload::server::{Client, RemoteEvent, RemoteItem};

use crate::{Engine, Link, share};

/// Pages of the server's item list searched for one link; 50 items each, newest first.
const MAX_PAGES: u32 = 20;

/// Who may open a link and for how long, chosen when sharing. The team server fixes both at upload time.
#[derive(Debug, Clone, Default)]
pub struct LinkAccess {
    /// Viewers must type this first. `None` or empty = open link.
    pub password: Option<String>,
    /// Hours until the link stops working; `Some(0)` = never, `None` = the Links setting.
    pub expiry_hours: Option<u32>,
}

/// A view or comment on one of your shared links.
#[derive(Debug, Clone)]
pub struct ServerEvent {
    /// Name of the destination (team server) it happened on.
    pub destination: String,
    pub event: RemoteEvent,
}

impl Engine {
    /// Whether any configured destination is a team server with a token, so views and comments can be asked for.
    pub fn has_team_server(&self) -> bool {
        self.settings().destinations.iter().any(|d| self.server_client(&d.id).is_ok())
    }

    /// The team-server item behind a link, with its view count, password flag and expiry. `None` when the server does
    /// not list it (deleted there, or a short link). Only the link's own `url` matches.
    pub fn remote_item(&self, destination: &str, url: &str) -> Result<Option<RemoteItem>> {
        let client = self.server_client(destination)?;
        for page in 1..=MAX_PAGES {
            let items = client.list(page)?;
            if items.is_empty() {
                break;
            }
            if let Some(found) = items.into_iter().find(|i| i.url == url) {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// Delete the item on its team server, so the link stops working.
    pub fn delete_remote(&self, destination: &str, item_id: &str) -> Result<()> {
        self.server_client(destination)?.delete(item_id)
    }

    /// [`upload_file`](Engine::upload_file) with a password and expiry for this one link. Only a team server honours them.
    pub fn upload_file_with(
        &self,
        path: &Path,
        dest: Option<&str>,
        id: Option<&CaptureId>,
        access: &LinkAccess,
        progress: &dyn Fn(u64, u64),
    ) -> Result<Link> {
        share::upload_file_with(self, path, dest, id, access, progress, true)
    }

    /// Views and comments on your links since `since_ms`, from every team server, oldest first.
    pub fn server_events(&self, since_ms: i64) -> Result<Vec<ServerEvent>> {
        let mut all = Vec::new();
        for d in self.settings().destinations {
            let Ok(client) = self.server_client(&d.id) else { continue };
            let events = client.events(since_ms)?;
            all.extend(events.into_iter().map(|event| ServerEvent { destination: d.name.clone(), event }));
        }
        all.sort_by_key(|e| e.event.created_ms);
        Ok(all)
    }

    fn server_client(&self, destination: &str) -> Result<Client> {
        let s = self.settings();
        let d =
            s.destination(destination).ok_or_else(|| Error::NotFound(tf("engine.error.unknown_destination", &[("id", destination)])))?;
        let DestinationKind::SnapServer { url } = &d.kind else {
            return Err(Error::invalid(t("engine.error.not_team_server").into_owned()));
        };
        let token = d.secret_ref.as_deref().filter(|k| !k.is_empty()).and_then(|k| self.secrets().get(k)).filter(|k| !k.trim().is_empty());
        let token = token.ok_or_else(|| Error::permission(tf("upload.error.missing_secret", &[("name", &d.name)])))?;
        Ok(Client::new(url, &token))
    }
}
