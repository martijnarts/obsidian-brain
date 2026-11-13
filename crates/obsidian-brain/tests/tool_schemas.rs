//! Every tool's name, description and input schema must match what the
//! TypeScript server listed, in `conformance/tools-list.golden.json`. The
//! comparison ignores what zod and schemars spell differently but mean the
//! same.

use std::path::Path;
use std::sync::Arc;

use obsidian_brain::mcp::Server;
use obsidian_brain::vaults::{VaultSpec, Vaults};
use serde_json::{Map, Value};

const SAFE_INTEGER: f64 = 9007199254740991.0;

fn golden() -> Vec<Value> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/tools-list.golden.json");
    let text = std::fs::read_to_string(path).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["tools"].as_array().unwrap().clone()
}

async fn server() -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let specs = [VaultSpec {
        name: "fixture".into(),
        path: vault,
    }];
    let vaults = Vaults::open(&specs, &dir.path().join("data"))
        .await
        .unwrap();
    (
        Server::new(Arc::new(vaults), obsidian_brain::tools::all()),
        dir,
    )
}

/// Rewrites a schema into one canonical spelling.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = Map::new();
            for (key, v) in obj {
                match key.as_str() {
                    "$schema" | "format" | "propertyNames" | "title" => continue,
                    "maximum" if v.as_f64() == Some(SAFE_INTEGER) => continue,
                    "minimum" if v.as_f64() == Some(-SAFE_INTEGER) => continue,
                    "additionalProperties"
                        if v == &Value::Bool(true) || v == &Value::Object(Map::new()) =>
                    {
                        continue;
                    }
                    "exclusiveMinimum"
                        if v.as_f64() == Some(0.0)
                            && obj.get("type") == Some(&"integer".into()) =>
                    {
                        out.insert("minimum".into(), Value::from(1));
                    }
                    "required" => {
                        let mut names: Vec<Value> = v.as_array().cloned().unwrap_or_default();
                        names.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                        out.insert(key.clone(), Value::Array(names));
                    }
                    _ => {
                        out.insert(key.clone(), normalize(v));
                    }
                }
            }
            // An integer field that only says `minimum: 0` came from an
            // unsigned Rust type; zod said `minimum: 0` too.
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

#[tokio::test]
async fn ported_tools_match_the_typescript_schemas() {
    let (server, _dir) = server().await;
    let golden = golden();
    let mut mismatches = Vec::new();
    for tool in server.tool_list() {
        let ours = serde_json::to_value(&tool).unwrap();
        let name = ours["name"].as_str().unwrap();
        let Some(theirs) = golden.iter().find(|t| t["name"] == name) else {
            mismatches.push(format!("{name}: not a TypeScript tool"));
            continue;
        };
        if ours["description"] != theirs["description"] {
            mismatches.push(format!(
                "{name}: description differs\n  ours:   {}\n  theirs: {}",
                ours["description"], theirs["description"]
            ));
        }
        let (a, b) = (
            normalize(&ours["inputSchema"]),
            normalize(&theirs["inputSchema"]),
        );
        if a != b {
            mismatches.push(format!(
                "{name}: inputSchema differs\n  ours:   {}\n  theirs: {}",
                serde_json::to_string(&a).unwrap(),
                serde_json::to_string(&b).unwrap()
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
}

/// Fails until every tool is ported. Run with `--ignored` to see the gap.
#[tokio::test]
#[ignore = "the port is not finished"]
async fn every_typescript_tool_is_ported() {
    let (server, _dir) = server().await;
    let ours = server.names();
    let missing: Vec<String> = golden()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .filter(|n| !ours.contains(&n.as_str()))
        .collect();
    assert!(missing.is_empty(), "not ported: {}", missing.join(", "));
}
