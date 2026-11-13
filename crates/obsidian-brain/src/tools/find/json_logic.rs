//! A JsonLogic evaluator with the semantics of json-logic-js: JS loose equality and comparison, JS
//! number parsing, and JsonLogic truthiness (an empty array is falsy, `"0"`
//! is truthy). `datalogic-rs` follows the JsonLogic spec instead and
//! differs from json-logic-js on coercions and on unknown operators.
//!
//! Arrays and objects are reference-counted, so `==` on two of them
//! compares identity, as JS does: the same `var` read twice is equal, two
//! array literals are not.
//!
//! Gaps: `var` sees own data properties only (JS also reaches inherited
//! ones such as `toString`), and an operation name with a dot reports
//! every miss as `Unrecognized operation <op> (failed at <prefix>)`.

use std::rc::Rc;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::util::js;

/// A JS value as JsonLogic sees it.
#[derive(Debug, Clone)]
pub enum JsVal {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Rc<Vec<JsVal>>),
    Obj(Rc<Vec<(String, JsVal)>>),
}

use JsVal::*;

impl From<&Value> for JsVal {
    fn from(v: &Value) -> Self {
        match v {
            Value::Null => Null,
            Value::Bool(b) => Bool(*b),
            Value::Number(n) => Num(js::as_f64(n)),
            Value::String(s) => Str(s.clone()),
            Value::Array(items) => Arr(Rc::new(items.iter().map(JsVal::from).collect())),
            Value::Object(map) => Obj(Rc::new(
                map.iter()
                    .map(|(k, v)| (k.clone(), JsVal::from(v)))
                    .collect(),
            )),
        }
    }
}

/// `jsonLogic.is_logic`: an object with exactly one key.
pub fn is_logic(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.len() == 1)
}

/// `jsonLogic.truthy`.
pub fn truthy(v: &JsVal) -> bool {
    match v {
        Undefined | Null => false,
        Bool(b) => *b,
        Num(n) => *n != 0.0 && !n.is_nan(),
        Str(s) => !s.is_empty(),
        Arr(a) => !a.is_empty(),
        Obj(_) => true,
    }
}

/// `jsonLogic.apply(logic, data)`, with the `has_tag` operation added.
pub fn apply(logic: &Value, data: &JsVal) -> Result<JsVal> {
    match logic {
        Value::Array(items) => Ok(Arr(Rc::new(
            items
                .iter()
                .map(|l| apply(l, data))
                .collect::<Result<_>>()?,
        ))),
        Value::Object(map) if map.len() == 1 => {
            let (op, values) = map.iter().next().expect("one key");
            apply_op(op, values, data)
        }
        other => Ok(JsVal::from(other)),
    }
}

