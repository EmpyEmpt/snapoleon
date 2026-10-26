//! Text formatting for the Library: relative times, dates, sizes and plural forms.

use chrono::{DateTime, Datelike, Local, TimeZone};
use snap_core::{i18n, t, tf};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Plural category of `n` in the active language: `one`, `few`, `many` or `other`.
pub fn plural(n: u64) -> &'static str {
    let lang = i18n::current_language();
    match lang.split(['-', '_']).next().unwrap_or("en") {
        "ru" | "uk" | "be" => {
            let (m10, m100) = (n % 10, n % 100);
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// `"{n} items"` in the right plural form; `base` is a locale key prefix with `one/few/many/other` children.
pub fn counted(base: &str, n: u64) -> String {
    tf(&format!("{base}.{}", plural(n)), &[("n", &n.to_string())])
}

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// "just now", "2 min ago", "3 h ago", "yesterday", "5 days ago", then a short date.
pub fn relative(now_ms: i64, then_ms: i64) -> String {
    let diff = (now_ms - then_ms).max(0);
    if diff < 45_000 {
        return t("library.time.now").into_owned();
    }
    if diff < HOUR {
        return tf("library.time.minutes", &[("n", &(diff / MINUTE).max(1).to_string())]);
    }
    if diff < DAY {
        return tf("library.time.hours", &[("n", &(diff / HOUR).to_string())]);
    }
    let days = (diff / DAY) as u64;
    match days {
        1 => t("library.time.yesterday").into_owned(),
        2..=6 => counted("library.time.days", days),
        _ => short_date(then_ms, now_ms),
    }
}

fn month_name(month: u32) -> String {
    const KEYS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    t(&format!("library.month.{}", KEYS[(month as usize).saturating_sub(1).min(11)])).into_owned()
}

/// "12 Oct", or "12 Oct 2025" when the year differs from today's.
pub fn short_date(ms: i64, now_ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    let same_year = local(now_ms).is_some_and(|n| n.year() == d.year());
    let (day, month, year) = (d.day().to_string(), month_name(d.month()), d.year().to_string());
    if same_year {
        tf("library.date.short", &[("day", &day), ("month", &month)])
    } else {
        tf("library.date.short_year", &[("day", &day), ("month", &month), ("year", &year)])
    }
}

/// "12 Oct 2026, 14:03".
pub fn date_time(ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    tf(
        "library.date.full",
        &[
            ("day", &d.day().to_string()),
            ("month", &month_name(d.month())),
            ("year", &d.year().to_string()),
            ("time", &d.format("%H:%M").to_string()),
        ],
    )
}

/// Local midnight of `ms`'s day, in epoch milliseconds.
pub fn day_start(ms: i64) -> i64 {
    local(ms)
        .and_then(|d| d.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|n| Local.from_local_datetime(&n).earliest())
        .map_or(ms - ms.rem_euclid(DAY), |d| d.timestamp_millis())
}

/// Local midnight of the Monday of `ms`'s week.
pub fn week_start(ms: i64) -> i64 {
    let back = local(ms).map_or(0, |d| i64::from(d.weekday().num_days_from_monday()));
    day_start(day_start(ms) - back * DAY + HOUR * 12)
}

/// Local midnight of the first day of `ms`'s month.
pub fn month_start(ms: i64) -> i64 {
    let back = local(ms).map_or(0, |d| i64::from(d.day() - 1));
    day_start(day_start(ms) - back * DAY + HOUR * 12)
}

/// "0:07", "12:34", "1:02:03".
pub fn duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}
