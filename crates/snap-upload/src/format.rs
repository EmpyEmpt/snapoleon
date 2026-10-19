//! Copy-as link formats.

use snap_core::settings::LinkFormat;

use crate::util::html_escape;

/// Format `url` for pasting. With `image` the link is an inline image
/// (`![t](u)`, `<img>`, `[img]u[/img]`), otherwise a hyperlink labelled with
/// `title` (or the URL itself).
pub fn format_link(url: &str, f: LinkFormat, title: Option<&str>, image: bool) -> String {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    match f {
        LinkFormat::Url => url.to_string(),
        LinkFormat::Markdown => {
            let target = markdown_target(url);
            if image {
                format!("![{}]({target})", markdown_text(title.unwrap_or_default()))
            } else {
                format!("[{}]({target})", markdown_text(title.unwrap_or(url)))
            }
        }
        LinkFormat::Html => {
            let href = html_escape(url);
            if image {
                match title {
                    Some(t) => format!("<img src=\"{href}\" alt=\"{}\">", html_escape(t)),
                    None => format!("<img src=\"{href}\" alt=\"\">"),
                }
            } else {
                format!("<a href=\"{href}\">{}</a>", html_escape(title.unwrap_or(url)))
            }
        }
        LinkFormat::BbCode => {
            if image {
                format!("[img]{url}[/img]")
            } else {
                match title {
                    Some(t) => format!("[url={url}]{}[/url]", t.replace('[', "(").replace(']', ")")),
                    None => format!("[url]{url}[/url]"),
                }
            }
        }
    }
}

/// Escape characters that would end Markdown link text early.
fn markdown_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(if c == '\n' { ' ' } else { c });
    }
    out
}

/// Keep spaces and parentheses from breaking the `(...)` destination.
fn markdown_target(url: &str) -> String {
    url.replace(' ', "%20").replace('(', "%28").replace(')', "%29")
}
