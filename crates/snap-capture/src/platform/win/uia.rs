//! UI Automation on a dedicated MTA worker thread.
//!
//! UIA calls cross process boundaries and can block on hung applications, so every query is sent to the
//! worker over a channel and the caller waits at most [`QUERY_TIMEOUT`]. Requests that
//! sat in the queue past their deadline are dropped instead of piling up behind a stuck call.

use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Sender, bounded, unbounded};
use snap_core::{Error, Point, Result};
use windows::{
    Win32::{
        Foundation::POINT,
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest, IUIAutomationElement,
                IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl, IUIAutomationValuePattern,
                TreeScope_Children, TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_CONTROLTYPE_ID,
                UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_IsOffscreenPropertyId, UIA_NamePropertyId,
                UIA_NativeWindowHandlePropertyId, UIA_ProcessIdPropertyId, UIA_ValuePatternId,
            },
            WindowsAndMessaging::{GA_ROOT, GetAncestor},
        },
    },
    core::{BSTR, Interface, Ref, implement},
};

use super::{com, os_err, process, rect_of, window};
use crate::{ElementInfo, WindowInfo};

/// How long an element query may block the caller.
const QUERY_TIMEOUT: Duration = Duration::from_millis(150);
/// The first query also pays for creating the UIA client.
const COLD_TIMEOUT: Duration = Duration::from_millis(1500);
const URL_TIMEOUT: Duration = Duration::from_millis(300);
/// How long UIA itself may wait on one provider before failing the call.
const PROVIDER_TIMEOUT_MS: u32 = 400;
const MAX_DEPTH: usize = 48;

enum Query {
    ElementsAt(Point),
    BrowserUrl { hwnd: isize, firefox: bool },
}

enum Reply {
    Elements(Result<Vec<ElementInfo>>),
    Url(Option<String>),
}

struct Request {
    query: Query,
    deadline: Instant,
    reply: Sender<Reply>,
}

static READY: AtomicBool = AtomicBool::new(false);

fn worker() -> &'static Sender<Request> {
    static WORKER: OnceLock<Sender<Request>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = unbounded::<Request>();
        thread::Builder::new()
            .name("snap-uia".into())
            .spawn(move || {
                com::ensure();
                let ctx = Ctx::new();
                READY.store(true, Ordering::Release);
                for req in rx {
                    if Instant::now() >= req.deadline {
                        continue;
                    }
                    let reply = match (&ctx, req.query) {
                        (Ok(c), Query::ElementsAt(p)) => Reply::Elements(c.elements_at(p)),
                        (Ok(c), Query::BrowserUrl { hwnd, firefox }) => Reply::Url(c.browser_url(hwnd, firefox)),
                        (Err(e), Query::ElementsAt(_)) => {
                            Reply::Elements(Err(Error::Other(anyhow::anyhow!("UI Automation unavailable: {e}"))))
                        }
                        (Err(_), Query::BrowserUrl { .. }) => Reply::Url(None),
                    };
                    let _ = req.reply.send(reply);
                }
            })
            .expect("spawn UI Automation worker");
        tx
    })
}

fn ask(query: Query, timeout: Duration) -> Result<Reply> {
    let timeout = if READY.load(Ordering::Acquire) { timeout } else { timeout.max(COLD_TIMEOUT) };
    let (reply, rx) = bounded(1);
    let req = Request { query, deadline: Instant::now() + timeout, reply };
    worker().send(req).map_err(|_| Error::Other(anyhow::anyhow!("UI Automation worker stopped")))?;
    rx.recv_timeout(timeout).map_err(|_| Error::Other(anyhow::anyhow!("UI Automation query timed out")))
}

/// Start the UI Automation worker now so the first real query is not slowed by client creation.
pub fn warm_up() {
    worker();
}

pub fn elements_at(p: Point) -> Result<Vec<ElementInfo>> {
    match ask(Query::ElementsAt(p), QUERY_TIMEOUT)? {
        Reply::Elements(r) => r,
        Reply::Url(_) => unreachable!("worker answers each query with its own reply kind"),
    }
}

const BROWSERS: &[&str] =
    &["chrome", "msedge", "firefox", "brave", "opera", "opera_gx", "vivaldi", "chromium", "thorium", "librewolf", "waterfox", "arc", "zen"];

pub fn browser_url(w: &WindowInfo) -> Option<String> {
    let exe = w.app_id.as_deref()?;
    let stem = process::exe_stem(exe);
    let yandex = stem == "browser" && exe.to_lowercase().contains("yandex");
    if !BROWSERS.contains(&stem.as_str()) && !yandex {
        return None;
    }
    let firefox = matches!(stem.as_str(), "firefox" | "librewolf" | "waterfox" | "zen");
    match ask(Query::BrowserUrl { hwnd: w.id as isize, firefox }, URL_TIMEOUT).ok()? {
        Reply::Url(u) => u,
        Reply::Elements(_) => None,
    }
}

