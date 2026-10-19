//! Wayland global shortcuts through `org.freedesktop.portal.GlobalShortcuts`.
//!
//! The portal owns the final key assignment: we only send a preferred trigger
//! and the compositor may show a dialog where the user changes it. Because that
//! dialog can stay open indefinitely, binding runs on a helper thread and
//! [`Portal::bind`] waits a bounded time for the answer.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use snap_core::{Error, Result, t};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Message, Type};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::accel::{Accel, xdg_trigger};
use super::{HotkeyEvent, Routes, wayland_unsupported};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// How long `bind` waits for the portal before reporting the shortcuts as pending.
const BIND_WAIT: Duration = Duration::from_secs(4);

pub(super) struct Portal {
    conn: Connection,
    session: OwnedObjectPath,
    listener: Option<JoinHandle<()>>,
    routes: Arc<Routes>,
    token: Arc<AtomicU32>,
}

impl Portal {
    pub fn new(on_event: impl Fn(HotkeyEvent) + Send + 'static, routes: Arc<Routes>) -> Result<Self> {
        let unsupported = |e: &dyn std::fmt::Display| wayland_unsupported(&format!("GlobalShortcuts portal unavailable ({e})."));
        let conn = Connection::session().map_err(|e| unsupported(&e))?;
        conn.call_method(Some(PORTAL), PORTAL_PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(SHORTCUTS, "version"))
            .map_err(|e| unsupported(&e))?;

        let token = Arc::new(AtomicU32::new(0));
        let session_token = format!("snapoleon_session_{}", std::process::id());
        let results = request(&conn, &next_token(&token), |handle_token| {
            let mut options: HashMap<&str, Value<'_>> = HashMap::new();
            options.insert("handle_token", Value::from(handle_token.to_string()));
            options.insert("session_handle_token", Value::from(session_token.clone()));
            conn.call_method(Some(PORTAL), PORTAL_PATH, Some(SHORTCUTS), "CreateSession", &(options,))
        })
        .map_err(|e| unsupported(&e))?;
        let session = results
            .get("session_handle")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok())
            .and_then(|s| OwnedObjectPath::try_from(s).ok())
            .ok_or_else(|| unsupported(&"no session handle"))?;

        let listener = spawn_listener(conn.clone(), session.clone(), routes.clone(), on_event)?;
        Ok(Self { conn, session, listener: Some(listener), routes, token })
    }

    /// Ask the portal to bind the shortcuts. Shortcut ids are `b<binding index>`.
    pub fn bind(&mut self, accels: &[(usize, Accel)]) -> std::result::Result<(), String> {
        {
            let mut ids = self.routes.ids.lock();
            ids.clear();
            ids.extend(accels.iter().map(|(i, _)| (*i as u32, *i)));
        }
        if accels.is_empty() {
            return Ok(());
        }
        let shortcuts: Vec<(String, HashMap<&str, Value<'_>>)> = accels
            .iter()
            .map(|(i, a)| {
                let mut props: HashMap<&str, Value<'_>> = HashMap::new();
                props.insert("description", Value::from(format!("Snapoleon {}", a.label())));
                props.insert("preferred_trigger", Value::from(xdg_trigger(*a)));
                (format!("b{i}"), props)
            })
            .collect();

        let conn = self.conn.clone();
        let session = self.session.clone();
        let token = next_token(&self.token);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new().name("snap-portal-bind".into()).spawn(move || {
            let outcome = request(&conn, &token, |handle_token| {
                let mut options: HashMap<&str, Value<'_>> = HashMap::new();
                options.insert("handle_token", Value::from(handle_token.to_string()));
                conn.call_method(Some(PORTAL), PORTAL_PATH, Some(SHORTCUTS), "BindShortcuts", &(&session, &shortcuts, "", options))
            });
            let _ = tx.send(outcome.map(|_| ()).map_err(|e| e.to_string()));
        });
        if let Err(e) = spawned {
            return Err(e.to_string());
        }
        match rx.recv_timeout(BIND_WAIT) {
            Ok(r) => r,
            Err(_) => {
                tracing::info!("GlobalShortcuts: waiting for the user to confirm the shortcuts");
                Ok(())
            }
        }
    }

    /// Forget all shortcuts. The portal keeps its own binding until the session ends.
    pub fn reset(&mut self) {
        self.routes.ids.lock().clear();
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        let _ = self.conn.call_method(Some(PORTAL), self.session.as_str(), Some("org.freedesktop.portal.Session"), "Close", &());
        // Closing the connection ends the listener's message iterator.
        let _ = self.conn.clone().close();
        if let Some(t) = self.listener.take() {
            let _ = t.join();
        }
    }
}

fn next_token(counter: &AtomicU32) -> String {
    format!("snapoleon_{}_{}", std::process::id(), counter.fetch_add(1, Ordering::Relaxed))
}

/// Run a portal method that answers asynchronously through `Request::Response`
/// and return the response's result dictionary.
fn request(conn: &Connection, token: &str, call: impl FnOnce(&str) -> zbus::Result<Message>) -> zbus::Result<HashMap<String, OwnedValue>> {
    let sender = conn.unique_name().map(|n| n.as_str().trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let rule = MatchRule::builder().msg_type(Type::Signal).interface(REQUEST)?.member("Response")?.path(path)?.build();
    let mut responses = MessageIterator::for_match_rule(rule, conn, Some(4))?;
    call(token)?;
    let msg = responses.next().ok_or_else(|| zbus::Error::Failure("connection closed while waiting for the portal".into()))??;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    match code {
        0 => Ok(results),
        1 => Err(zbus::Error::Failure(t("platform.hotkey.portal_cancelled").into_owned())),
        _ => Err(zbus::Error::Failure("the portal request failed".into())),
    }
}

fn spawn_listener(
    conn: Connection,
    session: OwnedObjectPath,
    routes: Arc<Routes>,
    on_event: impl Fn(HotkeyEvent) + Send + 'static,
) -> Result<JoinHandle<()>> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(SHORTCUTS)
        .and_then(|b| b.path(PORTAL_PATH))
        .map_err(crate::util::other)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).map_err(crate::util::other)?;
    std::thread::Builder::new()
        .name("snap-portal-shortcuts".into())
        .spawn(move || {
            for msg in signals.flatten() {
                let header = msg.header();
                let pressed = match header.member().map(|m| m.as_str()) {
                    Some("Activated") => true,
                    Some("Deactivated") => false,
                    _ => continue,
                };
                let Ok((handle, id, _ts, _opts)) = msg.body().deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
                else {
                    continue;
                };
                if handle != session || routes.paused.load(Ordering::SeqCst) {
                    continue;
                }
                let Some(index) = id.strip_prefix('b').and_then(|n| n.parse::<u32>().ok()) else { continue };
                let Some(index) = routes.ids.lock().get(&index).copied() else { continue };
                on_event(if pressed { HotkeyEvent::Pressed(index) } else { HotkeyEvent::Released(index) });
            }
        })
        .map_err(Error::from)
}
