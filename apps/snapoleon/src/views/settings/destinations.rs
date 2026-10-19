//! Destinations: the list, the preset picker, one editor per destination kind, secrets, OAuth sign-in and tests.

use egui::{Ui, Widget};
use snap_core::{
    Settings,
    secrets::secret_key,
    settings::{CustomUploader, Destination, DestinationKind},
    tf,
};
use snap_ui::{
    ToastKind, Tone, icon, space, text,
    widgets::{self, ButtonKind, badge},
};
use snap_upload::{
    oauth::{self, AuthorizeOptions, Provider, Tokens},
    presets,
};

use super::kit::{self, Msg, Pick, Rows, SecretSlot, dd, dd_opt, field, field_mono, list_row, managed_banner, opts, s};
use crate::views::ViewCx;

#[derive(Default)]
pub struct State {
    selected: Option<String>,
    adding: bool,
    /// The selected destination's secret (password, token or OAuth tokens).
    secret: SecretSlot,
    /// OAuth client credentials typed by the user for the selected destination.
    client_id: String,
    client_secret: String,
    busy: Option<Busy>,
    /// Outcome of the last connection test.
    test: Option<(String, Result<(), String>)>,
}

enum Busy {
    Testing(String),
    SigningIn(String),
}

impl State {
    pub fn on_msg(&mut self, cx: &mut ViewCx, set: &mut Settings, msg: Msg) -> Option<Msg> {
        match msg {
            Msg::Tested { id, result } => {
                if matches!(&self.busy, Some(Busy::Testing(b)) if *b == id) {
                    self.busy = None;
                }
                self.test = Some((id, result));
            }
            Msg::SignedIn { id, result } => {
                if matches!(&self.busy, Some(Busy::SigningIn(b)) if *b == id) {
                    self.busy = None;
                }
                match result {
                    Ok(()) => {
                        if self.selected.as_deref() == Some(id.as_str()) {
                            self.secret.set_stored(true);
                        }
                        cx.toast(ToastKind::Success, s("destinations.signed_in"));
                    }
                    Err(e) => cx.toast(ToastKind::Error, e),
                }
            }
            Msg::SxcuImported(result) => match result {
                Ok((name, uploader)) => {
                    let id = unique_id(set, &name);
                    set.destinations.push(Destination {
                        id: id.clone(),
                        name,
                        kind: DestinationKind::CustomHttp(uploader),
                        secret_ref: Some(secret_key("destination", &id)),
                    });
                    self.select(cx, set, &id);
                    cx.toast(ToastKind::Success, s("destinations.imported"));
                }
                Err(e) if e.is_empty() => {}
                Err(e) => cx.toast(ToastKind::Error, e),
            },
            Msg::SxcuExported(result) => match result {
                Ok(path) => cx.toast(ToastKind::Success, tf("settings.destinations.exported", &[("path", &path.display().to_string())])),
                Err(e) if e.is_empty() => {}
                Err(e) => cx.toast(ToastKind::Error, e),
            },
            Msg::Picked { target: Pick::LocalDir { dest }, path: Some(path) } => {
                if let Some(Destination { kind: DestinationKind::LocalFolder { dir, .. }, .. }) =
                    set.destinations.iter_mut().find(|d| d.id == dest)
                {
                    *dir = path.display().to_string();
                }
            }
            other => return Some(other),
        }
        None
    }

