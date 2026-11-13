//! Arguments are checked against the tool's input schema before any
//! handler runs, with the error text of the TS SDK.

use serde_json::{Value, json};

use std::sync::Arc;

use obsidian_brain::mcp::Server;
use obsidian_brain::vaults::Vaults;

use crate::support::{Vault, call_text};

async fn text(v: &Vault, tool: &str, args: Value) -> (bool, String) {
    call_text(&v.ctx, tool, args).await
}

/// A call with exactly `args`, `vault` or not.
async fn raw(v: &Vault, tool: &str, args: Value) -> (bool, String) {
    let server = Server::new(
        Arc::new(Vaults::single(v.ctx.clone())),
        obsidian_brain::tools::all(),
    );
    let result = server.call(tool, args.as_object().unwrap().clone()).await;
    let text = result.content[0].as_text().unwrap().text.clone();
    (result.is_error == Some(true), text)
}

#[tokio::test]
async fn a_missing_or_unknown_vault_is_an_argument_error() {
    let v = Vault::new();
    let prefix =
        "MCP error -32602: Input validation error: Invalid arguments for tool list_notes: ";
    let want = format!("{prefix}Invalid input: expected \"test\" at vault");
    for vault in [json!("nope"), json!(3), Value::Null] {
        assert_eq!(
            raw(&v, "list_notes", json!({ "vault": vault })).await,
            (true, want.clone())
        );
    }
    assert_eq!(raw(&v, "list_notes", json!({})).await, (true, want));
}

#[tokio::test]
async fn every_problem_is_listed_on_its_own_line() {
    let v = Vault::new();
    let (is_error, text) = text(&v, "grep_vault", json!({ "query": "", "limit": "x" })).await;
    assert!(is_error);
    assert_eq!(
        text,
        "MCP error -32602: Input validation error: Invalid arguments for tool grep_vault: \
         Too small: expected string to have >=1 characters at query\n\
         Invalid input: expected number, received string at limit"
    );
}

#[tokio::test]
async fn an_integral_float_counts_as_an_integer() {
    let v = Vault::new();
    v.write("A.md", "# A\n");
    v.index().await;
    let (is_error, text) = text(&v, "list_notes", json!({ "limit": 1.0 })).await;
    assert!(!is_error, "{text}");
}
