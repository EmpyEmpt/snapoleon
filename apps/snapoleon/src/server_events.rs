//! Views and comments on shared links: a slow background poll of the team server, shown as notices.

use std::time::Duration;

use snap_core::{t, tf};
use snap_engine::{Engine, Notice, NoticeKind, ServerEvent};
use snap_shell::Proxy;
use snap_upload::server::RemoteEventKind;

use crate::events::AppEvent;

/// Between two asks of the server.
const EVERY: Duration = Duration::from_secs(180);
/// After errors the wait doubles up to this.
const LONGEST: Duration = Duration::from_secs(30 * 60);

/// Ask the team servers for new views and comments for as long as the app lives, while the setting is on.
/// Starts counting from now: what happened while the app was closed stays with the server's own notifications.
pub fn start(engine: Engine, proxy: Proxy<AppEvent>) {
    let spawned = std::thread::Builder::new().name("snap-server-events".into()).spawn(move || {
        let mut since = snap_core::now_ms();
        let mut wait = EVERY;
        loop {
            std::thread::sleep(wait);
            if !engine.settings().links.notify_events || !engine.has_team_server() {
                wait = EVERY;
                continue;
            }
            match engine.server_events(since) {
                Ok(events) => {
                    wait = EVERY;
                    for event in events {
                        since = since.max(event.event.created_ms);
                        proxy.send(AppEvent::Notice(notice(&event)));
                    }
                }
                Err(e) => {
                    tracing::debug!("server events: {e}");
                    wait = (wait * 2).min(LONGEST);
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the server events poll: {e}");
    }
}

fn notice(e: &ServerEvent) -> Notice {
    let event = &e.event;
    let who = if event.actor.is_empty() { t("app.events.someone").into_owned() } else { event.actor.clone() };
    let title = if event.title.is_empty() { event.item_id.clone() } else { event.title.clone() };
    let (headline, body) = match event.kind {
        RemoteEventKind::Comment => (tf("app.events.commented", &[("who", &who), ("title", &title)]), event.text.clone()),
        _ => (tf("app.events.viewed", &[("who", &who), ("title", &title)]), String::new()),
    };
    Notice { kind: NoticeKind::Info, title: headline, body, url: Some(event.url.clone()).filter(|u| !u.is_empty()), ..Notice::default() }
}
