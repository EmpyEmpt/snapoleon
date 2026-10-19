//! Server-rendered pages shared by the public viewer and the admin UI.

use crate::i18n::{lang as current_language, t, tf};
use axum::http::StatusCode;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::{
    db::{Comment, Item},
    util::{format_ms, human_bytes, iso_ms},
};

pub const STYLE_PATH: &str = "/static/style.css";
pub const SCRIPT_PATH: &str = "/static/app.js";
/// Cache-busting query for static assets.
const ASSET_VERSION: &str = env!("CARGO_PKG_VERSION");

const LOGO_SVG: &str = r##"<svg class="logo" viewBox="0 0 32 32" width="22" height="22" aria-hidden="true"><rect width="32" height="32" rx="8" fill="var(--accent)"/><path d="M9 14V9h5M18 9h5v5M23 18v5h-5M14 23H9v-5" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

pub fn logo() -> Markup {
    PreEscaped(LOGO_SVG.to_string())
}

/// Document shell with theme-aware styles; `head` carries page-specific meta tags.
pub fn shell(title: &str, head: Markup, body_class: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(current_language()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                title { (title) }
                link rel="icon" type="image/svg+xml" href="/static/favicon.svg";
                link rel="stylesheet" href=(format!("{STYLE_PATH}?v={ASSET_VERSION}"));
                (head)
            }
            body class=(body_class) {
                (body)
                script src=(format!("{SCRIPT_PATH}?v={ASSET_VERSION}")) defer {}
            }
        }
    }
}

pub fn brand_link() -> Markup {
    html! { a.brand href="/" { (logo()) span { "Snapoleon" } } }
}

pub fn error_page(status: StatusCode, message: &str) -> Markup {
    let heading = status.canonical_reason().unwrap_or("Error");
    shell(
        &format!("{} - Snapoleon", status.as_u16()),
        html! { meta name="robots" content="noindex"; },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                p.code-large { (status.as_u16()) }
                h1 { (heading) }
                p.dim { (message) }
                p { a.button.secondary href="/" { (t("server.common.home")) } }
            }
        },
    )
}

pub fn home_page(site_url: &str) -> Markup {
    shell(
        "Snapoleon",
        html! {
            meta name="description" content=(t("server.home.tagline"));
            meta property="og:site_name" content="Snapoleon";
            meta property="og:title" content=(t("server.home.title"));
            meta property="og:description" content=(t("server.home.tagline"));
            meta property="og:image" content=(format!("{site_url}/og.png"));
            meta name="twitter:card" content="summary_large_image";
        },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                h1 { (t("server.home.title")) }
                p.dim { (t("server.home.tagline")) }
                p { a.button href="/admin" { (t("server.home.open_admin")) } }
            }
        },
    )
}

pub struct Links {
    pub page: String,
    pub raw: String,
    pub og: String,
    pub thumb: Option<String>,
}

pub struct ViewerState<'a> {
    pub item: &'a Item,
    pub links: &'a Links,
    pub comments: &'a [Comment],
    pub unlocked: bool,
    pub wrong_password: bool,
    pub comment_error: Option<String>,
    pub comment_sent: bool,
}