    /// Open a destination's editor and look up what is already stored for it.
    fn select(&mut self, cx: &mut ViewCx, set: &Settings, id: &str) {
        self.selected = Some(id.to_owned());
        self.adding = false;
        self.secret.reset();
        self.client_secret.clear();
        self.client_id.clear();
        let Some(dest) = set.destination(id) else { return };
        if let Some(stored) = dest.secret_ref.as_deref().and_then(|key| cx.engine.secrets().get(key))
            && let Ok(tokens) = Tokens::from_json(&stored)
        {
            self.client_id = tokens.client_id;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Kinds and presets

fn kind_glyph(kind: &DestinationKind) -> &'static str {
    match kind {
        DestinationKind::S3 { .. } => icon::DATABASE,
        DestinationKind::Ftp { .. } | DestinationKind::WebDav { .. } => icon::HARD_DRIVES,
        DestinationKind::LocalFolder { .. } => icon::FOLDER,
        DestinationKind::Imgur { .. } => icon::IMAGES,
        DestinationKind::Telegram { .. } => icon::TELEGRAM_LOGO,
        DestinationKind::Dropbox { .. } => icon::DROPBOX_LOGO,
        DestinationKind::GoogleDrive { .. } => icon::GOOGLE_DRIVE_LOGO,
        DestinationKind::YandexDisk { .. } | DestinationKind::OneDrive { .. } | DestinationKind::Box { .. } => icon::CLOUD,
        DestinationKind::CustomHttp(_) => icon::CODE,
        DestinationKind::SnapServer { .. } => icon::GLOBE,
    }
}

fn kind_label(kind: &DestinationKind) -> String {
    let key = match kind {
        DestinationKind::S3 { .. } => "s3",
        DestinationKind::Ftp { secure: false, .. } => "ftp",
        DestinationKind::Ftp { secure: true, .. } => "ftps",
        DestinationKind::WebDav { .. } => "webdav",
        DestinationKind::LocalFolder { .. } => "local",
        DestinationKind::Imgur { .. } => "imgur",
        DestinationKind::Telegram { .. } => "telegram",
        DestinationKind::YandexDisk { .. } => "yandex",
        DestinationKind::Dropbox { .. } => "dropbox",
        DestinationKind::GoogleDrive { .. } => "gdrive",
        DestinationKind::OneDrive { .. } => "onedrive",
        DestinationKind::Box { .. } => "box",
        DestinationKind::CustomHttp(_) => "custom",
        DestinationKind::SnapServer { .. } => "server",
    };
    s(&format!("destinations.type.{key}"))
}

/// A short, human line for the list: where the destination points.
fn summary(d: &Destination) -> String {
    let detail = match &d.kind {
        DestinationKind::S3 { bucket, endpoint, .. } => format!("{bucket} · {}", endpoint.trim_start_matches("https://")),
        DestinationKind::Ftp { host, .. } => host.clone(),
        DestinationKind::WebDav { url, .. } | DestinationKind::SnapServer { url } => url.trim_start_matches("https://").to_owned(),
        DestinationKind::LocalFolder { dir, .. } => dir.clone(),
        DestinationKind::CustomHttp(c) => c.request_url.trim_start_matches("https://").to_owned(),
        DestinationKind::Telegram { chat_id } => chat_id.clone(),
        DestinationKind::Dropbox { folder } | DestinationKind::OneDrive { folder } | DestinationKind::YandexDisk { folder, .. } => {
            folder.clone()
        }
        DestinationKind::Imgur { .. } | DestinationKind::GoogleDrive { .. } | DestinationKind::Box { .. } => String::new(),
    };
    let label = kind_label(&d.kind);
    if detail.trim().is_empty() { label } else { format!("{label} · {detail}") }
}

/// What the "add" picker offers.
#[derive(Clone, Copy)]
enum Preset {
    S3(usize),
    Ftp,
    Ftps,
    WebDav,
    Local,
    Imgur,
    Telegram,
    Yandex,
    Dropbox,
    GoogleDrive,
    OneDrive,
    Box,
    Custom,
    Server,
}

impl Preset {
    fn make(self) -> (String, DestinationKind) {
        let named = |key: &str| s(&format!("destinations.type.{key}"));
        match self {
            Preset::S3(i) => {
                let p = &presets::s3()[i];
                (
                    p.name.to_owned(),
                    DestinationKind::S3 {
                        endpoint: p.endpoint.to_owned(),
                        region: p.region.to_owned(),
                        bucket: String::new(),
                        access_key_id: String::new(),
                        path_style: p.path_style,
                        prefix: String::new(),
                        public_base_url: String::new(),
                        random_code_len: 8,
                        acl_public_read: false,
                    },
                )
            }
            Preset::Ftp => (
                named("ftp"),
                DestinationKind::Ftp {
                    host: String::new(),
                    port: 21,
                    user: String::new(),
                    secure: false,
                    dir: String::new(),
                    public_base_url: String::new(),
                },
            ),
            Preset::Ftps => (
                named("ftps"),
                DestinationKind::Ftp {
                    host: String::new(),
                    port: 21,
                    user: String::new(),
                    secure: true,
                    dir: String::new(),
                    public_base_url: String::new(),
                },
            ),
            Preset::WebDav => {
                (named("webdav"), DestinationKind::WebDav { url: String::new(), user: String::new(), public_base_url: String::new() })
            }
            Preset::Local => (named("local"), DestinationKind::LocalFolder { dir: String::new(), public_base_url: String::new() }),
            Preset::Imgur => (named("imgur"), DestinationKind::Imgur { client_id: String::new() }),
            Preset::Telegram => (named("telegram"), DestinationKind::Telegram { chat_id: String::new() }),
            Preset::Yandex => (named("yandex"), DestinationKind::YandexDisk { folder: "Snapoleon".into(), publish: true }),
            Preset::Dropbox => (named("dropbox"), DestinationKind::Dropbox { folder: "/Snapoleon".into() }),
            Preset::GoogleDrive => (named("gdrive"), DestinationKind::GoogleDrive { folder_id: String::new() }),
            Preset::OneDrive => (named("onedrive"), DestinationKind::OneDrive { folder: "Snapoleon".into() }),
            Preset::Box => (named("box"), DestinationKind::Box { folder_id: "0".into(), client_id: String::new() }),
            Preset::Custom => (
                named("custom"),
                DestinationKind::CustomHttp(CustomUploader {
                    request_method: "POST".into(),
                    body: "multipart_form_data".into(),
                    file_form_name: "file".into(),
                    url: "{json:url}".into(),
                    ..CustomUploader::default()
                }),
            ),
            Preset::Server => (named("server"), DestinationKind::SnapServer { url: String::new() }),
        }
    }
}

fn unique_id(set: &Settings, name: &str) -> String {
    kit::unique_slug(name, "destination", |id| set.destinations.iter().any(|d| d.id == id))
}

// ---------------------------------------------------------------------------------------------------------------------
// Pages

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings) {
    let policy = &rc.env.policy;
    let list_locked = policy.destinations.is_some() || policy.disable_uploads;
    let default_locked = policy.default_destination.is_some() || policy.disable_uploads;

    if rc.searching() {
        defaults(ui, rc, set, default_locked);
        return;
    }
    if policy.disable_uploads {
        managed_banner(ui, &s("destinations.uploads_off"));
    } else if policy.destinations.is_some() {
        managed_banner(ui, &s("destinations.managed_list"));
    }
    if st.adding && !list_locked {
        picker(ui, st, cx, set);
        return;
    }
    if let Some(id) = st.selected.clone() {
        if set.destinations.iter().any(|d| d.id == id) {
            editor(ui, rc, st, cx, set, &id, list_locked);
            return;
        }
        st.selected = None;
    }
    overview(ui, rc, st, cx, set, list_locked, default_locked);
}

fn defaults(ui: &mut Ui, rc: &mut Rows, set: &mut Settings, locked: bool) {
    let names: Vec<(String, String)> = set.destinations.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
    rc.section(ui, &s("destinations.section.defaults"), |ui, rc| {
        rc.row_locked(ui, locked, "destinations.default", |ui| {
            dd_opt(ui, "default-destination", &mut set.default_destination, &s("destinations.no_default"), &names);
        });
    });
}

fn overview(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings, list_locked: bool, default_locked: bool) {
    if set.destinations.is_empty() {
        widgets::empty_state_with(ui, icon::CLOUD_ARROW_UP, &s("destinations.empty.title"), &s("destinations.empty.body"), |ui| {
            if !list_locked {
                if widgets::primary_button(ui, &s("destinations.add")).clicked() {
                    st.adding = true;
                }
                if widgets::Button::new(&s("destinations.import")).kind(ButtonKind::Ghost).icon(icon::FILE_ARROW_DOWN).ui(ui).clicked() {
                    import_sxcu(cx);
                }
            }
        });
        ui.add_space(space::XL);
    } else {
        ui.label(text::body_strong(s("destinations.list")));
        ui.add_space(space::XS);
        let mut open = None;
        for d in &set.destinations {
            let is_default = set.default_destination.as_deref() == Some(d.id.as_str());
            let resp = list_row(ui, kind_glyph(&d.kind), &d.name, &summary(d), |ui| {
                if is_default {
                    badge(ui, &s("destinations.default_badge"), Tone::Accent);
                }
            });
            if resp.clicked() {
                open = Some(d.id.clone());
            }
        }
        if let Some(id) = open {
            st.select(cx, set, &id);
        }
        ui.add_space(space::XS);
        if !list_locked {
            ui.horizontal(|ui| {
                if widgets::Button::new(&s("destinations.add")).kind(ButtonKind::Primary).icon(icon::PLUS).ui(ui).clicked() {
                    st.adding = true;
                }
                if widgets::Button::new(&s("destinations.import")).kind(ButtonKind::Secondary).icon(icon::FILE_ARROW_DOWN).ui(ui).clicked()
                {
                    import_sxcu(cx);
                }
            });
        }
        ui.add_space(space::XL);
    }
    defaults(ui, rc, set, default_locked);
}

fn import_sxcu(cx: &mut ViewCx) {
    cx.spawn(|| {
        let picked = rfd::FileDialog::new().add_filter("ShareX custom uploader", &["sxcu", "json"]).pick_file();
        let Some(path) = picked else { return Msg::SxcuImported(Err(String::new())) };
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|json| snap_upload::sxcu::import(&json).map_err(|e| crate::notices::error_text(&e)));
        Msg::SxcuImported(result)
    });
}