/// Accepts the notification only to make UIA clients "listening"; browsers then expose their full tree.
#[implement(IUIAutomationFocusChangedEventHandler)]
struct FocusSink;

impl IUIAutomationFocusChangedEventHandler_Impl for FocusSink_Impl {
    fn HandleFocusChangedEvent(&self, _sender: Ref<IUIAutomationElement>) -> windows::core::Result<()> {
        Ok(())
    }
}

struct Ctx {
    auto: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    _focus_sink: IUIAutomationFocusChangedEventHandler,
}

impl Ctx {
    fn new() -> Result<Self> {
        let auto: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.map_err(|e| os_err("CUIAutomation", e))?;
        if let Ok(a2) = auto.cast::<IUIAutomation2>() {
            unsafe {
                let _ = a2.SetConnectionTimeout(PROVIDER_TIMEOUT_MS);
                let _ = a2.SetTransactionTimeout(PROVIDER_TIMEOUT_MS);
            }
        }
        let cache = unsafe { auto.CreateCacheRequest() }.map_err(|e| os_err("CreateCacheRequest", e))?;
        for prop in [
            UIA_NamePropertyId,
            UIA_ControlTypePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_ProcessIdPropertyId,
            UIA_NativeWindowHandlePropertyId,
            UIA_IsOffscreenPropertyId,
        ] {
            unsafe { cache.AddProperty(prop) }.map_err(|e| os_err("AddProperty", e))?;
        }
        let sink: IUIAutomationFocusChangedEventHandler = FocusSink.into();
        let _ = unsafe { auto.AddFocusChangedEventHandler(None::<&IUIAutomationCacheRequest>, &sink) };
        Ok(Self { auto, cache, _focus_sink: sink })
    }

    /// Element chain at `p`, smallest to largest.
    fn elements_at(&self, p: Point) -> Result<Vec<ElementInfo>> {
        let own_pid = std::process::id() as i32;
        let hit = unsafe { self.auto.ElementFromPointBuildCache(POINT { x: p.x, y: p.y }, &self.cache) }
            .map_err(|e| os_err("ElementFromPoint", e))?;
        let chain = if unsafe { hit.CachedProcessId() }.is_ok_and(|pid| pid == own_pid) {
            // Our own overlay covers the screen: look through it at the window underneath.
            self.descend_through_windows(p)?
        } else {
            self.ancestors(hit)
        };
        Ok(self.to_infos(&chain))
    }

    fn ancestors(&self, leaf: IUIAutomationElement) -> Vec<IUIAutomationElement> {
        let mut chain = vec![leaf];
        let Ok(walker) = (unsafe { self.auto.ControlViewWalker() }) else { return chain };
        let root = unsafe { self.auto.GetRootElement() }.ok();
        while chain.len() < MAX_DEPTH {
            let Ok(parent) = (unsafe { walker.GetParentElementBuildCache(chain.last().expect("non-empty"), &self.cache) }) else { break };
            let is_root = root.as_ref().is_some_and(|r| unsafe { self.auto.CompareElements(r, &parent) }.is_ok_and(|b| b.as_bool()));
            if is_root {
                break;
            }
            chain.push(parent);
        }
        chain
    }

    /// Walk down from the front-most foreign window containing `p`, one level per cached `FindAll`.
    fn descend_through_windows(&self, p: Point) -> Result<Vec<IUIAutomationElement>> {
        let target = window::list()?
            .into_iter()
            .find(|w| !w.minimized && w.rect.contains(p))
            .ok_or_else(|| Error::NotFound("no window at point".into()))?;
        let top = unsafe { self.auto.ElementFromHandleBuildCache(window::hwnd_from_id(target.id), &self.cache) }
            .map_err(|e| os_err("ElementFromHandle", e))?;
        let condition = unsafe { self.auto.ControlViewCondition() }.map_err(|e| os_err("ControlViewCondition", e))?;

        let mut chain = vec![top];
        while chain.len() < MAX_DEPTH {
            let cur = chain.last().expect("non-empty");
            let Ok(children) = (unsafe { cur.FindAllBuildCache(TreeScope_Children, &condition, &self.cache) }) else { break };
            let count = unsafe { children.Length() }.unwrap_or(0);
            // Later siblings are drawn on top, so scan from the end.
            let next = (0..count).rev().filter_map(|i| unsafe { children.GetElement(i) }.ok()).find(|c| {
                !unsafe { c.CachedIsOffscreen() }.is_ok_and(|b| b.as_bool())
                    && unsafe { c.CachedBoundingRectangle() }.is_ok_and(|r| rect_of(r).contains(p))
            });
            match next {
                Some(c) => chain.push(c),
                None => break,
            }
        }
        chain.reverse();
        Ok(chain)
    }

