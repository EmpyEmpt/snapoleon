//! Windows / macOS tray through `tray-icon`.

use std::sync::{Once, OnceLock};

use image::RgbaImage;
use parking_lot::Mutex;
use snap_core::Result;
use tray_icon::menu::accelerator::Accelerator;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{Callback, TrayEvent, TrayItem};
use crate::util::other;

/// The `tray-icon` handlers are process-wide and can be installed only once, so
/// they forward to whichever callback the current [`Tray`](super::Tray) registered.
fn current() -> &'static Mutex<Option<Callback>> {
    static CURRENT: OnceLock<Mutex<Option<Callback>>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(None))
}

fn dispatch(event: TrayEvent) {
    let cb = current().lock().clone();
    if let Some(cb) = cb {
        cb(event);
    }
}

fn install_handlers() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        TrayIconEvent::set_event_handler(Some(|ev: TrayIconEvent| match ev {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => dispatch(TrayEvent::Click),
            TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => dispatch(TrayEvent::DoubleClick),
            _ => {}
        }));
        MenuEvent::set_event_handler(Some(|ev: MenuEvent| dispatch(TrayEvent::Menu(ev.id.0))));
    });
}

pub(super) struct Backend {
    tray: TrayIcon,
}

impl Backend {
    pub fn new(tooltip: &str, icon: &RgbaImage, items: Vec<TrayItem>, on_event: Callback) -> Result<Self> {
        install_handlers();
        *current().lock() = Some(on_event);
        let menu = build_menu(&items)?;
        #[cfg(target_os = "macos")]
        let status_menu = ns_menu(&menu);
        let builder = TrayIconBuilder::new()
            .with_tooltip(tooltip)
            // The tray keeps the menu alive: its items call back into it.
            .with_menu(Box::new(menu))
            // Windows: left click reports `Click`, right click opens the menu. macOS: see `attach_menu`.
            .with_menu_on_left_click(false);
        // A template follows the menu bar's own colour, which on macOS 26+ follows the wallpaper rather than the
        // app's light or dark appearance.
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(to_icon(icon)?);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(to_icon(icon)?);
        let tray = builder.build().map_err(other)?;
        #[cfg(target_os = "macos")]
        if let Some(button) = status_button(&tray) {
            // Without a label VoiceOver and UI automation see an unnamed button.
            use objc2_app_kit::NSAccessibility;
            button.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(snap_core::APP_NAME)));
            attach_menu(&tray, &button, status_menu.as_deref());
        }
        Ok(Self { tray })
    }

    pub fn set_items(&mut self, items: Vec<TrayItem>) {
        match build_menu(&items) {
            Ok(menu) => {
                #[cfg(target_os = "macos")]
                if let Some(item) = self.tray.ns_status_item() {
                    item.setMenu(ns_menu(&menu).as_deref());
                }
                self.tray.set_menu(Some(Box::new(menu)));
            }
            Err(e) => tracing::warn!("tray menu: {e}"),
        }
    }

    pub fn set_icon(&mut self, icon: &RgbaImage) {
        match to_icon(icon) {
            Ok(icon) => {
                #[cfg(target_os = "macos")]
                let set = self.tray.set_icon_templated(Some(icon));
                #[cfg(not(target_os = "macos"))]
                let set = self.tray.set_icon(Some(icon));
                if let Err(e) = set {
                    tracing::warn!("tray icon: {e}");
                }
            }
            Err(e) => tracing::warn!("tray icon: {e}"),
        }
    }

    /// A template ignores the icon's colours, so a running recording tints the whole glyph instead.
    #[cfg(target_os = "macos")]
    pub fn set_recording(&mut self, recording: bool) {
        if let Some(button) = status_button(&self.tray) {
            button.setContentTintColor(recording.then(objc2_app_kit::NSColor::systemRedColor).as_deref());
        }
    }

    /// Highlight the status item on and off, as if it was clicked, `FLASHES` times.
    #[cfg(target_os = "macos")]
    pub fn flash(&mut self) {
        use std::cell::Cell;
        use std::ptr::NonNull;

        use objc2_foundation::NSTimer;

        const FLASHES: u32 = 3;
        let Some(button) = status_button(&self.tray) else { return };
        let ticks = Cell::new(0u32);
        let blink = block2::RcBlock::new(move |timer: NonNull<NSTimer>| {
            let tick = ticks.get() + 1;
            ticks.set(tick);
            button.highlight(tick % 2 == 1);
            if tick >= 2 * FLASHES {
                // SAFETY: the timer passes itself, and it is alive for the duration of the call.
                unsafe { timer.as_ref() }.invalidate();
            }
        });
        // SAFETY: a scheduled timer fires on the run loop of the thread that scheduled it, which is this one: the main
        // thread (`status_button` checked), the only thread that touches the button.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.3, true, &blink) };
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        if let Err(e) = self.tray.set_tooltip(Some(tooltip)) {
            tracing::warn!("tray tooltip: {e}");
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        *current().lock() = None;
    }
}

