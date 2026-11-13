//! Argument checks that zod ran before a handler saw the call: positive and
//! non-negative integers, `z.literal(true)` and bounded lists.

use serde::Deserialize;
use serde::de::{Deserializer, Error};
use serde_json::Value;

const MAX_SAFE_INTEGER: f64 = 9007199254740991.0;

/// A JSON number that is a whole number at least `min`, as
/// `z.number().int()` with `.positive()` or `.min(0)`.
fn integer<E: Error>(value: &Value, min: f64) -> Result<u64, E> {
    let n = value
        .as_f64()
        .ok_or_else(|| E::custom(format!("expected a number, got {value}")))?;
    if n.fract() != 0.0 || n > MAX_SAFE_INTEGER {
        return Err(E::custom(format!("expected an integer, got {value}")));
    }
    if n < min {
        return Err(E::custom(format!(
            "expected a number >= {min}, got {value}"
        )));
    }
    Ok(n as u64)
}

/// An optional `z.number().int().positive()`.
pub fn opt_positive<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::<Value>::deserialize(d)?
        .map(|v| integer(&v, 1.0))
        .transpose()
}

/// An optional `z.number().int().min(0)`.
pub fn opt_non_negative<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::<Value>::deserialize(d)?
        .map(|v| integer(&v, 0.0))
        .transpose()
}

/// `z.literal(true)`.
pub fn literal_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    match bool::deserialize(d)? {
        true => Ok(true),
        false => Err(D::Error::custom("expected true")),
    }
}

/// A list of 1 to 20 strings, as `read_notes` takes.
pub fn one_to_twenty<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let list = Vec::<String>::deserialize(d)?;
    if list.is_empty() || list.len() > 20 {
        return Err(D::Error::custom(format!(
            "expected 1 to 20 items, got {}",
            list.len()
        )));
    }
    Ok(list)
}

/// A string of at least one character.
pub fn non_empty<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(D::Error::custom("expected a non-empty string"));
    }
    Ok(s)
}

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