/// The grid of things a destination can be.
fn picker(ui: &mut Ui, st: &mut State, cx: &mut ViewCx, set: &mut Settings) {
    if kit::back_header(ui, &s("destinations.back")) {
        st.adding = false;
        return;
    }
    ui.label(text::headline(s("destinations.picker.title")));
    ui.label(text::dim(s("destinations.picker.body")));
    ui.add_space(space::XL);

    let s3: Vec<(Preset, &str)> = (0..presets::s3().len()).map(|i| (Preset::S3(i), icon::DATABASE)).collect();
    let groups: [(&str, Vec<(Preset, &str)>); 5] = [
        ("destinations.group.s3", s3),
        (
            "destinations.group.servers",
            vec![
                (Preset::Server, icon::GLOBE),
                (Preset::Ftp, icon::HARD_DRIVES),
                (Preset::Ftps, icon::HARD_DRIVES),
                (Preset::WebDav, icon::HARD_DRIVES),
                (Preset::Local, icon::FOLDER),
            ],
        ),
        (
            "destinations.group.cloud",
            vec![
                (Preset::Dropbox, icon::DROPBOX_LOGO),
                (Preset::GoogleDrive, icon::GOOGLE_DRIVE_LOGO),
                (Preset::OneDrive, icon::CLOUD),
                (Preset::Yandex, icon::CLOUD),
                (Preset::Box, icon::CLOUD),
            ],
        ),
        ("destinations.group.services", vec![(Preset::Imgur, icon::IMAGES), (Preset::Telegram, icon::TELEGRAM_LOGO)]),
        ("destinations.group.custom", vec![(Preset::Custom, icon::CODE)]),
    ];

    let mut chosen = None;
    for (title, items) in groups {
        ui.label(text::body_strong(s(title)));
        ui.add_space(space::XS);
        ui.columns(2, |cols| {
            for (n, (preset, glyph)) in items.iter().enumerate() {
                let (name, kind) = preset.make();
                let subtitle = match preset {
                    Preset::S3(i) => presets::s3()[*i].region,
                    _ => "",
                };
                if list_row(&mut cols[n % 2], glyph, &name, subtitle, |_| {}).clicked() {
                    chosen = Some((name, kind));
                }
            }
        });
        ui.add_space(space::LG);
    }
    if let Some((name, kind)) = chosen {
        let id = unique_id(set, &name);
        let secret_ref = needs_secret(&kind).then(|| secret_key("destination", &id));
        set.destinations.push(Destination { id: id.clone(), name, kind, secret_ref });
        if set.default_destination.is_none() {
            set.default_destination = Some(id.clone());
        }
        st.select(cx, set, &id);
    }
}

