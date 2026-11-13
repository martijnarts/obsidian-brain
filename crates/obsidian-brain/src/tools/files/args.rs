//! Helpers the file tools share: ISO timestamps and JS string order.

/// An ISO 8601 timestamp with milliseconds, as `Date.toISOString` writes.
pub fn iso_time(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `new Date(ms).toISOString()`.
pub fn iso_from_ms(ms: f64) -> String {
    chrono::DateTime::from_timestamp_millis(ms.trunc() as i64)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// JS `<` on strings: UTF-16 code unit order.
pub fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
