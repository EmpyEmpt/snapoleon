//! Tiny i18n: flat TOML string tables.
//!
//! Files live at `locales/<lang>/<area>.toml` and are embedded at build time;
//! users can drop extra files into `paths::user_locales_dir()/<lang>/`.
//! Each file's tables flatten into dotted keys:
//!
//! ```toml
//! # locales/en/editor.toml
//! [editor.tool]
//! arrow = "Arrow"
//! ```
//!
//! gives `t("editor.tool.arrow")`. Placeholders use `{name}`:
//! `tf("history.count", &[("n", "3")])`. Missing keys fall back to English,
//! then to the key itself, so a missing string is visible but never fatal.
//!
//! On macOS a `macos.<key>` entry replaces `<key>` (a `[macos.settings.general]` table overrides
//! `settings.general.*`). It carries the platform's own words, "menu bar" for the tray or "Finder" for the file
//! manager, while the code keeps asking for one key.

use std::{borrow::Cow, collections::HashMap};

use parking_lot::RwLock;

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

struct Catalog {
    lang: String,
    active: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

static CATALOG: RwLock<Option<Catalog>> = RwLock::new(None);

/// Languages with embedded or user-provided tables, as (code, native name).
pub fn available_languages() -> Vec<(String, String)> {
    let mut langs: Vec<String> = EMBEDDED.iter().map(|(l, _)| l.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(crate::paths::user_locales_dir()) {
        langs.extend(rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()));
    }
    langs.sort();
    langs.dedup();
    langs
        .into_iter()
        .map(|l| {
            let name = load(&l).get("meta.language_name").cloned().unwrap_or_else(|| l.clone());
            (l, name)
        })
        .collect()
}

/// Activate a language. Empty = detect from the OS (`LANG`/`LC_ALL` or user default).
pub fn set_language(lang: &str) {
    let lang = if lang.is_empty() { detect_system_language() } else { lang.to_string() };
    let fallback = load("en");
    let active = if lang == "en" { HashMap::new() } else { load(&lang) };
    *CATALOG.write() = Some(Catalog { lang, active, fallback });
}

pub fn current_language() -> String {
    CATALOG.read().as_ref().map(|c| c.lang.clone()).unwrap_or_else(|| "en".into())
}

/// Translate `key`.
pub fn t(key: &str) -> Cow<'static, str> {
    {
        let guard = CATALOG.read();
        if let Some(c) = guard.as_ref() {
            if let Some(v) = c.active.get(key).or_else(|| c.fallback.get(key)) {
                return Cow::Owned(v.clone());
            }
            return Cow::Owned(key.to_string());
        }
    }
    set_language("");
    t(key)
}

/// Translate `key` and substitute `{name}` placeholders.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(key).into_owned();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

static OTHERS: RwLock<Option<HashMap<String, HashMap<String, String>>>> = RwLock::new(None);

/// Translate `key` in a specific language, independent of the active one
/// (the server answers each request in its `Accept-Language`).
pub fn t_in(lang: &str, key: &str) -> String {
    if lang == current_language() {
        return t(key).into_owned();
    }
    let hit = OTHERS.read().as_ref().and_then(|m| m.get(lang)).map(|table| table.get(key).cloned());
    let found = match hit {
        Some(v) => v,
        None => {
            let table = load(lang);
            let v = table.get(key).cloned();
            OTHERS.write().get_or_insert_with(HashMap::new).insert(lang.to_string(), table);
            v
        }
    };
    found.unwrap_or_else(|| t_in_fallback(key))
}

fn t_in_fallback(key: &str) -> String {
    let guard = CATALOG.read();
    match guard.as_ref() {
        Some(c) => c.fallback.get(key).cloned().unwrap_or_else(|| key.to_string()),
        None => {
            drop(guard);
            set_language("");
            t_in_fallback(key)
        }
    }
}

/// [`tf`] for a specific language.
pub fn tf_in(lang: &str, key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t_in(lang, key);
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

fn load(lang: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (l, text) in EMBEDDED {
        if *l == lang {
            merge(&mut map, text);
        }
    }
    let user = crate::paths::user_locales_dir().join(lang);
    if let Ok(rd) = std::fs::read_dir(user) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "toml")
                && let Ok(text) = std::fs::read_to_string(e.path())
            {
                merge(&mut map, &text);
            }
        }
    }
    if cfg!(target_os = "macos") {
        let overrides: Vec<(String, String)> =
            map.iter().filter_map(|(k, v)| k.strip_prefix("macos.").map(|base| (base.to_owned(), v.clone()))).collect();
        map.extend(overrides);
    }
    map
}

fn merge(map: &mut HashMap<String, String>, text: &str) {
    match text.parse::<toml::Table>() {
        Ok(table) => flatten("", &toml::Value::Table(table), map),
        Err(e) => tracing::warn!("bad locale file: {e}"),
    }
}

fn flatten(prefix: &str, v: &toml::Value, out: &mut HashMap<String, String>) {
    match v {
        toml::Value::Table(t) => {
            for (k, v) in t {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn detect_system_language() -> String {
    let from_env = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok()).filter(|s| !s.is_empty() && s != "C");
    let raw = from_env.or_else(os_locale).unwrap_or_else(|| "en".into());
    let code: String = raw.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
    if EMBEDDED.iter().any(|(l, _)| *l == code) { code } else { "en".into() }
}

#[cfg(windows)]
fn os_locale() -> Option<String> {
    // GetUserDefaultLocaleName without pulling the `windows` crate into core.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32;
    }
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

#[cfg(target_os = "macos")]
fn os_locale() -> Option<String> {
    let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn os_locale() -> Option<String> {
    None
}