    fn to_infos(&self, chain: &[IUIAutomationElement]) -> Vec<ElementInfo> {
        let mut out: Vec<ElementInfo> = Vec::with_capacity(chain.len());
        let mut apps: Vec<(i32, Option<String>)> = Vec::new();
        for el in chain {
            let Ok(raw) = (unsafe { el.CachedBoundingRectangle() }) else { continue };
            let rect = rect_of(raw);
            if rect.is_empty() || unsafe { el.CachedIsOffscreen() }.is_ok_and(|b| b.as_bool()) {
                continue;
            }
            let name = unsafe { el.CachedName() }.map(|n| n.to_string()).unwrap_or_default();
            // A parent that occupies exactly the same area adds nothing to cycle through; keep the
            // smaller element but borrow a name if it had none.
            if let Some(prev) = out.last_mut()
                && prev.rect == rect
            {
                if prev.name.is_empty() {
                    prev.name = name;
                }
                continue;
            }
            let pid = unsafe { el.CachedProcessId() }.unwrap_or(0);
            let app_name = match apps.iter().find(|(p, _)| *p == pid) {
                Some((_, n)) => n.clone(),
                None => {
                    let n = Some(process::app_info(pid as u32).name).filter(|n| !n.is_empty());
                    apps.push((pid, n.clone()));
                    n
                }
            };
            let role = role_name(unsafe { el.CachedControlType() }.unwrap_or_default());
            out.push(ElementInfo { rect, name, role, app_name, window_id: None });
        }

        // The largest element that owns a native window identifies the top-level window.
        let window_id = chain.iter().rev().find_map(|el| {
            let h = unsafe { el.CachedNativeWindowHandle() }.ok().filter(|h| !h.is_invalid())?;
            let root = unsafe { GetAncestor(h, GA_ROOT) };
            Some(window::id_of(if root.is_invalid() { h } else { root }))
        });
        for e in &mut out {
            e.window_id = window_id;
        }
        out
    }

    fn browser_url(&self, hwnd: isize, firefox: bool) -> Option<String> {
        let h = window::hwnd_from_id(hwnd as u64);
        let root = unsafe { self.auto.ElementFromHandle(h) }.ok()?;
        let edit = VARIANT::from(UIA_EditControlTypeId.0);

        let by_property = |prop, value: BSTR| {
            let class = unsafe { self.auto.CreatePropertyCondition(prop, &VARIANT::from(value)) }.ok()?;
            let kind = unsafe { self.auto.CreatePropertyCondition(UIA_ControlTypePropertyId, &edit) }.ok()?;
            let both = unsafe { self.auto.CreateAndCondition(&kind, &class) }.ok()?;
            unsafe { root.FindFirst(TreeScope_Descendants, &both) }.ok()
        };
        // Chromium's omnibox and Firefox's URL bar have stable identifiers; any other Edit is a last resort.
        let bar = if firefox {
            by_property(UIA_AutomationIdPropertyId, "urlbar-input".into())
        } else {
            by_property(UIA_ClassNamePropertyId, "OmniboxViewViews".into())
        }
        .or_else(|| {
            let kind = unsafe { self.auto.CreatePropertyCondition(UIA_ControlTypePropertyId, &edit) }.ok()?;
            let all = unsafe { root.FindAll(TreeScope_Descendants, &kind) }.ok()?;
            (0..unsafe { all.Length() }.ok()?).filter_map(|i| unsafe { all.GetElement(i) }.ok()).find(|e| edit_url(e).is_some())
        })?;
        edit_url(&bar)
    }
}

/// URL text of an address-bar element, normalised to include a scheme. Free-form search text is rejected.
fn edit_url(el: &IUIAutomationElement) -> Option<String> {
    let pattern: IUIAutomationValuePattern = unsafe { el.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
    let text = unsafe { pattern.CurrentValue() }.ok()?.to_string();
    normalize_url(&text)
}

fn normalize_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    if t.contains("://") || t.starts_with("about:") || t.starts_with("file:") {
        return Some(t.to_string());
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or(t);
    let local = host.starts_with("localhost") || host.split(':').next().is_some_and(|h| h.parse::<std::net::Ipv4Addr>().is_ok());
    if !host.contains('.') && !local {
        return None;
    }
    Some(format!("{}://{t}", if local { "http" } else { "https" }))
}

/// Lower-case English name of a UIA control type (50000 = Button ... 50040 = AppBar).
fn role_name(t: UIA_CONTROLTYPE_ID) -> String {
    const NAMES: [&str; 41] = [
        "button",
        "calendar",
        "check box",
        "combo box",
        "edit",
        "hyperlink",
        "image",
        "list item",
        "list",
        "menu",
        "menu bar",
        "menu item",
        "progress bar",
        "radio button",
        "scroll bar",
        "slider",
        "spinner",
        "status bar",
        "tab",
        "tab item",
        "text",
        "tool bar",
        "tool tip",
        "tree",
        "tree item",
        "custom",
        "group",
        "thumb",
        "data grid",
        "data item",
        "document",
        "split button",
        "window",
        "pane",
        "header",
        "header item",
        "table",
        "title bar",
        "separator",
        "semantic zoom",
        "app bar",
    ];
    usize::try_from(t.0 - 50000).ok().and_then(|i| NAMES.get(i)).copied().unwrap_or("element").to_string()
}
