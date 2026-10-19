//! Linux tray: StatusNotifierItem over D-Bus through `ksni` (no GTK).

use image::RgbaImage;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use snap_core::{APP_ID, APP_NAME, Error, Result};

use super::{Callback, TrayEvent, TrayItem};

struct Sni {
    tooltip: String,
    icon: ksni::Icon,
    items: Vec<TrayItem>,
    on_event: Callback,
}

impl ksni::Tray for Sni {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.tooltip.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_event)(TrayEvent::Click);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        (self.on_event)(TrayEvent::DoubleClick);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        convert(&self.items, &self.on_event)
    }
}

pub(super) struct Backend {
    handle: Handle<Sni>,
}

impl Backend {
    pub fn new(tooltip: &str, icon: &RgbaImage, items: Vec<TrayItem>, on_event: Callback) -> Result<Self> {
        let sni = Sni { tooltip: tooltip.into(), icon: to_icon(icon), items, on_event };
        // The panel may start after us (autostart): keep the item registered and
        // let it appear once a StatusNotifierHost shows up.
        let handle =
            sni.assume_sni_available(true).spawn().map_err(|e| Error::unsupported(format!("StatusNotifierItem tray unavailable: {e}")))?;
        Ok(Self { handle })
    }

    pub fn set_items(&mut self, items: Vec<TrayItem>) {
        self.handle.update(|t| t.items = items);
    }

    pub fn set_icon(&mut self, icon: &RgbaImage) {
        let icon = to_icon(icon);
        self.handle.update(|t| t.icon = icon);
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        self.handle.update(|t| t.tooltip = tooltip.into());
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

/// SNI pixmaps are ARGB32 in network byte order.
fn to_icon(img: &RgbaImage) -> ksni::Icon {
    let data = img.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    ksni::Icon { width: img.width() as i32, height: img.height() as i32, data }
}

/// `_` introduces a mnemonic in SNI labels; double it to show it literally.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

fn convert(items: &[TrayItem], cb: &Callback) -> Vec<MenuItem<Sni>> {
    items
        .iter()
        .map(|item| {
            if item.separator {
                return MenuItem::Separator;
            }
            if !item.children.is_empty() {
                return SubMenu {
                    label: label(&item.label),
                    enabled: item.enabled,
                    submenu: convert(&item.children, cb),
                    ..Default::default()
                }
                .into();
            }
            let id = item.id.clone();
            let cb = cb.clone();
            let activate = Box::new(move |_: &mut Sni| cb(TrayEvent::Menu(id.clone())));
            match item.checked {
                Some(checked) => {
                    CheckmarkItem { label: label(&item.label), enabled: item.enabled, checked, activate, ..Default::default() }.into()
                }
                None => StandardItem { label: label(&item.label), enabled: item.enabled, activate, ..Default::default() }.into(),
            }
        })
        .collect()
}
