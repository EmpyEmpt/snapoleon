//! Accessibility elements and browser addresses through AT-SPI, spoken on the accessibility D-Bus with zbus' blocking
//! API. Every query runs on a helper thread with a deadline, because an application that has stopped answering must
//! not freeze the overlay.
//!
//! Applications only expose their trees when the desktop has accessibility turned on (GNOME: Settings, Accessibility;
//! most Qt and GTK apps then follow). Chromium-based browsers additionally need `--force-renderer-accessibility` or
//! `chrome://accessibility` before they show page contents, so their address stays unknown without it.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use snap_core::{Error, Point, Rect, Result};
use zbus::{
    blocking::{Connection, connection::Builder},
    zvariant::{OwnedObjectPath, OwnedValue},
};

use crate::{ElementInfo, WindowInfo};

/// A warm connection answers in a few milliseconds; the first query also connects.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const COLD_TIMEOUT: Duration = Duration::from_millis(4000);
/// One D-Bus call to an application. A query makes many, so this is far below `QUERY_TIMEOUT`: an application that
/// stopped answering costs a call or two, and the worker returns (releasing `BUSY`) close to the query's deadline.
const CALL_TIMEOUT: Duration = Duration::from_millis(250);
/// How long a query keeps going through applications and their windows.
const SCAN_BUDGET: Duration = Duration::from_millis(1000);
/// Toolkit scale factors (`GDK_SCALE`, `QT_SCALE_FACTOR`) under which extents come in logical, not physical, pixels.
const TOOLKIT_SCALES: [i32; 3] = [1, 2, 3];
/// How far a window's extents may differ from the window manager's rectangle for it (frames, shadows).
const FRAME_SLACK: i32 = 48;
/// Looking for the address bar's document may walk a big tree.
const URL_BUDGET: Duration = Duration::from_millis(800);
/// Nodes visited at most while looking for a document.
const MAX_NODES: usize = 4000;
const MAX_DEPTH: usize = 40;
/// Coordinates relative to the screen (`ATSPI_COORD_TYPE_SCREEN`).
const SCREEN: u32 = 0;
const NULL_PATH: &str = "/org/a11y/atspi/null";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const DOCUMENT: &str = "org.a11y.atspi.Document";

/// An accessible object: the unique bus name of its application and its object path.
type Node = (String, OwnedObjectPath);

fn bus_error(e: zbus::Error) -> Error {
    Error::Other(anyhow::anyhow!("AT-SPI: {e}"))
}

/// The connection to the accessibility bus, made on first use and kept.
fn connection() -> Result<Connection> {
    static BUS: Mutex<Option<Connection>> = Mutex::new(None);
    let mut slot = BUS.lock().map_err(|_| Error::Other(anyhow::anyhow!("AT-SPI connection lock")))?;
    if let Some(conn) = slot.as_ref() {
        return Ok(conn.clone());
    }
    let session = Connection::session().map_err(|_| Error::unsupported("accessibility elements need a D-Bus session"))?;
    let address: String = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .and_then(|reply| reply.body().deserialize())
        .map_err(|_| Error::unsupported("accessibility elements: the AT-SPI service is not running"))?;
    // zbus waits for a reply forever by default, and so does the bus: one stuck application would hold the worker.
    let conn = Builder::address(address.as_str()).map(|b| b.method_timeout(CALL_TIMEOUT)).and_then(Builder::build).map_err(bus_error)?;
    *slot = Some(conn.clone());
    Ok(conn)
}

/// Run `work` on its own thread and wait at most `timeout`. One query at a time: while a stuck one still runs,
/// the next fails at once instead of piling up threads.
fn run<T: Send + 'static>(timeout: Duration, work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    static BUSY: AtomicBool = AtomicBool::new(false);
    static WARM: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::AcqRel) {
        return Err(Error::Other(anyhow::anyhow!("an accessibility query is still running")));
    }
    let timeout = if WARM.load(Ordering::Acquire) { timeout } else { timeout.max(COLD_TIMEOUT) };
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new().name("snap-atspi".into()).spawn(move || {
        let result = work();
        WARM.store(true, Ordering::Release);
        BUSY.store(false, Ordering::Release);
        let _ = tx.send(result);
    });
    if let Err(e) = spawned {
        BUSY.store(false, Ordering::Release);
        return Err(Error::Io(e));
    }
    rx.recv_timeout(timeout).map_err(|_| Error::Other(anyhow::anyhow!("the accessibility query timed out")))?
}