fn needs_secret(kind: &DestinationKind) -> bool {
    !matches!(kind, DestinationKind::LocalFolder { .. } | DestinationKind::Imgur { .. })
}

// ---------------------------------------------------------------------------------------------------------------------
// One destination

fn editor(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings, id: &str, locked: bool) {
    if kit::back_header(ui, &s("destinations.back")) {
        st.selected = None;
        return;
    }
    let Some(index) = set.destinations.iter().position(|d| d.id == id) else { return };
    let is_default = set.default_destination.as_deref() == Some(id);
    let default_locked = rc.env.policy.default_destination.is_some();
    let mut make_default = false;
    let mut delete = false;
    let mut test = false;

    {
        let dest = &mut set.destinations[index];
        let glyph = kind_glyph(&dest.kind);
        let type_label = kind_label(&dest.kind);
        ui.horizontal(|ui| {
            ui.label(text::headline(glyph.to_string()));
            ui.vertical(|ui| {
                ui.label(text::headline(dest.name.as_str()));
                ui.label(text::dim(type_label));
            });
        });
        ui.add_space(space::LG);

        rc.section(ui, &s("destinations.section.general"), |ui, rc| {
            rc.row_locked(ui, locked, "destinations.f.name", |ui| field(ui, &mut dest.name, "", 280.0));
        });

        let title = s("destinations.section.connection");
        ui.add_enabled_ui(!locked, |ui| connection(ui, rc, &title, st, cx, dest));

        rc.section(ui, &s("destinations.section.actions"), |ui, rc| {
            rc.row(ui, "destinations.test", |ui| {
                let testing = matches!(&st.busy, Some(Busy::Testing(b)) if b == id);
                if testing {
                    widgets::spinner(ui);
                    ui.label(text::dim(s("destinations.testing")));
                } else {
                    if widgets::secondary_button(ui, &s("destinations.test.button")).clicked() {
                        test = true;
                    }
                    match &st.test {
                        Some((tested, Ok(()))) if tested == id => {
                            badge(ui, &s("destinations.test.ok"), Tone::Success);
                        }
                        Some((tested, Err(e))) if tested == id => {
                            badge(ui, &s("destinations.test.failed"), Tone::Danger).on_hover_text(e);
                        }
                        _ => {}
                    }
                }
            });
            if let Some((tested, Err(e))) = &st.test
                && tested == id
            {
                rc.content(ui, |ui| {
                    ui.add(egui::Label::new(text::dim(e.as_str())).wrap());
                });
            }
            rc.row_locked(ui, default_locked && !is_default, "destinations.make_default", |ui| {
                if is_default {
                    badge(ui, &s("destinations.default_badge"), Tone::Accent);
                } else if widgets::secondary_button(ui, &s("destinations.make_default.button")).clicked() {
                    make_default = true;
                }
            });
            if let DestinationKind::CustomHttp(custom) = &dest.kind {
                let name = dest.name.clone();
                rc.row(ui, "destinations.export", |ui| {
                    if widgets::secondary_button(ui, &s("destinations.export.button")).clicked() {
                        export_sxcu(cx, name, custom.clone());
                    }
                });
            }
            if !locked {
                rc.row(ui, "destinations.delete", |ui| {
                    if kit::confirm_button(
                        ui,
                        ("delete-destination", id),
                        &s("destinations.delete.button"),
                        &s("destinations.delete.confirm"),
                    ) {
                        delete = true;
                    }
                });
            }
        });
    }

    if test {
        st.busy = Some(Busy::Testing(id.to_owned()));
        st.test = None;
        let dest = set.destinations[index].clone();
        let secrets = cx.engine.secrets();
        cx.spawn(move || Msg::Tested {
            id: dest.id.clone(),
            result: snap_upload::uploader_for(&dest, &*secrets).and_then(|u| u.test()).map_err(|e| crate::notices::error_text(&e)),
        });
    }
    if make_default {
        set.default_destination = Some(id.to_owned());
    }
    if delete {
        let removed = set.destinations.remove(index);
        if let Some(key) = &removed.secret_ref {
            let _ = cx.engine.secrets().delete(key);
        }
        if set.default_destination.as_deref() == Some(id) {
            set.default_destination = set.destinations.first().map(|d| d.id.clone());
        }
        for step in set.pipelines.iter_mut().flat_map(|p| p.steps.iter_mut()) {
            if let snap_core::pipeline::Step::Upload { destination } = step
                && destination.as_deref() == Some(id)
            {
                *destination = None;
            }
        }
        st.selected = None;
    }
}

