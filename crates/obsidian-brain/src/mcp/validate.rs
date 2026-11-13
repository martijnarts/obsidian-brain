//! Checks a tool's arguments against its input schema before the handler
//! runs, as the TS SDK checked them against the zod schema. The problems
//! read like zod's issues: `<message> at <path>`, one per line.

use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::{JsonType, ValidationError, Validator};
use serde_json::Value;

use crate::util::js;

/// A compiled input schema.
pub struct ArgsValidator {
    schema: Value,
    validator: Validator,
}

impl ArgsValidator {
    pub fn new(schema: Value) -> Self {
        let validator = jsonschema::draft7::new(&schema)
            .unwrap_or_else(|e| panic!("a tool's input schema does not compile: {e}"));
        Self { schema, validator }
    }

    /// `None` when `args` is valid, else the problems.
    pub fn check(&self, args: &Value) -> Option<String> {
        let errors: Vec<ValidationError<'_>> = self.validator.iter_errors(args).collect();
        // zod checks an enum or a literal without a type check first, so a
        // wrong value is one problem, not two.
        let fixed: Vec<&str> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e.kind(),
                    ValidationErrorKind::Enum { .. } | ValidationErrorKind::Constant { .. }
                )
            })
            .map(|e| e.instance_path().as_str())
            .collect();
        let problems: Vec<String> = errors
            .iter()
            .filter(|e| {
                !matches!(e.kind(), ValidationErrorKind::Type { .. })
                    || !fixed.contains(&e.instance_path().as_str())
            })
            .map(|e| self.describe(e))
            .collect();
        (!problems.is_empty()).then(|| problems.join("\n"))
    }

    fn describe(&self, e: &ValidationError<'_>) -> String {
        let mut path: Vec<String> = e
            .instance_path()
            .as_str()
            .split('/')
            .skip(1)
            .map(|s| s.replace("~1", "/").replace("~0", "~"))
            .collect();
        let message = match e.kind() {
            ValidationErrorKind::Required { property } => {
                let name = property.as_str().unwrap_or_default().to_string();
                let message = match schema_at(&self.schema, &path)
                    .and_then(|s| s.get("properties"))
                    .and_then(|p| p.get(&name))
                {
                    Some(prop) => expected(prop, "received undefined"),
                    None => "Invalid input".to_string(),
                };
                path.push(name);
                message
            }
            ValidationErrorKind::Type { kind } => match kind {
                TypeKind::Single(JsonType::Integer) if e.instance().is_number() => {
                    "Invalid input: expected int, received number".to_string()
                }
                TypeKind::Single(t) => format!(
                    "Invalid input: expected {}, received {}",
                    zod_type(*t),
                    type_of(e.instance())
                ),
                TypeKind::Multiple(_) => "Invalid input".to_string(),
            },
            ValidationErrorKind::Enum { options } => enum_message(options),
            ValidationErrorKind::Constant { expected_value } => {
                format!(
                    "Invalid input: expected {}",
                    js::json_stringify(expected_value)
                )
            }
            ValidationErrorKind::MinLength { limit } => {
                format!("Too small: expected string to have >={limit} characters")
            }
            ValidationErrorKind::MaxLength { limit } => {
                format!("Too big: expected string to have <={limit} characters")
            }
            ValidationErrorKind::MinItems { limit } => {
                format!("Too small: expected array to have >={limit} items")
            }
            ValidationErrorKind::MaxItems { limit } => {
                format!("Too big: expected array to have <={limit} items")
            }
            ValidationErrorKind::Minimum { limit } => {
                format!(
                    "Too small: expected number to be >={}",
                    js::json_stringify(limit)
                )
            }
            ValidationErrorKind::ExclusiveMinimum { limit } => {
                format!(
                    "Too small: expected number to be >{}",
                    js::json_stringify(limit)
                )
            }
            ValidationErrorKind::Maximum { limit } => {
                format!(
                    "Too big: expected number to be <={}",
                    js::json_stringify(limit)
                )
            }
            ValidationErrorKind::ExclusiveMaximum { limit } => {
                format!(
                    "Too big: expected number to be <{}",
                    js::json_stringify(limit)
                )
            }
            ValidationErrorKind::AnyOf { .. } | ValidationErrorKind::OneOfNotValid { .. } => {
                "Invalid input".to_string()
            }
            _ => e.to_string(),
        };
        if path.is_empty() {
            message
        } else {
            format!("{message} at {}", dot_path(&path))
        }
    }
}

