//! Pure display helpers, unit-testable off the DOM.

/// A relative time like "3 h ago" against the given "now" (unix seconds).
#[must_use]
pub fn human_time(unix_secs: i64, now_secs: i64) -> String {
    if unix_secs <= 0 {
        return "unknown".into();
    }
    let delta = now_secs - unix_secs;
    if delta < 0 {
        return "just now".into();
    }
    match delta {
        0..=59 => "just now".into(),
        60..=3_599 => format!("{} min ago", delta / 60),
        3_600..=86_399 => format!("{} h ago", delta / 3_600),
        86_400..=2_591_999 => format!("{} d ago", delta / 86_400),
        _ => format!("{} mo ago", delta / 2_592_000),
    }
}

/// Seconds as a compact duration ("45s", "12m", "3h", "2d").
#[must_use]
pub fn short_duration(seconds: u64) -> String {
    match seconds {
        0..=99 => format!("{seconds}s"),
        100..=5_999 => format!("{}m", seconds / 60),
        6_000..=172_799 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// The first UUID segment, enough to tell rows apart in a dense table.
#[must_use]
pub fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_owned()
}

/// Absolute timestamp for tooltips (UTC, ISO-ish). The upper bound keeps
/// `Date::to_iso_string` from throwing on garbage timestamps.
#[must_use]
pub fn absolute_time(unix_secs: i64) -> String {
    if unix_secs <= 0 || unix_secs > 253_402_300_799 {
        return "unknown".into();
    }
    #[allow(clippy::cast_precision_loss)]
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(unix_secs as f64 * 1000.0));
    date.to_iso_string().as_string().unwrap_or_default()
}

/// Unix seconds now, from the browser clock.
#[must_use]
pub fn now_secs() -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let now = (js_sys::Date::now() / 1000.0) as i64;
    now
}