fn export_sxcu(cx: &mut ViewCx, name: String, uploader: CustomUploader) {
    cx.spawn(move || {
        let Some(path) =
            rfd::FileDialog::new().set_file_name(format!("{name}.sxcu")).add_filter("ShareX custom uploader", &["sxcu"]).save_file()
        else {
            return Msg::SxcuExported(Err(String::new()));
        };
        let json = snap_upload::sxcu::export(&name, &uploader);
        Msg::SxcuExported(std::fs::write(&path, json).map(|()| path).map_err(|e| e.to_string()))
    });
}

/// The kind-specific sections of a destination.
fn connection(ui: &mut Ui, rc: &mut Rows, title: &str, st: &mut State, cx: &mut ViewCx, dest: &mut Destination) {
    let Destination { id, secret_ref, kind, .. } = dest;
    match kind {
        DestinationKind::S3 {
            endpoint,
            region,
            bucket,
            access_key_id,
            path_style,
            prefix,
            public_base_url,
            random_code_len,
            acl_public_read,
        } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.endpoint", endpoint, "https://s3.example.com", true, 320.0);
                if endpoint.contains("<account-id>") {
                    rc.content(ui, |ui| kit::note(ui, &s("destinations.f.endpoint.account_id")));
                }
                rc.text(ui, "destinations.f.region", region, "us-east-1", true, 200.0);
                rc.text(ui, "destinations.f.bucket", bucket, "", true, 240.0);
                rc.toggle(ui, "destinations.f.path_style", path_style);
                rc.text(ui, "destinations.f.prefix", prefix, "screenshots/", true, 240.0);
                rc.text(ui, "destinations.f.public_base_url", public_base_url, "https://img.example.com", true, 320.0);
                rc.number(ui, "destinations.f.random_code_len", random_code_len, 0..=32, "");
                rc.toggle(ui, "destinations.f.acl_public_read", acl_public_read);
            });
            rc.section(ui, &s("destinations.section.access"), |ui, rc| {
                rc.text(ui, "destinations.f.access_key_id", access_key_id, "", true, 260.0);
                secret_row(ui, rc, "destinations.f.s3_secret", st, cx, (id, secret_ref), None);
            });
        }
        DestinationKind::Ftp { host, port, user, secure, dir, public_base_url } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.host", host, "ftp.example.com", true, 260.0);
                rc.number(ui, "destinations.f.port", port, 1..=65535, "");
                rc.toggle(ui, "destinations.f.secure", secure);
                rc.text(ui, "destinations.f.dir", dir, "/screenshots", true, 260.0);
                rc.text(ui, "destinations.f.public_base_url", public_base_url, "https://img.example.com/screenshots", true, 320.0);
            });
            rc.section(ui, &s("destinations.section.access"), |ui, rc| {
                rc.text(ui, "destinations.f.user", user, "", false, 220.0);
                secret_row(ui, rc, "destinations.f.password", st, cx, (id, secret_ref), None);
            });
        }
        DestinationKind::WebDav { url, user, public_base_url } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.webdav_url", url, "https://cloud.example.com/remote.php/dav/files/me/", true, 340.0);
                rc.text(ui, "destinations.f.public_base_url", public_base_url, "https://img.example.com", true, 320.0);
            });
            rc.section(ui, &s("destinations.section.access"), |ui, rc| {
                rc.text(ui, "destinations.f.user", user, "", false, 220.0);
                secret_row(ui, rc, "destinations.f.password", st, cx, (id, secret_ref), None);
            });
        }
        DestinationKind::LocalFolder { dir, public_base_url } => {
            rc.section(ui, title, |ui, rc| {
                rc.row(ui, "destinations.f.local_dir", |ui| {
                    kit::path_field(ui, cx, dir, "", Pick::LocalDir { dest: id.clone() }, 320.0);
                });
                rc.text(ui, "destinations.f.public_base_url", public_base_url, "https://img.example.com", true, 320.0);
            });
        }
        DestinationKind::Imgur { client_id } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.imgur_client", client_id, "", true, 260.0);
            });
        }
        DestinationKind::Telegram { chat_id } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.chat_id", chat_id, "@channel", true, 220.0);
            });
            rc.section(ui, &s("destinations.section.access"), |ui, rc| {
                secret_row(ui, rc, "destinations.f.bot_token", st, cx, (id, secret_ref), None);
            });
        }
        DestinationKind::SnapServer { url } => {
            let server = url.clone();
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.server_url", url, "https://snap.example.com", true, 320.0);
            });
            rc.section(ui, &s("destinations.section.access"), |ui, rc| {
                // The shortener looks the same token up under a key derived from the server address.
                let shortener_key = snap_upload::snap_server_secret_key(&server);
                secret_row(ui, rc, "destinations.f.server_token", st, cx, (id, secret_ref), Some(shortener_key));
            });
        }
        DestinationKind::CustomHttp(custom) => custom_http(ui, rc, title, st, cx, (id, secret_ref), custom),
        DestinationKind::YandexDisk { folder, publish } => {
            rc.section(ui, title, |ui, rc| {
                rc.text(ui, "destinations.f.folder", folder, "Snapoleon", true, 240.0);
                rc.toggle(ui, "destinations.f.publish", publish);
            });
            oauth_section(ui, rc, st, cx, (id, secret_ref), Provider::YandexDisk, None);
        }
        DestinationKind::Dropbox { folder } => {
            rc.section(ui, title, |ui, rc| rc.text(ui, "destinations.f.folder", folder, "/Snapoleon", true, 240.0));
            oauth_section(ui, rc, st, cx, (id, secret_ref), Provider::Dropbox, None);
        }
        DestinationKind::GoogleDrive { folder_id } => {
            rc.section(ui, title, |ui, rc| rc.text(ui, "destinations.f.folder_id", folder_id, "", true, 280.0));
            oauth_section(ui, rc, st, cx, (id, secret_ref), Provider::GoogleDrive, None);
        }
        DestinationKind::OneDrive { folder } => {
            rc.section(ui, title, |ui, rc| rc.text(ui, "destinations.f.folder", folder, "Snapoleon", true, 240.0));
            oauth_section(ui, rc, st, cx, (id, secret_ref), Provider::OneDrive, None);
        }
        DestinationKind::Box { folder_id, client_id } => {
            rc.section(ui, title, |ui, rc| rc.text(ui, "destinations.f.folder_id", folder_id, "0", true, 280.0));
            oauth_section(ui, rc, st, cx, (id, secret_ref), Provider::Box, Some(client_id));
        }
    }
}