fn call<B, R>(conn: &Connection, node: &Node, interface: &str, method: &str, body: &B) -> zbus::Result<R>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: serde::de::DeserializeOwned + zbus::zvariant::Type,
{
    conn.call_method(Some(node.0.as_str()), &node.1, Some(interface), method, body)?.body().deserialize()
}

fn text_property(conn: &Connection, node: &Node, name: &str) -> String {
    call::<_, OwnedValue>(conn, node, "org.freedesktop.DBus.Properties", "Get", &(ACCESSIBLE, name))
        .ok()
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_default()
}

fn children(conn: &Connection, node: &Node) -> Vec<Node> {
    call(conn, node, ACCESSIBLE, "GetChildren", &()).unwrap_or_default()
}

/// Extents on the screen, as the toolkit reports them (logical pixels under a toolkit scale); `None` when the object
/// has none or they are empty.
fn extents(conn: &Connection, node: &Node) -> Option<Rect> {
    let (x, y, w, h): (i32, i32, i32, i32) = call(conn, node, COMPONENT, "GetExtents", &(SCREEN,)).ok()?;
    let rect = Rect::new(x, y, u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    (!rect.is_empty()).then_some(rect)
}

fn scaled(r: Rect, scale: i32) -> Rect {
    let s = scale.unsigned_abs();
    Rect::new(r.x * scale, r.y * scale, r.w * s, r.h * s)
}

/// Whether toolkit extents `r` describe the window the window manager reports as `window`. Native Wayland clients
/// cannot know their global position and report extents from the origin, which never agree.
fn agrees(r: Rect, window: Rect) -> bool {
    [(r.x, window.x), (r.y, window.y), (r.right(), window.right()), (r.bottom(), window.bottom())]
        .iter()
        .all(|(a, b)| (a - b).abs() <= FRAME_SLACK)
}

fn covers(outer: Rect, inner: Rect) -> bool {
    outer.intersect(inner) == Some(inner)
}

fn registry() -> Node {
    ("org.a11y.atspi.Registry".into(), OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").expect("a valid object path"))
}

fn process_of(conn: &Connection, bus_name: &str) -> Option<u32> {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetConnectionUnixProcessID",
        &(bus_name,),
    )
    .ok()?
    .body()
    .deserialize()
    .ok()
}

/// The application objects of the desktop, restricted to process `pid` when given.
fn applications(conn: &Connection, pid: Option<u32>) -> Vec<Node> {
    let all = children(conn, &registry());
    match pid {
        Some(pid) => {
            let own: Vec<Node> = all.iter().filter(|app| process_of(conn, &app.0) == Some(pid)).cloned().collect();
            if own.is_empty() { all } else { own }
        }
        None => all,
    }
}

/// Elements under `p`, smallest first, each containing the one before.
pub fn elements_at(p: Point) -> Result<Vec<ElementInfo>> {
    // The window on top at `p` tells which application to ask and where its frame has to be. Toolkit extents are only
    // global on X11, so without the window manager's list (native Wayland) there is nothing to check them against.
    let windows = crate::windows()
        .ok()
        .filter(|ws| !ws.is_empty())
        .ok_or_else(|| Error::unsupported("accessibility elements need the window list of an X11 session"))?;
    let Some(top) = windows.into_iter().filter(|w| !w.minimized && w.rect.contains(p)).min_by_key(|w| w.z) else {
        return Ok(Vec::new());
    };
    run(QUERY_TIMEOUT, move || {
        let conn = connection()?;
        let began = Instant::now();
        let apps = applications(&conn, Some(top.pid).filter(|pid| *pid != 0));
        // Of the frames of the top window holding `p`, the smallest is the one in front: dialogs and popups sit over big
        // windows. Windows of other applications behind it are ignored, and so is a frame whose extents are not the
        // top window's (a Flatpak application's frames are found by their position, not their process).
        let mut found: Option<(u64, &Node, Node, i32)> = None;
        'apps: for app in &apps {
            for frame in children(&conn, app) {
                if began.elapsed() > SCAN_BUDGET {
                    break 'apps;
                }
                let Some(raw) = extents(&conn, &frame) else { continue };
                let Some(scale) = TOOLKIT_SCALES.into_iter().find(|s| agrees(scaled(raw, *s), top.rect)) else { continue };
                let physical = scaled(raw, scale);
                if physical.contains(p) && found.as_ref().is_none_or(|(best, ..)| physical.area() < *best) {
                    found = Some((physical.area(), app, frame, scale));
                }
            }
        }
        let Some((_, app, frame, scale)) = found else { return Ok(Vec::new()) };
        let app_name = Some(text_property(&conn, app, "Name")).filter(|n| !n.is_empty());

        // The toolkit speaks logical pixels, the caller physical ones.
        let (x, y) = (p.x / scale, p.y / scale);
        let mut chain = vec![frame];
        for _ in 0..MAX_DEPTH {
            let last = chain.last().expect("the chain starts with the frame");
            let Ok(next) = call::<_, Node>(&conn, last, COMPONENT, "GetAccessibleAtPoint", &(x, y, SCREEN)) else { break };
            if next.0.is_empty() || next.1.as_str() == NULL_PATH || chain.contains(&next) {
                break;
            }
            chain.push(next);
        }

        let mut elements: Vec<ElementInfo> = Vec::new();
        for node in chain.iter().rev() {
            let Some(rect) = extents(&conn, node).map(|r| scaled(r, scale)) else { continue };
            if elements.last().is_some_and(|inner| !covers(rect, inner.rect)) {
                continue;
            }
            let role = call::<_, String>(&conn, node, ACCESSIBLE, "GetRoleName", &()).unwrap_or_default().to_lowercase();
            elements.push(ElementInfo {
                rect,
                name: text_property(&conn, node, "Name"),
                role,
                app_name: app_name.clone(),
                window_id: Some(top.id),
            });
        }
        Ok(elements)
    })
}