#[cfg(target_os = "macos")]
fn status_button(tray: &TrayIcon) -> Option<objc2::rc::Retained<objc2_app_kit::NSStatusBarButton>> {
    let mtm = objc2::MainThreadMarker::new()?;
    tray.ns_status_item()?.button(mtm)
}

/// Make the menu the status item's own, as a standard menu bar extra has it: AppKit then opens it on any click, and
/// on the button's AXPress, so VoiceOver and UI automation can open it too.
///
/// `tray-icon` 0.26 instead lays a view of its own (`TaoTrayTarget`) over the button that opens the menu from
/// `mouseDown:` only, and clears the status item's menu after each click. That view goes, so the button gets the
/// clicks itself; `tray-icon` then reports no click events, which macOS has no use for: any click opens the menu.
#[cfg(target_os = "macos")]
fn attach_menu(tray: &TrayIcon, button: &objc2_app_kit::NSStatusBarButton, menu: Option<&objc2_app_kit::NSMenu>) {
    let Some(item) = tray.ns_status_item() else { return };
    item.setMenu(menu);
    let mut detached = false;
    for view in button.subviews() {
        if view.class().name() == c"TaoTrayTarget" {
            view.removeFromSuperview();
            detached = true;
        }
    }
    if !detached {
        tracing::warn!("tray: tray-icon's click view not found; the status menu may not open on click");
    }
}

/// The `NSMenu` behind `menu`, retained so it outlives the borrow.
#[cfg(target_os = "macos")]
fn ns_menu(menu: &Menu) -> Option<objc2::rc::Retained<objc2_app_kit::NSMenu>> {
    use tray_icon::menu::ContextMenu;
    // SAFETY: muda hands out the pointer of the `NSMenu` it owns, valid for as long as `menu` is, which spans the call.
    unsafe { objc2::rc::Retained::retain(menu.ns_menu().cast::<objc2_app_kit::NSMenu>()) }
}

fn to_icon(img: &RgbaImage) -> Result<Icon> {
    Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()).map_err(other)
}

fn build_menu(items: &[TrayItem]) -> Result<Menu> {
    let menu = Menu::new();
    for item in items {
        add(&|i| menu.append(i), item)?;
    }
    Ok(menu)
}

/// `&` introduces a mnemonic in native menus; double it to show it literally.
fn label(text: &str) -> String {
    text.replace('&', "&&")
}

/// macOS shows the shortcut as the item's key equivalent. These menus are not the main menu, so the key equivalent
/// only works while the menu is open and never competes with the global hotkey.
#[cfg(target_os = "macos")]
fn accelerator(item: &TrayItem) -> Option<Accelerator> {
    let accel = crate::hotkeys::accel::Accel::parse(item.accel.as_deref()?).ok()?;
    Some(Accelerator::new(accel.mods, accel.key))
}

/// Windows menus right-align text after a tab as the shortcut column; a muda accelerator would also bind the keys.
#[cfg(windows)]
fn accelerator(_item: &TrayItem) -> Option<Accelerator> {
    None
}

fn text(item: &TrayItem) -> String {
    #[cfg(windows)]
    if let Some(accel) = &item.accel {
        return format!("{}\t{}", label(&item.label), crate::hotkeys::display(accel));
    }
    label(&item.label)
}

/// Build `item` and hand it to `append`, which adds it to its parent menu.
fn add(append: &dyn Fn(&dyn IsMenuItem) -> tray_icon::menu::Result<()>, item: &TrayItem) -> Result<()> {
    if item.separator {
        return append(&PredefinedMenuItem::separator()).map_err(other);
    }
    if !item.children.is_empty() {
        let sub = Submenu::with_id(item.id.as_str(), label(&item.label), item.enabled);
        for child in &item.children {
            add(&|i| sub.append(i), child)?;
        }
        return append(&sub).map_err(other);
    }
    let make = |accel: Option<Accelerator>| -> Box<dyn IsMenuItem> {
        match item.checked {
            Some(checked) => Box::new(CheckMenuItem::with_id(item.id.as_str(), text(item), item.enabled, checked, accel)),
            None => Box::new(MenuItem::with_id(item.id.as_str(), text(item), item.enabled, accel)),
        }
    };
    let accel = accelerator(item);
    match append(&*make(accel)) {
        // A key muda has no key equivalent for fails the item; it must never cost the whole menu (without a tray the
        // app quits with its last window).
        Err(e) if accel.is_some() => {
            tracing::debug!("tray shortcut for '{}' not shown: {e}", item.id);
            append(&*make(None)).map_err(other)
        }
        result => result.map_err(other),
    }
}