/// A password-style row backed by the secret store. `mirror` is a second key the same value is written to.
fn secret_row(
    ui: &mut Ui,
    rc: &mut Rows,
    key: &str,
    st: &mut State,
    cx: &mut ViewCx,
    (id, secret_ref): (&str, &mut Option<String>),
    mirror: Option<String>,
) {
    st.secret.mirror = mirror;
    st.secret.row(ui, rc, key, cx, secret_ref, &secret_key("destination", id));
}

/// Client id, optional client secret, sign-in and sign-out.
fn oauth_section(
    ui: &mut Ui,
    rc: &mut Rows,
    st: &mut State,
    cx: &mut ViewCx,
    (id, secret_ref): (&str, &mut Option<String>),
    provider: Provider,
    mut box_client_id: Option<&mut String>,
) {
    rc.section(ui, &s("destinations.section.account"), |ui, rc| {
        let busy = matches!(&st.busy, Some(Busy::SigningIn(b)) if b == id);
        rc.row(ui, "destinations.f.oauth_client", |ui| match box_client_id.as_deref_mut() {
            Some(value) => field_mono(ui, value, "", 260.0),
            None => field_mono(ui, &mut st.client_id, "", 260.0),
        });
        rc.row(ui, "destinations.f.oauth_secret", |ui| {
            ui.add(
                widgets::TextField::new(&mut st.client_secret)
                    .password(true)
                    .mono(true)
                    .hint(&s("destinations.f.oauth_secret.hint"))
                    .width(260.0),
            );
        });
        rc.row(ui, "destinations.f.oauth_account", |ui| {
            if busy {
                widgets::spinner(ui);
                ui.label(text::dim(s("destinations.waiting_browser")));
                return;
            }
            if st.secret.is_stored(cx, secret_ref) {
                if widgets::ghost_button(ui, &s("destinations.sign_out")).clicked() {
                    st.secret.remove(cx, secret_ref);
                }
                badge(ui, &s("destinations.signed_in_badge"), Tone::Success);
            } else if widgets::Button::new(&tf("settings.destinations.sign_in", &[("provider", provider.name())]))
                .kind(ButtonKind::Primary)
                .icon(icon::SIGN_IN)
                .ui(ui)
                .clicked()
            {
                let client_id = box_client_id.as_deref().cloned().unwrap_or_else(|| st.client_id.clone());
                let client_secret = Some(st.client_secret.trim().to_owned()).filter(|v| !v.is_empty());
                let name = secret_ref.get_or_insert_with(|| secret_key("destination", id)).clone();
                let secrets = cx.engine.secrets();
                let dest_id = id.to_owned();
                st.busy = Some(Busy::SigningIn(dest_id.clone()));
                cx.spawn(move || {
                    let result = oauth::authorize_with(provider, client_id.trim(), client_secret.as_deref(), &AuthorizeOptions::default())
                        .and_then(|tokens| secrets.set(&name, &tokens.to_json()))
                        .map_err(|e| crate::notices::error_text(&e));
                    Msg::SignedIn { id: dest_id, result }
                });
            }
        });
    });
}

