//! The self-contained HTML guide: one file, inline CSS, images as data URIs, light and dark themes.

use snap_core::{t, tf};

use super::Guide;

const STYLE: &str = r#"
:root{--bg:#F7F7F5;--surface:#FFFFFF;--text:#17181C;--dim:#62646B;--faint:#A3A49E;--border:#E3E2DC;--accent:#3D5AFE;--shadow:0 8px 24px rgba(0,0,0,.14);color-scheme:light dark}
@media (prefers-color-scheme:dark){:root{--bg:#0E0F13;--surface:#16181E;--text:#ECEDEF;--dim:#9AA0AC;--faint:#5D6270;--border:#2C303B;--accent:#6B7FFF;--shadow:0 8px 24px rgba(0,0,0,.35)}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.6 Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI Variable Text","Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;-webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility}
main{max-width:820px;margin:0 auto;padding:72px 24px 96px}
header{margin-bottom:56px;padding-bottom:40px;border-bottom:1px solid var(--border)}
h1{margin:0 0 18px;font-size:clamp(30px,5.2vw,46px);line-height:1.1;letter-spacing:-.025em;font-weight:700;text-wrap:balance}
.intro{margin:0 0 20px;font-size:19px;line-height:1.55;color:var(--dim);max-width:62ch;text-wrap:pretty}
.meta{display:flex;flex-wrap:wrap;gap:6px 18px;margin:0;padding:0;list-style:none;font:500 12px/1.4 ui-monospace,"JetBrains Mono",SFMono-Regular,Consolas,monospace;color:var(--faint)}
ol.steps{margin:0;padding:0;list-style:none;counter-reset:step}
.step{counter-increment:step;margin:0 0 64px}
.step h2{display:flex;gap:14px;align-items:flex-start;margin:0 0 18px;font-size:21px;line-height:1.38;font-weight:600;letter-spacing:-.011em;text-wrap:balance}
.step h2::before{content:counter(step);flex:none;display:grid;place-items:center;width:32px;height:32px;margin-top:-1px;border-radius:50%;background:var(--accent);color:#fff;font:600 15px/1 Inter,ui-sans-serif,system-ui,sans-serif}
.step figure{margin:0}
.step img{display:block;width:100%;height:auto;border-radius:12px;border:1px solid var(--border);background:var(--surface);box-shadow:var(--shadow)}
footer{padding-top:32px;border-top:1px solid var(--border);color:var(--faint);font-size:13px}
@media (max-width:560px){main{padding:40px 16px 64px}.intro{font-size:17px}.step h2{font-size:18px}}
@media print{:root{--bg:#fff;--surface:#fff;--text:#000;--dim:#333;--border:#ccc;--shadow:none}.step{break-inside:avoid}main{padding:0}}
"#;

/// `images[i]` is the `src` (a data URI) of step `i`.
pub(super) fn render(guide: &Guide, images: &[String]) -> String {
    let title = if guide.title.trim().is_empty() { t("engine.guide.default_title").into_owned() } else { guide.title.trim().to_string() };
    let mut out = String::with_capacity(STYLE.len() + images.iter().map(String::len).sum::<usize>() + 4096);
    out.push_str("<!doctype html>\n");
    out.push_str(&format!("<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n", escape(&snap_core::i18n::current_language())));
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!("<meta name=\"generator\" content=\"{} {}\">\n", snap_core::APP_NAME, snap_core::VERSION));
    out.push_str(&format!("<title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n<header>\n", escape(&title)));
    out.push_str(&format!("<h1>{}</h1>\n", escape(&title)));
    if !guide.intro.trim().is_empty() {
        out.push_str(&format!("<p class=\"intro\">{}</p>\n", escape(guide.intro.trim())));
    }
    let steps = guide.steps.len().to_string();
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    out.push_str(&format!(
        "<ul class=\"meta\"><li>{}</li><li>{}</li></ul>\n</header>\n<ol class=\"steps\">\n",
        escape(&tf("engine.guide.step_count", &[("n", &steps)])),
        date
    ));
    for (step, src) in guide.steps.iter().zip(images) {
        let caption = escape(step.caption.trim());
        out.push_str(&format!(
            "<li class=\"step\">\n<h2>{caption}</h2>\n<figure><img src=\"{src}\" alt=\"{caption}\" width=\"{}\" height=\"{}\" loading=\"lazy\" decoding=\"async\"></figure>\n</li>\n",
            step.image.width(),
            step.image.height()
        ));
    }
    out.push_str(&format!("</ol>\n<footer>{}</footer>\n</main>\n</body>\n</html>\n", escape(&t("engine.guide.footer"))));
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