fn apply_op(op: &str, values: &Value, data: &JsVal) -> Result<JsVal> {
    // Unary syntax: `{"var": "x"}` is `{"var": ["x"]}`.
    let values: &[Value] = match values {
        Value::Array(items) => items,
        other => std::slice::from_ref(other),
    };
    let arg = |i: usize| -> Result<JsVal> {
        match values.get(i) {
            Some(v) => apply(v, data),
            None => Ok(Undefined),
        }
    };

    // These evaluate their own arguments, lazily.
    match op {
        "if" | "?:" => {
            let mut i = 0;
            while i + 1 < values.len() {
                if truthy(&apply(&values[i], data)?) {
                    return apply(&values[i + 1], data);
                }
                i += 2;
            }
            return if values.len() == i + 1 {
                apply(&values[i], data)
            } else {
                Ok(Null)
            };
        }
        "and" | "or" => {
            let want = op == "or";
            let mut current = Undefined;
            for v in values {
                current = apply(v, data)?;
                if truthy(&current) == want {
                    return Ok(current);
                }
            }
            return Ok(current);
        }
        "filter" | "map" | "all" | "none" | "some" => {
            let scoped = arg(0)?;
            let logic = values.get(1).unwrap_or(&Value::Null);
            let Arr(items) = scoped else {
                return Ok(match op {
                    "filter" | "map" => Arr(Rc::new(Vec::new())),
                    "none" => Bool(true),
                    _ => Bool(false),
                });
            };
            return Ok(match op {
                "filter" => {
                    let mut kept = Vec::new();
                    for item in items.iter() {
                        if truthy(&apply(logic, item)?) {
                            kept.push(item.clone());
                        }
                    }
                    Arr(Rc::new(kept))
                }
                "map" => Arr(Rc::new(
                    items
                        .iter()
                        .map(|item| apply(logic, item))
                        .collect::<Result<_>>()?,
                )),
                "all" => {
                    if items.is_empty() {
                        return Ok(Bool(false));
                    }
                    for item in items.iter() {
                        if !truthy(&apply(logic, item)?) {
                            return Ok(Bool(false));
                        }
                    }
                    Bool(true)
                }
                "none" => {
                    for item in items.iter() {
                        if truthy(&apply(logic, item)?) {
                            return Ok(Bool(false));
                        }
                    }
                    Bool(true)
                }
                _ => {
                    for item in items.iter() {
                        if truthy(&apply(logic, item)?) {
                            return Ok(Bool(true));
                        }
                    }
                    Bool(false)
                }
            });
        }
        "reduce" => {
            let scoped = arg(0)?;
            let logic = values.get(1).unwrap_or(&Value::Null);
            let initial = match values.get(2) {
                Some(v) => apply(v, data)?,
                None => Null,
            };
            let Arr(items) = scoped else {
                return Ok(initial);
            };
            let mut acc = initial;
            for item in items.iter() {
                let scope = Obj(Rc::new(vec![
                    ("current".into(), item.clone()),
                    ("accumulator".into(), acc),
                ]));
                acc = apply(logic, &scope)?;
            }
            return Ok(acc);
        }
        _ => {}
    }

    // Everything else evaluates its arguments first.
    let args: Vec<JsVal> = values
        .iter()
        .map(|v| apply(v, data))
        .collect::<Result<_>>()?;
    let a = |i: usize| args.get(i).cloned().unwrap_or(Undefined);

    Ok(match op {
        "==" => Bool(loose_eq(&a(0), &a(1))),
        "===" => Bool(strict_eq(&a(0), &a(1))),
        "!=" => Bool(!loose_eq(&a(0), &a(1))),
        "!==" => Bool(!strict_eq(&a(0), &a(1))),
        ">" => Bool(less_than(&a(1), &a(0)) == Some(true)),
        ">=" => Bool(less_than(&a(0), &a(1)) == Some(false)),
        "<" => {
            let ab = less_than(&a(0), &a(1)) == Some(true);
            match args.get(2) {
                None | Some(Undefined) => Bool(ab),
                Some(c) => Bool(ab && less_than(&a(1), c) == Some(true)),
            }
        }
        "<=" => {
            let le = |x: &JsVal, y: &JsVal| less_than(y, x) == Some(false);
            match args.get(2) {
                None | Some(Undefined) => Bool(le(&a(0), &a(1))),
                Some(c) => Bool(le(&a(0), &a(1)) && le(&a(1), c)),
            }
        }
        "!!" => Bool(truthy(&a(0))),
        "!" => Bool(!truthy(&a(0))),
        "%" => Num(to_number(&a(0)) % to_number(&a(1))),
        "log" => a(0),
        "in" => Bool(js_in(&a(0), &a(1))),
        "cat" => Str(args.iter().map(join_part).collect()),
        "substr" => Str(substr(&a(0), &a(1), &a(2))),
        "+" => Num(args
            .iter()
            .fold(0.0, |acc, v| acc + parse_float(&to_js_string(v)))),
        "*" => match args.split_first() {
            None => bail!("Reduce of empty array with no initial value"),
            Some((first, [])) => first.clone(),
            Some((first, rest)) => Num(rest
                .iter()
                .fold(parse_float(&to_js_string(first)), |acc, v| {
                    acc * parse_float(&to_js_string(v))
                })),
        },
        "-" => match args.get(1) {
            None | Some(Undefined) => Num(-to_number(&a(0))),
            Some(b) => Num(to_number(&a(0)) - to_number(b)),
        },
        "/" => Num(to_number(&a(0)) / to_number(&a(1))),
        "min" => Num(args.iter().map(to_number).fold(f64::INFINITY, js_min)),
        "max" => Num(args.iter().map(to_number).fold(f64::NEG_INFINITY, js_max)),
        "merge" => {
            let mut out = Vec::new();
            for v in &args {
                match v {
                    Arr(items) => out.extend(items.iter().cloned()),
                    other => out.push(other.clone()),
                }
            }
            Arr(Rc::new(out))
        }
        "var" => var(&a(0), &a(1), data),
        "missing" => missing(&args, data),
        "missing_some" => {
            let options = a(1);
            let keys: Vec<JsVal> = match &options {
                Arr(items) => items.to_vec(),
                other => vec![other.clone()],
            };
            let are_missing = missing(&keys, data);
            let Arr(missing_keys) = &are_missing else {
                unreachable!("missing returns an array")
            };
            let have = to_number(&property(&options, "length")) - missing_keys.len() as f64;
            if less_than(&Num(have), &a(0)) == Some(false) {
                Arr(Rc::new(Vec::new()))
            } else {
                are_missing
            }
        }
        "has_tag" => Bool(has_tag(&a(0), &a(1))),
        _ => {
            if op.find('.').is_some_and(|i| i > 0) {
                let parts: Vec<&str> = op.split('.').collect();
                let fail = if is_operation(parts[0]) { 2 } else { 1 };
                bail!(
                    "Unrecognized operation {op} (failed at {})",
                    parts[..fail].join(".")
                );
            }
            bail!("Unrecognized operation {op}")
        }
    })
}

