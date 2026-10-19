//! Windows / macOS tray through `tray-icon`.

use std::sync::{Once, OnceLock};

use image::RgbaImage;
use parking_lot::Mutex;
use snap_core::Result;
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
        let tray = TrayIconBuilder::new()
            .with_tooltip(tooltip)
            .with_icon(to_icon(icon)?)
            .with_menu(Box::new(build_menu(&items)?))
            // Windows: left click reports `Click`, right click opens the menu.
            // macOS convention: any click opens the menu.
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .build()
            .map_err(other)?;
        Ok(Self { tray })
    }

    pub fn set_items(&mut self, items: Vec<TrayItem>) {
        match build_menu(&items) {
            Ok(menu) => self.tray.set_menu(Some(Box::new(menu))),
            Err(e) => tracing::warn!("tray menu: {e}"),
        }
    }

    pub fn set_icon(&mut self, icon: &RgbaImage) {
        match to_icon(icon) {
            Ok(icon) => {
                if let Err(e) = self.tray.set_icon(Some(icon)) {
                    tracing::warn!("tray icon: {e}");
                }
            }
            Err(e) => tracing::warn!("tray icon: {e}"),
        }
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

fn to_icon(img: &RgbaImage) -> Result<Icon> {
    Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()).map_err(other)
}

fn build_menu(items: &[TrayItem]) -> Result<Menu> {
    let menu = Menu::new();
    for item in items {
        menu.append(&*make_item(item)?).map_err(other)?;
    }
    Ok(menu)
}

/// `&` introduces a mnemonic in native menus; double it to show it literally.
fn label(text: &str) -> String {
    text.replace('&', "&&")
}

fn make_item(item: &TrayItem) -> Result<Box<dyn IsMenuItem>> {
    if item.separator {
        return Ok(Box::new(PredefinedMenuItem::separator()));
    }
    if !item.children.is_empty() {
        let sub = Submenu::with_id(item.id.as_str(), label(&item.label), item.enabled);
        for child in &item.children {
            sub.append(&*make_item(child)?).map_err(other)?;
        }
        return Ok(Box::new(sub));
    }
    Ok(match item.checked {
        Some(checked) => Box::new(CheckMenuItem::with_id(item.id.as_str(), label(&item.label), item.enabled, checked, None)),
        None => Box::new(MenuItem::with_id(item.id.as_str(), label(&item.label), item.enabled, None)),
    })
}