pub fn viewer_page(v: &ViewerState) -> Markup {
    let item = v.item;
    let locked = item.is_protected() && !v.unlocked;
    let title = if locked { t("server.og.protected").into_owned() } else { item.display_title().to_string() };
    let desc = if locked {
        t("server.viewer.desc_protected").into_owned()
    } else if item.is_video() {
        tf("server.viewer.desc_video", &[("name", &item.uploader)])
    } else {
        tf("server.viewer.desc_image", &[("name", &item.uploader)])
    };
    let head = html! {
        meta name="description" content=(desc);
        meta name="robots" content="noindex, nofollow";
        link rel="canonical" href=(v.links.page);
        meta property="og:site_name" content="Snapoleon";
        meta property="og:title" content=(title);
        meta property="og:description" content=(desc);
        meta property="og:url" content=(v.links.page);
        @if item.is_video() && !item.is_protected() {
            meta property="og:type" content="video.other";
            meta property="og:image" content=(v.links.og);
            meta property="og:image:type" content="image/png";
            meta property="og:image:width" content="1200";
            meta property="og:image:height" content="630";
            meta property="og:video" content=(v.links.raw);
            meta property="og:video:url" content=(v.links.raw);
            meta property="og:video:secure_url" content=(v.links.raw);
            meta property="og:video:type" content=(item.mime);
            @if item.width > 0 && item.height > 0 {
                meta property="og:video:width" content=(item.width);
                meta property="og:video:height" content=(item.height);
            }
        } @else {
            meta property="og:type" content="website";
            meta property="og:image" content=(v.links.og);
            meta property="og:image:type" content="image/png";
            meta property="og:image:width" content="1200";
            meta property="og:image:height" content="630";
            meta property="og:image:alt" content=(title);
        }
        meta name="twitter:card" content="summary_large_image";
        meta name="twitter:title" content=(title);
        meta name="twitter:description" content=(desc);
        meta name="twitter:image" content=(v.links.og);
    };
    let body = html! {
        header.bar {
            (brand_link())
            @if !locked {
                div.actions {
                    button.button.secondary type="button" data-copy=(v.links.page) data-done=(t("server.viewer.copied")) { (t("server.viewer.copy_link")) }
                    a.button.secondary href=(format!("{}?download=1", v.links.raw)) { (t("server.viewer.download")) }
                }
            }
        }
        @if locked {
            (unlock_form(&item.code, v.wrong_password))
        } @else {
            main.stage {
                @if item.is_video() {
                    video controls preload="metadata" playsinline poster=[v.links.thumb.as_ref()] src=(v.links.raw) {}
                } @else {
                    a href=(v.links.raw) { img src=(v.links.raw) alt=(item.display_title()) width=[(item.width > 0).then_some(item.width)] height=[(item.height > 0).then_some(item.height)]; }
                }
            }
            section.meta {
                h1 { (item.display_title()) }
                p.dim.facts {
                    span { (tf("server.viewer.shared_by", &[("name", &item.uploader)])) }
                    span { time datetime=(iso_ms(item.created_ms)) { (format_ms(item.created_ms)) " UTC" } }
                    span { (human_bytes(item.bytes.max(0) as u64)) }
                    @if item.width > 0 { span.mono { (item.width) " × " (item.height) } }
                    span { (t("server.viewer.views_label")) " " (item.views) }
                    @if let Some(exp) = item.expires_ms {
                        span { (tf("server.viewer.expires", &[("when", &format_ms(exp))])) }
                    }
                }
            }
            (comments_section(v))
        }
        footer.foot { (t("server.viewer.footer")) }
    };
    shell(&format!("{title} - Snapoleon"), head, "viewer", body)
}

fn unlock_form(code: &str, wrong: bool) -> Markup {
    html! {
        main.card.narrow {
            h1 { (t("server.viewer.protected_title")) }
            p.dim { (t("server.viewer.protected_hint")) }
            form method="post" action=(format!("/{code}/unlock")) {
                @if wrong { p.notice.error { (t("server.viewer.wrong_password")) } }
                label { span { (t("server.viewer.password_label")) }
                    input type="password" name="password" required autofocus autocomplete="current-password";
                }
                button.button type="submit" { (t("server.viewer.unlock")) }
            }
        }
    }
}

fn comments_section(v: &ViewerState) -> Markup {
    html! {
        section.comments #comments {
            h2 { (t("server.viewer.comments")) @if !v.comments.is_empty() { " " span.dim { (v.comments.len()) } } }
            @if v.comments.is_empty() {
                p.dim { (t("server.viewer.no_comments")) }
            }
            @for c in v.comments {
                article.comment {
                    header { strong { (c.name) } " " time.dim datetime=(iso_ms(c.created_ms)) { (format_ms(c.created_ms)) } }
                    p { (c.text) }
                }
            }
            form.comment-form method="post" action=(format!("/{}/comment", v.item.code)) {
                @if v.comment_sent { p.notice.ok { (t("server.viewer.comment_sent")) } }
                @if let Some(err) = &v.comment_error { p.notice.error { (err) } }
                label { span { (t("server.viewer.name_label")) } input type="text" name="name" maxlength="60" required autocomplete="nickname"; }
                label { span { (t("server.viewer.comment_label")) } textarea name="text" rows="3" maxlength="2000" required {} }
                // Honeypot for naive bots; hidden from people and assistive tech.
                div.hp aria-hidden="true" { input type="text" name="website" tabindex="-1" autocomplete="off"; }
                button.button type="submit" { (t("server.viewer.comment_submit")) }
            }
        }
    }
}

pub fn delete_confirm_page(item: &Item) -> Markup {
    shell(
        &format!("{} - Snapoleon", t("server.delete.title")),
        html! { meta name="robots" content="noindex"; },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                h1 { (t("server.delete.title")) }
                p.dim { (tf("server.delete.body", &[("title", item.display_title())])) }
                form method="post" {
                    button.button.danger type="submit" { (t("server.delete.confirm")) }
                    " "
                    a.button.secondary href=(format!("/{}", item.code)) { (t("server.delete.cancel")) }
                }
            }
        },
    )
}

pub fn deleted_page() -> Markup {
    shell(
        &format!("{} - Snapoleon", t("server.delete.done")),
        html! { meta name="robots" content="noindex"; },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                h1 { (t("server.delete.done")) }
                p { a.button.secondary href="/" { (t("server.common.home")) } }
            }
        },
    )
}