fn is_operation(name: &str) -> bool {
    matches!(
        name,
        "==" | "==="
            | "!="
            | "!=="
            | ">"
            | ">="
            | "<"
            | "<="
            | "!!"
            | "!"
            | "%"
            | "log"
            | "in"
            | "cat"
            | "substr"
            | "+"
            | "*"
            | "-"
            | "/"
            | "min"
            | "max"
            | "merge"
            | "var"
            | "missing"
            | "missing_some"
            | "has_tag"
    )
}

/// `has_tag(tags, tag)`: case-insensitive, ignores a leading `#`, and a
/// parent tag matches its nested tags (`area` matches `area/work`), as in
/// Obsidian's tag search.
fn has_tag(tags: &JsVal, tag: &JsVal) -> bool {
    let (Arr(tags), Str(tag)) = (tags, tag) else {
        return false;
    };
    let want = tag.strip_prefix('#').unwrap_or(tag).to_lowercase();
    let nested = format!("{want}/");
    tags.iter().any(|t| match t {
        Str(t) => {
            let have = t.to_lowercase();
            have == want || have.starts_with(&nested)
        }
        _ => false,
    })
}

/// `{"var": [path, default]}`: walk a dotted path into the data.
fn var(path: &JsVal, default: &JsVal, data: &JsVal) -> JsVal {
    let not_found = match default {
        Undefined => Null,
        other => other.clone(),
    };
    match path {
        Undefined | Null => return data.clone(),
        Str(s) if s.is_empty() => return data.clone(),
        _ => {}
    }
    let mut cur = data.clone();
    for prop in to_js_string(path).split('.') {
        if matches!(cur, Null | Undefined) {
            return not_found;
        }
        cur = property(&cur, prop);
        if matches!(cur, Undefined) {
            return not_found;
        }
    }
    cur
}

/// `obj[key]` for the own data properties of a JSON value.
fn property(v: &JsVal, key: &str) -> JsVal {
    let index = || -> Option<usize> {
        if key == "0" || (!key.starts_with('0') && key.bytes().all(|b| b.is_ascii_digit())) {
            key.parse().ok()
        } else {
            None
        }
    };
    match v {
        Obj(fields) => fields
            .iter()
            .find(|(k, _)| k == key)
            .map_or(Undefined, |(_, v)| v.clone()),
        Arr(items) if key == "length" => Num(items.len() as f64),
        Arr(items) => index()
            .and_then(|i| items.get(i).cloned())
            .unwrap_or(Undefined),
        Str(s) if key == "length" => Num(js::utf16_len(s) as f64),
        Str(s) => match index() {
            Some(i) if i < js::utf16_len(s) => Str(js::utf16_slice(s, i, i + 1)),
            _ => Undefined,
        },
        _ => Undefined,
    }
}