/// The address of the page a browser window shows, from the `DocURL` (or `URI`) attribute of its web document.
/// `None` for anything but a browser that exposes its page, and whenever the lookup is slow.
pub fn browser_url(w: &WindowInfo) -> Option<String> {
    const BROWSERS: &[&str] = &[
        "chrome",
        "chromium",
        "google-chrome",
        "brave",
        "brave-browser",
        "vivaldi",
        "opera",
        "msedge",
        "firefox",
        "librewolf",
        "waterfox",
        "zen",
        "epiphany",
    ];
    let exe = w.app_id.as_deref()?;
    let stem = exe.rsplit('/').next().unwrap_or(exe).to_lowercase();
    if !BROWSERS.iter().any(|b| stem.starts_with(b)) {
        return None;
    }
    let (pid, title) = (w.pid, w.title.clone());
    run(QUERY_TIMEOUT, move || {
        let conn = connection()?;
        let began = Instant::now();
        let mut seen = 0;
        for app in applications(&conn, Some(pid)) {
            for frame in children(&conn, &app) {
                if began.elapsed() > URL_BUDGET {
                    return Ok(None);
                }
                // Only the frame of this window: browsers have one per window.
                let name = text_property(&conn, &frame, "Name");
                if !title.is_empty() && !name.is_empty() && !title.contains(&name) && !name.contains(&title) {
                    continue;
                }
                if let Some(url) = document_url(&conn, &frame, 0, &mut seen, began) {
                    return Ok(Some(url));
                }
            }
        }
        Ok(None)
    })
    .ok()
    .flatten()
}

/// Depth-first search below `node` for the web document and its address.
fn document_url(conn: &Connection, node: &Node, depth: usize, seen: &mut usize, began: Instant) -> Option<String> {
    *seen += 1;
    if depth > MAX_DEPTH || *seen > MAX_NODES || began.elapsed() > URL_BUDGET {
        return None;
    }
    if call::<_, String>(conn, node, ACCESSIBLE, "GetRoleName", &()).is_ok_and(|role| role.starts_with("document")) {
        for attribute in ["DocURL", "URI"] {
            if let Ok(url) = call::<_, String>(conn, node, DOCUMENT, "GetAttributeValue", &(attribute,))
                && !url.is_empty()
            {
                return Some(url);
            }
        }
    }
    children(conn, node).iter().find_map(|child| document_url(conn, child, depth + 1, seen, began))
}
