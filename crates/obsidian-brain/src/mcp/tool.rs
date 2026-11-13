//! The tool registry. A tool is a name, a description, an argument type
//! that derives `JsonSchema`, and an async handler bound to one vault.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::context::VaultContext;

pub type JsonObject = Map<String, Value>;
pub type ToolFuture = Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send>>;
type Handler = Arc<dyn Fn(Arc<VaultContext>, JsonObject) -> ToolFuture + Send + Sync>;

/// Why a call never reached the handler: the arguments do not match the
/// tool's input schema. The server reports it as the TS SDK did, with
/// `MCP error -32602: Input validation error: ...` and no `Error: `.
#[derive(Debug, thiserror::Error)]
#[error("Input validation error: {0}")]
pub struct InvalidArguments(pub String);

#[derive(Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The input schema without the `vault` argument.
    pub input_schema: JsonObject,
    handler: Handler,
}

impl ToolSpec {
    /// A tool whose arguments deserialize into `A`. The handler returns a
    /// JSON value; a string is sent as it is, anything else pretty-printed.
    pub fn new<A, F, Fut>(name: &'static str, description: &'static str, handler: F) -> Self
    where
        A: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(Arc<VaultContext>, A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<Value>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        Self {
            name,
            description,
            input_schema: input_schema_for::<A>(),
            handler: Arc::new(move |ctx, args| {
                let handler = handler.clone();
                Box::pin(async move {
                    let args: A = serde_json::from_value(js_integers(Value::Object(args)))
                        .map_err(|e| InvalidArguments(e.to_string()))?;
                    handler(ctx, args).await
                })
            }),
        }
    }

    pub fn call(&self, ctx: Arc<VaultContext>, args: JsonObject) -> ToolFuture {
        (self.handler)(ctx, args)
    }
}

/// Integral floats as integers: JS has one number type, so zod took `5.0`
/// wherever it took `5`, and serde must too.
fn js_integers(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => crate::util::js::number_value(n.as_f64().unwrap_or(0.0)),
        Value::Array(items) => Value::Array(items.into_iter().map(js_integers).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, js_integers(v))).collect())
        }
        other => other,
    }
}

/// The draft-07 schema of `A`, with optional fields left out of `required`
/// instead of made nullable, which is how zod described them.
pub fn input_schema_for<A: JsonSchema>() -> JsonObject {
    let mut settings = SchemaSettings::draft07();
    settings.inline_subschemas = true;
    let schema = settings.into_generator().into_root_schema_for::<A>();
    let mut obj = match schema.to_value() {
        Value::Object(obj) => obj,
        _ => JsonObject::new(),
    };
    strip_null(&mut obj);
    // A doc comment on the argument struct is for Rust readers only.
    obj.remove("description");
    obj.remove("title");
    obj.remove("definitions");
    obj.remove("$defs");
    obj.insert(
        "$schema".into(),
        "http://json-schema.org/draft-07/schema#".into(),
    );
    obj.entry("type").or_insert_with(|| "object".into());
    obj.entry("properties")
        .or_insert_with(|| Value::Object(JsonObject::new()));
    obj
}

/// Arguments for a tool that takes none.
#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct NoArgs {}

/// Removes the `null` alternative schemars gives an `Option` field. An
/// absent field already means "not given", and zod never allowed null.
fn strip_null(obj: &mut JsonObject) {
    if let Some(Value::Array(types)) = obj.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            obj.insert("type".into(), only);
        }
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(alts)) = obj.get_mut(key) {
            alts.retain(|a| a.get("type").and_then(Value::as_str) != Some("null"));
            if alts.len() == 1
                && let Value::Object(only) = alts.remove(0)
            {
                obj.remove(key);
                for (k, v) in only {
                    obj.entry(k).or_insert(v);
                }
            }
        }
    }
    if let Some(Value::Array(values)) = obj.get_mut("enum") {
        values.retain(|v| !v.is_null());
    }
    if obj.get("default") == Some(&Value::Null) {
        obj.remove("default");
    }
    for value in obj.values_mut() {
        match value {
            Value::Object(child) => strip_null(child),
            Value::Array(items) => items.iter_mut().for_each(|item| {
                if let Value::Object(child) = item {
                    strip_null(child)
                }
            }),
            _ => {}
        }
    }
}