/// `missing`: the keys whose `var` is null or `""`. One array argument is
/// the list of keys.
fn missing(args: &[JsVal], data: &JsVal) -> JsVal {
    let keys: Vec<JsVal> = match args.first() {
        Some(Arr(items)) => items.to_vec(),
        _ => args.to_vec(),
    };
    let out = keys
        .into_iter()
        .filter(|key| {
            let value = match key {
                Arr(pair) => var(
                    pair.first().unwrap_or(&Undefined),
                    pair.get(1).unwrap_or(&Undefined),
                    data,
                ),
                other => var(other, &Undefined, data),
            };
            matches!(&value, Null) || matches!(&value, Str(s) if s.is_empty())
        })
        .collect();
    Arr(Rc::new(out))
}

/// `b.indexOf(a) !== -1` for a string or an array `b`.
fn js_in(a: &JsVal, b: &JsVal) -> bool {
    match b {
        Str(s) if !s.is_empty() => s.contains(to_js_string(a).as_str()),
        Arr(items) => items.iter().any(|item| strict_eq(a, item)),
        _ => false,
    }
}

/// `String(source).substr(start, length)`, with a negative length cutting
/// that many units off the end, as json-logic-js does.
fn substr(source: &JsVal, start: &JsVal, end: &JsVal) -> String {
    let s = to_js_string(source);
    if less_than(end, &Num(0.0)) == Some(true) {
        let temp = js_substr(&s, start, &Undefined);
        let len = js::utf16_len(&temp) as f64 + to_number(end);
        return js_substr(&temp, &Num(0.0), &Num(len));
    }
    js_substr(&s, start, end)
}

/// `String.prototype.substr` on UTF-16 units.
fn js_substr(s: &str, start: &JsVal, length: &JsVal) -> String {
    let size = js::utf16_len(s) as f64;
    let mut from = to_integer(to_number(start));
    if from == f64::NEG_INFINITY {
        from = 0.0;
    } else if from < 0.0 {
        from = (size + from).max(0.0);
    } else {
        from = from.min(size);
    }
    let len = match length {
        Undefined => size,
        other => to_integer(to_number(other)),
    };
    let to = (from + len).clamp(from, size);
    if from >= to {
        return String::new();
    }
    js::utf16_slice(s, from as usize, to as usize)
}

/// ToIntegerOrInfinity.
fn to_integer(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x.trunc() }
}

fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Array.prototype.join` of one element: null and undefined are empty.
fn join_part(v: &JsVal) -> String {
    match v {
        Undefined | Null => String::new(),
        other => to_js_string(other),
    }
}

/// `String(v)`.
pub fn to_js_string(v: &JsVal) -> String {
    match v {
        Undefined => "undefined".into(),
        Null => "null".into(),
        Bool(b) => b.to_string(),
        Num(n) => js::number_to_string(*n),
        Str(s) => s.clone(),
        Arr(items) => items.iter().map(join_part).collect::<Vec<_>>().join(","),
        Obj(_) => "[object Object]".into(),
    }
}

/// ToPrimitive: an array or object becomes its string form.
fn to_primitive(v: &JsVal) -> JsVal {
    match v {
        Arr(_) | Obj(_) => Str(to_js_string(v)),
        other => other.clone(),
    }
}

/// ToNumber.
fn to_number(v: &JsVal) -> f64 {
    match v {
        Undefined => f64::NAN,
        Null => 0.0,
        Bool(b) => f64::from(u8::from(*b)),
        Num(n) => *n,
        Str(s) => string_to_number(s),
        Arr(_) | Obj(_) => to_number(&to_primitive(v)),
    }
}

/// StringToNumber: the whole trimmed string must be a numeric literal.
fn string_to_number(s: &str) -> f64 {
    let t = js::trim(s);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = t.strip_prefix(prefix) {
            if digits.is_empty() {
                return f64::NAN;
            }
            let mut n = 0.0;
            for c in digits.chars() {
                let Some(d) = c.to_digit(radix) else {
                    return f64::NAN;
                };
                n = n * radix as f64 + d as f64;
            }
            return n;
        }
    }
    match decimal_prefix(t) {
        Some(end) if end == t.len() => parse_decimal(t),
        _ => f64::NAN,
    }
}

/// `parseFloat(s)`: the longest decimal literal at the start, after
/// leading whitespace.
fn parse_float(s: &str) -> f64 {
    let t = js::trim_start(s);
    match decimal_prefix(t) {
        Some(end) => parse_decimal(&t[..end]),
        None => f64::NAN,
    }
}

fn parse_decimal(lit: &str) -> f64 {
    let (sign, body) = match lit.as_bytes().first() {
        Some(b'-') => (-1.0, &lit[1..]),
        Some(b'+') => (1.0, &lit[1..]),
        _ => (1.0, lit),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    sign * body.parse::<f64>().unwrap_or(f64::NAN)
}

/// The byte length of the StrDecimalLiteral at the start of `s`: an
/// optional sign, then `Infinity` or digits with an optional fraction and
/// exponent.
fn decimal_prefix(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return Some(i + "Infinity".len());
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        let mut j = i + 1;
        frac = digits(&mut j);
        if int > 0 || frac > 0 {
            i = j;
        }
    }
    if int == 0 && frac == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    Some(i)
}

/// `a === b`.
fn strict_eq(a: &JsVal, b: &JsVal) -> bool {
    match (a, b) {
        (Undefined, Undefined) | (Null, Null) => true,
        (Bool(x), Bool(y)) => x == y,
        (Num(x), Num(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        (Arr(x), Arr(y)) => Rc::ptr_eq(x, y),
        (Obj(x), Obj(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

/// `a == b`: IsLooselyEqual.
fn loose_eq(a: &JsVal, b: &JsVal) -> bool {
    match (a, b) {
        (Undefined | Null, Undefined | Null) => true,
        (Undefined | Null, _) | (_, Undefined | Null) => false,
        (Num(_), Str(_)) | (Str(_), Num(_)) => to_number(a) == to_number(b),
        (Bool(_), _) => loose_eq(&Num(to_number(a)), b),
        (_, Bool(_)) => loose_eq(a, &Num(to_number(b))),
        (Arr(_) | Obj(_), Num(_) | Str(_)) => loose_eq(&to_primitive(a), b),
        (Num(_) | Str(_), Arr(_) | Obj(_)) => loose_eq(a, &to_primitive(b)),
        _ => strict_eq(a, b),
    }
}

/// IsLessThan: `None` when either side is NaN (JS `undefined`). Two
/// strings compare by UTF-16 code units; anything else as numbers.
fn less_than(a: &JsVal, b: &JsVal) -> Option<bool> {
    let (pa, pb) = (to_primitive(a), to_primitive(b));
    if let (Str(x), Str(y)) = (&pa, &pb) {
        return Some(x.encode_utf16().lt(y.encode_utf16()));
    }
    let (x, y) = (to_number(&pa), to_number(&pb));
    if x.is_nan() || y.is_nan() {
        return None;
    }
    Some(x < y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(logic: Value, data: Value) -> Result<JsVal> {
        apply(&logic, &JsVal::from(&data))
    }

    fn t(logic: Value, data: Value) -> bool {
        truthy(&eval(logic, data).unwrap())
    }

    fn s(logic: Value) -> String {
        to_js_string(&eval(logic, json!({})).unwrap())
    }

    // Every expected value below is what json-logic-js 2.0.5 returns for
    // the same logic and data.

    #[test]
    fn truthiness_matches_json_logic_js() {
        let data = json!({"empty": [], "zero": "0", "list": [0], "obj": {}, "n": 0, "blank": ""});
        let cases = [
            (json!({"var": "empty"}), false),
            (json!({"var": "zero"}), true),
            (json!({"var": "list"}), true),
            (json!({"var": "obj"}), true),
            (json!({"var": "n"}), false),
            (json!({"var": "blank"}), false),
            (json!({"var": "nope"}), false),
            (json!({"!!": [[]]}), false),
            (json!({"!!": ["0"]}), true),
            (json!({"!": [[]]}), true),
            (json!({"!!": [[0]]}), true),
            (json!({"and": [[], true]}), false),
            (json!({"or": [[], "0"]}), true),
            (json!({"if": [[], 1, 0]}), false),
            (json!({"if": ["0", 1, 0]}), true),
            (json!({"/": [0, 0]}), false),
        ];
        for (logic, want) in cases {
            assert_eq!(t(logic.clone(), data.clone()), want, "{logic}");
        }
    }

    #[test]
    fn loose_equality_and_comparison_match_js() {
        let cases = [
            (json!({"==": [1, "1"]}), true),
            (json!({"==": ["4", 4]}), true),
            (json!({"==": [0, false]}), true),
            (json!({"==": ["", 0]}), true),
            (json!({"==": [null, 0]}), false),
            (json!({"==": [{"var": "x"}, null]}), true),
            (json!({"==": [[1], 1]}), true),
            (json!({"==": [[1, 2], "1,2"]}), true),
            (json!({"==": [[], []]}), false),
            (json!({"==": [{"var": "a"}, {"var": "a"}]}), true),
            (json!({"===": [1, "1"]}), false),
            (json!({"!=": [1, "1"]}), false),
            (json!({"!==": [1, "1"]}), true),
            (json!({">": ["10", 9]}), true),
            (json!({">": ["10", "9"]}), false),
            (json!({"<": ["2020-01-01", "2021"]}), true),
            (json!({"<": [1, 2, 3]}), true),
            (json!({"<": [1, 3, 2]}), false),
            (json!({"<=": [1, 1, 2]}), true),
            (json!({">=": [null, 0]}), true),
            (json!({">=": ["a", 0]}), false),
            (json!({"<=": [{"var": "x"}, 0]}), true),
            (json!({">": [" 5 ", 4]}), true),
            (json!({"==": ["0x10", 16]}), true),
            (json!({"==": ["1e3", 1000]}), true),
            (json!({"==": ["Infinity", {"/": [1, 0]}]}), true),
            (json!({"==": ["inf", {"/": [1, 0]}]}), false),
            (json!({"<=": [{"var": "nope"}, -1]}), false),
            (json!({"==": [true, "1"]}), true),
            (json!({"==": [[0], false]}), true),
            (json!({"==": [{"var": "a"}, "1"]}), true),
            (json!({"<": ["a", "b", "c"]}), true),
            (json!({"<": [null, 1]}), true),
            (json!({"<": [[2], [10]]}), false),
            (json!({"<": [[2], 10]}), true),
            (json!({"==": ["+0x10", 16]}), false),
            (json!({"==": [" ", 0]}), true),
            (json!({"==": ["1.", 1]}), true),
            (json!({"==": [".5", 0.5]}), true),
            (json!({"==": ["\u{a0}1\n", 1]}), true),
        ];
        for (logic, want) in cases {
            assert_eq!(t(logic.clone(), json!({"a": [1]})), want, "{logic}");
        }
    }

    #[test]
    fn arithmetic_and_strings_match_js() {
        assert_eq!(s(json!({"+": ["1.5x", 2]})), "3.5");
        assert_eq!(s(json!({"+": []})), "0");
        assert_eq!(s(json!({"+": ["abc"]})), "NaN");
        assert_eq!(s(json!({"*": ["3"]})), "3");
        assert_eq!(s(json!({"*": [2, "3"]})), "6");
        assert_eq!(s(json!({"-": ["5"]})), "-5");
        assert_eq!(s(json!({"-": [5, "2"]})), "3");
        assert_eq!(s(json!({"%": [7, 3]})), "1");
        assert_eq!(s(json!({"/": [1, 0]})), "Infinity");
        assert_eq!(s(json!({"min": [3, "1", 2]})), "1");
        assert_eq!(s(json!({"max": []})), "-Infinity");
        assert_eq!(s(json!({"cat": ["a", null, 1, [1, 2]]})), "a11,2");
        assert_eq!(s(json!({"substr": ["jsonlogic", 4]})), "logic");
        assert_eq!(s(json!({"substr": ["jsonlogic", -5]})), "logic");
        assert_eq!(s(json!({"substr": ["jsonlogic", 1, 3]})), "son");
        assert_eq!(s(json!({"substr": ["jsonlogic", 4, -2]})), "log");
        assert_eq!(s(json!({"merge": [[1, 2], 3, [[4]]]})), "1,2,3,4");
        assert_eq!(s(json!({"+": [".5", "-.5e1", "1e", " 2"]})), "-1.5");
        assert_eq!(s(json!({"+": ["Infinityx"]})), "Infinity");
        assert_eq!(
            s(json!({"cat": [1e21, 0.1, 0, 1.0 / 3.0]})),
            "1e+210.100.3333333333333333"
        );
        assert_eq!(s(json!({"min": [1, "x"]})), "NaN");
        assert_eq!(s(json!({"%": [-7, 3]})), "-1");
        assert_eq!(s(json!({"+": [[1, 2]]})), "1");
        // JS slices half of the surrogate pair; it leaves JS as U+FFFD.
        assert_eq!(s(json!({"substr": ["\u{1F600}abc", 1, 2]})), "\u{FFFD}a");
        assert_eq!(
            eval(json!({"*": []}), json!({})).unwrap_err().to_string(),
            "Reduce of empty array with no initial value"
        );
    }

    #[test]
    fn in_var_and_missing_match_js() {
        let data = json!({"path": "Books/Dune.md", "tags": ["a", "b"], "fm": {"x": {"y": 2}}});
        assert!(t(json!({"in": ["Books/", {"var": "path"}]}), data.clone()));
        assert!(t(json!({"in": ["a", {"var": "tags"}]}), data.clone()));
        assert!(!t(json!({"in": ["a", ""]}), data.clone()));
        assert!(!t(json!({"in": ["a", 5]}), data.clone()));
        assert_eq!(s(json!({"var": "nope"})), "null");
        let d = JsVal::from(&data);
        let v = |l: Value| to_js_string(&apply(&l, &d).unwrap());
        assert_eq!(v(json!({"var": "fm.x.y"})), "2");
        assert_eq!(v(json!({"var": ["fm.z", "dflt"]})), "dflt");
        assert_eq!(v(json!({"var": "tags.1"})), "b");
        assert_eq!(v(json!({"var": "tags.length"})), "2");
        assert_eq!(v(json!({"var": "path.length"})), "13");
        assert_eq!(v(json!({"missing": ["path", "nope", "tags"]})), "nope");
        assert_eq!(v(json!({"missing_some": [1, ["path", "nope"]]})), "");
        assert_eq!(v(json!({"missing_some": [2, ["path", "nope"]]})), "nope");
    }

    #[test]
    fn scoped_operations_match_js() {
        let data = json!({"r": [1, 2, 3]});
        let d = JsVal::from(&data);
        let v = |l: Value| to_js_string(&apply(&l, &d).unwrap());
        assert_eq!(
            v(json!({"filter": [{"var": "r"}, {">": [{"var": ""}, 1]}]})),
            "2,3"
        );
        assert_eq!(
            v(json!({"map": [{"var": "r"}, {"*": [{"var": ""}, 2]}]})),
            "2,4,6"
        );
        assert_eq!(
            v(
                json!({"reduce": [{"var": "r"}, {"+": [{"var": "current"}, {"var": "accumulator"}]}, 0]})
            ),
            "6"
        );
        assert_eq!(v(json!({"all": [[], true]})), "false");
        assert_eq!(v(json!({"none": [[], true]})), "true");
        assert_eq!(
            v(json!({"some": [{"var": "r"}, {"==": [{"var": ""}, 2]}]})),
            "true"
        );
        assert_eq!(v(json!({"map": [5, 1]})), "");
    }

    #[test]
    fn has_tag_is_case_insensitive_and_nested() {
        let data = json!({"tags": ["Book", "area/work"], "title": "x"});
        assert!(t(
            json!({"has_tag": [{"var": "tags"}, "#BOOK"]}),
            data.clone()
        ));
        assert!(t(
            json!({"has_tag": [{"var": "tags"}, "area"]}),
            data.clone()
        ));
        assert!(!t(
            json!({"has_tag": [{"var": "tags"}, "are"]}),
            data.clone()
        ));
        assert!(!t(
            json!({"has_tag": [{"var": "title"}, "x"]}),
            data.clone()
        ));
        assert!(!t(json!({"has_tag": [{"var": "tags"}, 1]}), data));
    }

    #[test]
    fn unknown_operations_fail_as_in_js() {
        let err = |l: Value| eval(l, json!({})).unwrap_err().to_string();
        assert_eq!(
            err(json!({"startsWith": [1]})),
            "Unrecognized operation startsWith"
        );
        assert_eq!(
            err(json!({"a.b": [1]})),
            "Unrecognized operation a.b (failed at a)"
        );
        assert_eq!(
            err(json!({"var.x": [1]})),
            "Unrecognized operation var.x (failed at var.x)"
        );
        assert_eq!(
            err(json!({"==": [{"nope": 1}, 1]})),
            "Unrecognized operation nope"
        );
        // A literal object with several keys is data, not logic.
        assert!(!t(json!({"==": [{"a": 1, "b": 2}, 1]}), json!({})));
    }
}