/// The ShareX-compatible custom HTTP uploader.
fn custom_http(
    ui: &mut Ui,
    rc: &mut Rows,
    title: &str,
    st: &mut State,
    cx: &mut ViewCx,
    target: (&str, &mut Option<String>),
    c: &mut CustomUploader,
) {
    let methods = opts(&[
        ("POST".to_owned(), "settings.destinations.method.post"),
        ("PUT".to_owned(), "settings.destinations.method.put"),
        ("PATCH".to_owned(), "settings.destinations.method.patch"),
    ]);
    let bodies = opts(&[
        ("multipart_form_data".to_owned(), "settings.destinations.body.multipart"),
        ("binary".to_owned(), "settings.destinations.body.binary"),
        ("form_url_encoded".to_owned(), "settings.destinations.body.form"),
        ("json".to_owned(), "settings.destinations.body.json"),
        ("xml".to_owned(), "settings.destinations.body.xml"),
        ("none".to_owned(), "settings.destinations.body.none"),
    ]);
    if c.request_method.is_empty() {
        c.request_method = "POST".into();
    }
    if c.body.is_empty() {
        c.body = "multipart_form_data".into();
    }
    rc.section(ui, title, |ui, rc| {
        rc.row(ui, "destinations.f.request", |ui| {
            field_mono(ui, &mut c.request_url, "https://example.com/upload", 280.0);
            dd(ui, "custom-method", &mut c.request_method, &methods);
        });
        rc.choice(ui, "destinations.f.body", &mut c.body, &bodies);
        if c.body == "multipart_form_data" {
            rc.text(ui, "destinations.f.file_form_name", &mut c.file_form_name, "file", true, 160.0);
        }
        rc.stacked(ui, false, "destinations.f.arguments", |ui| kv_editor(ui, "custom-arguments", &mut c.arguments));
        if c.body == "json" || c.body == "xml" {
            rc.stacked(ui, false, "destinations.f.data", |ui| {
                let width = ui.available_width();
                ui.add(widgets::TextField::new(&mut c.data).mono(true).rows(5).width(width));
            });
        }
        rc.stacked(ui, false, "destinations.f.headers", |ui| kv_editor(ui, "custom-headers", &mut c.headers));
        rc.stacked(ui, false, "destinations.f.parameters", |ui| kv_editor(ui, "custom-parameters", &mut c.parameters));
    });
    rc.section(ui, &s("destinations.section.response"), |ui, rc| {
        rc.text(ui, "destinations.f.response_url", &mut c.url, "{json:data.link}", true, 300.0);
        rc.text(ui, "destinations.f.thumbnail_url", &mut c.thumbnail_url, "", true, 300.0);
        rc.text(ui, "destinations.f.deletion_url", &mut c.deletion_url, "", true, 300.0);
        rc.text(ui, "destinations.f.error_message", &mut c.error_message, "{json:error}", true, 300.0);
    });
    rc.section(ui, &s("destinations.section.access"), |ui, rc| {
        secret_row(ui, rc, "destinations.f.custom_secret", st, cx, (target.0, &mut *target.1), None);
    });
}

/// Editable list of name and value pairs.
fn kv_editor(ui: &mut Ui, id: &str, pairs: &mut Vec<(String, String)>) {
    let mut remove = None;
    for (i, (name, value)) in pairs.iter_mut().enumerate() {
        ui.push_id((id, i), |ui| {
            ui.horizontal(|ui| {
                field_mono(ui, name, &s("destinations.kv.name"), 150.0);
                field_mono(ui, value, &s("destinations.kv.value"), 220.0);
                if widgets::icon_button(ui, icon::X, &s("destinations.kv.remove"), None).clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        pairs.remove(i);
    }
    if widgets::Button::new(&s("destinations.kv.add")).kind(ButtonKind::Ghost).icon(icon::PLUS).ui(ui).clicked() {
        pairs.push((String::new(), String::new()));
    }
}