/// The subschema that describes the value at `path`.
fn schema_at<'a>(schema: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(schema, |s, seg| {
        s.get("properties")
            .and_then(|p| p.get(seg))
            .or_else(|| s.get("items"))
            .or_else(|| s.get("additionalProperties"))
    })
}

/// What zod says when a value does not match `schema` at all.
fn expected(schema: &Value, received: &str) -> String {
    if let Some(Value::Array(_)) = schema.get("enum") {
        return enum_message(&schema["enum"]);
    }
    if let Some(c) = schema.get("const") {
        return format!("Invalid input: expected {}", js::json_stringify(c));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some(t) => format!("Invalid input: expected {}, {received}", zod_type_name(t)),
        None => "Invalid input".to_string(),
    }
}

/// One allowed value reads as a literal, several as an enum.
fn enum_message(options: &Value) -> String {
    let options: Vec<String> = options
        .as_array()
        .map(|o| o.iter().map(js::json_stringify).collect())
        .unwrap_or_default();
    match options.as_slice() {
        [only] => format!("Invalid input: expected {only}"),
        _ => format!("Invalid option: expected one of {}", options.join("|")),
    }
}

fn zod_type(t: JsonType) -> &'static str {
    zod_type_name(t.as_str())
}

fn zod_type_name(t: &str) -> &str {
    match t {
        "integer" => "number",
        other => other,
    }
}

/// zod's name for the type of a received value.
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The SDK's `getDotPath`: `a.b[0]`.
fn dot_path(path: &[String]) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        if i == 0 {
            out.push_str(seg);
        } else if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(&format!("[{seg}]"));
        } else {
            out.push('.');
            out.push_str(seg);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(schema: Value, args: Value) -> Option<String> {
        ArgsValidator::new(schema).check(&args)
    }

    /// The texts the TS server gave for the same arguments.
    #[test]
    fn problems_read_like_zod_issues() {
        let schema = json!({
            "type": "object",
            "properties": {
                "vault": { "type": "string", "enum": ["v"] },
                "query": { "type": "string", "minLength": 1 },
                "limit": { "type": "integer", "exclusiveMinimum": 0 },
                "names": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "mode": { "type": "string", "enum": ["append", "prepend"] },
                "confirm": { "type": "boolean", "const": true },
            },
            "required": ["vault", "query"],
        });
        assert_eq!(
            check(schema.clone(), json!({"vault": "v", "query": "a"})),
            None
        );
        assert_eq!(
            check(schema.clone(), json!({"query": "a"})).unwrap(),
            "Invalid input: expected \"v\" at vault"
        );
        for vault in [json!("nope"), json!(3), Value::Null] {
            assert_eq!(
                check(schema.clone(), json!({"vault": vault, "query": "a"})).unwrap(),
                "Invalid input: expected \"v\" at vault"
            );
        }
        assert_eq!(
            check(schema.clone(), json!({"vault": "v"})).unwrap(),
            "Invalid input: expected string, received undefined at query"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "", "limit": "x"})
            )
            .unwrap(),
            "Too small: expected string to have >=1 characters at query\n\
             Invalid input: expected number, received string at limit"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "a", "limit": 0})
            )
            .unwrap(),
            "Too small: expected number to be >0 at limit"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "a", "limit": 1.5})
            )
            .unwrap(),
            "Invalid input: expected int, received number at limit"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "a", "names": []})
            )
            .unwrap(),
            "Too small: expected array to have >=1 items at names"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "a", "names": [1]})
            )
            .unwrap(),
            "Invalid input: expected string, received number at names[0]"
        );
        assert_eq!(
            check(
                schema.clone(),
                json!({"vault": "v", "query": "a", "mode": "x"})
            )
            .unwrap(),
            "Invalid option: expected one of \"append\"|\"prepend\" at mode"
        );
        assert_eq!(
            check(
                schema,
                json!({"vault": "v", "query": "a", "confirm": false})
            )
            .unwrap(),
            "Invalid input: expected true at confirm"
        );
    }
}
