//! Per-request language. A middleware picks the best match from
//! `Accept-Language` (falling back to the configured language) and scopes it
//! over the handler; `t`/`tf` read it. Background tasks (webhooks, sweeper)
//! run outside any request and use the configured language.

use std::borrow::Cow;

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};
use snap_core::i18n;

tokio::task_local! {
    static LANG: String;
}

/// The language of the current request (or the configured one).
pub fn lang() -> String {
    LANG.try_with(Clone::clone).unwrap_or_else(|_| i18n::current_language())
}

pub fn t(key: &str) -> Cow<'static, str> {
    match LANG.try_with(Clone::clone) {
        Ok(l) => Cow::Owned(i18n::t_in(&l, key)),
        Err(_) => i18n::t(key),
    }
}

pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    match LANG.try_with(Clone::clone) {
        Ok(l) => i18n::tf_in(&l, key, args),
        Err(_) => i18n::tf(key, args),
    }
}

pub async fn negotiate(req: Request, next: Next) -> Response {
    let header = req.headers().get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let lang = pick(header).unwrap_or_else(i18n::current_language);
    let mut res = LANG.scope(lang, next.run(req)).await;
    res.headers_mut().append(header::VARY, HeaderValue::from_static("Accept-Language"));
    res
}

/// Highest-q language from `Accept-Language` that we ship.
fn pick(header: &str) -> Option<String> {
    let available: Vec<String> = i18n::available_languages().into_iter().map(|(code, _)| code).collect();
    let mut prefs: Vec<(f32, String)> = header
        .split(',')
        .filter_map(|part| {
            let mut it = part.trim().split(';');
            let tag = it.next()?.trim();
            let q = it.find_map(|p| p.trim().strip_prefix("q=")).and_then(|q| q.parse().ok()).unwrap_or(1.0);
            let primary = tag.split('-').next()?.to_ascii_lowercase();
            (q > 0.0 && !primary.is_empty() && primary != "*").then_some((q, primary))
        })
        .collect();
    prefs.sort_by(|a, b| b.0.total_cmp(&a.0));
    prefs.into_iter().map(|(_, l)| l).find(|l| available.contains(l))
}
