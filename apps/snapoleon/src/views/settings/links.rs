//! Links: how a link is copied, URL shortening and the default expiry of server links.

use egui::Ui;
use snap_core::{
    secrets::secret_key,
    settings::{LinkSettings, Shortener},
    tf,
};

use super::{
    kit::{Rows, SecretSlot, field_mono, opts, s},
    pipelines::link_format_options,
};
use crate::views::ViewCx;

#[derive(Default)]
pub struct State {
    secret: SecretSlot,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    None,
    Server,
    Yourls,
    Shlink,
    IsGd,
}

impl Kind {
    fn of(shortener: &Shortener) -> Self {
        match shortener {
            Shortener::None => Kind::None,
            Shortener::SnapServer => Kind::Server,
            Shortener::Yourls { .. } => Kind::Yourls,
            Shortener::Shlink { .. } => Kind::Shlink,
            Shortener::IsGd => Kind::IsGd,
        }
    }

    fn fresh(self) -> Shortener {
        match self {
            Kind::None => Shortener::None,
            Kind::Server => Shortener::SnapServer,
            Kind::Yourls => Shortener::Yourls { url: String::new(), secret_ref: None },
            Kind::Shlink => Shortener::Shlink { url: String::new(), secret_ref: None },
            Kind::IsGd => Shortener::IsGd,
        }
    }
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, links: &mut LinkSettings) {
    let kinds = opts(&[
        (Kind::None, "settings.links.shortener.none"),
        (Kind::Server, "settings.links.shortener.server"),
        (Kind::Yourls, "settings.links.shortener.yourls"),
        (Kind::Shlink, "settings.links.shortener.shlink"),
        (Kind::IsGd, "settings.links.shortener.isgd"),
    ]);
    let formats = link_format_options();
    let expiry = expiry_options(links.expiry_hours);

    rc.section(ui, &s("links.section.copy"), |ui, rc| {
        rc.choice(ui, "links.copy_format", &mut links.copy_format, &formats);
        rc.choice(ui, "links.expiry_hours", &mut links.expiry_hours, &expiry);
        rc.toggle(ui, "links.notify_events", &mut links.notify_events);
    });

    rc.section(ui, &s("links.section.shortener"), |ui, rc| {
        let before = Kind::of(&links.shortener);
        let mut kind = before;
        rc.choice(ui, "links.shortener", &mut kind, &kinds);
        if kind != before {
            links.shortener = kind.fresh();
            st.secret.reset();
        }
        match &mut links.shortener {
            Shortener::Yourls { url, secret_ref } => {
                rc.row(ui, "links.shortener_url", |ui| field_mono(ui, url, "https://short.example.com/yourls-api.php", 320.0));
                st.secret.row(ui, rc, "links.shortener_secret_yourls", cx, secret_ref, &secret_key("shortener", "yourls"));
            }
            Shortener::Shlink { url, secret_ref } => {
                rc.row(ui, "links.shortener_url", |ui| field_mono(ui, url, "https://short.example.com", 320.0));
                st.secret.row(ui, rc, "links.shortener_secret_shlink", cx, secret_ref, &secret_key("shortener", "shlink"));
            }
            Shortener::SnapServer | Shortener::IsGd | Shortener::None => {}
        }
    });
}

/// Never, an hour, a day, a week, a month, a quarter; plus the current value when it is none of those.
fn expiry_options(current: u32) -> Vec<(u32, String)> {
    let mut all = opts(&[
        (0, "settings.links.expiry.never"),
        (1, "settings.links.expiry.hour"),
        (24, "settings.links.expiry.day"),
        (168, "settings.links.expiry.week"),
        (720, "settings.links.expiry.month"),
        (2160, "settings.links.expiry.quarter"),
    ]);
    if !all.iter().any(|(hours, _)| *hours == current) {
        all.push((current, tf("settings.links.expiry.custom", &[("n", &current.to_string())])));
    }
    all
}
