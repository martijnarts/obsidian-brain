//! The `next_actions` envelope. A tool opts in by returning
//! `{ data, context: { state, next_actions } }`; a client that reads
//! `context.next_actions` can route the agent's next call, and a client that
//! ignores `context` keeps working.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::util::js;

/// Facts about the last call, for the agent to carry forward. Absent fields
/// are left out of the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StateTokens {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_search_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_search_results: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_file_read: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_file_outgoing: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_file_unresolved_links: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connections_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connections_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextAction {
    pub description: String,
    pub tool: String,
    pub args: Map<String, Value>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ToolContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<StateTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_actions: Option<Vec<NextAction>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextualResult<T> {
    pub data: T,
    pub context: ToolContext,
}

/// True for the `{ data, context }` envelope: an object with both keys and
/// a JS object under `context` (`typeof` says "object" for an array and for
/// null too).
pub fn is_contextual_result(r: &Value) -> bool {
    let Some(obj) = r.as_object() else {
        return false;
    };
    obj.contains_key("data")
        && matches!(
            obj.get("context"),
            Some(Value::Object(_) | Value::Array(_) | Value::Null)
        )
}

fn args(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// One search hit, as the hints read it.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHintHit {
    pub node_id: String,
    pub title: Option<String>,
    pub score: Option<f64>,
}

pub fn compute_search_hints(query: &str, results: &[SearchHintHit]) -> ToolContext {
    let Some(top) = results.first() else {
        let simplified = simplify_query(query);
        let mut next_actions = Vec::new();
        if !simplified.is_empty() && simplified != query {
            next_actions.push(NextAction {
                description: "Retry with broader phrasing".into(),
                tool: "search".into(),
                args: args(json!({ "query": simplified })),
                reason: "No hits for this specific query — try broader phrasing".into(),
            });
        }
        return ToolContext {
            state: Some(StateTokens {
                last_search_query: Some(query.to_string()),
                last_search_results: Some(Vec::new()),
                ..StateTokens::default()
            }),
            next_actions: Some(next_actions),
        };
    };

    let top_title = top.title.as_deref().unwrap_or(&top.node_id);
    let mut next_actions = vec![NextAction {
        description: format!("Read the top hit: {top_title}"),
        tool: "read_note".into(),
        args: args(json!({ "name": top.node_id })),
        reason: match top.score {
            Some(score) => format!("{top_title} scored {score:.3}"),
            None => format!("{top_title} is the top-ranked result"),
        },
    }];
    if results.len() >= 2 {
        next_actions.push(NextAction {
            description: format!("Explore connections of top hit: {top_title}"),
            tool: "find_connections".into(),
            args: args(json!({ "name": top.node_id, "depth": 2 })),
            reason: format!(
                "Top-{} results cluster around {top_title} — trace its graph neighbourhood",
                results.len().min(3)
            ),
        });
    }

    ToolContext {
        state: Some(StateTokens {
            last_search_query: Some(query.to_string()),
            last_search_results: Some(results.iter().take(10).map(|r| r.node_id.clone()).collect()),
            ..StateTokens::default()
        }),
        next_actions: Some(next_actions),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadNoteHintInput {
    pub id: String,
    pub outgoing: Vec<String>,
    pub unresolved_links: Vec<String>,
}

pub fn compute_read_note_hints(note: &ReadNoteHintInput) -> ToolContext {
    let mut next_actions = Vec::new();

    if let Some(first) = note.unresolved_links.first() {
        let n = note.unresolved_links.len();
        next_actions.push(NextAction {
            description: format!("Create missing note: {first}"),
            tool: "create_note".into(),
            args: args(json!({ "title": first })),
            reason: format!(
                "Note has {n} unresolved [[...]] link{} — create '{first}' to resolve",
                if n == 1 { "" } else { "s" }
            ),
        });
    }

    if !note.outgoing.is_empty() {
        let n = note.outgoing.len();
        next_actions.push(NextAction {
            description: format!("Explore outgoing links from {}", note.id),
            tool: "find_connections".into(),
            args: args(json!({ "name": note.id })),
            reason: format!(
                "Explore the {n} note{} this one links out to",
                if n == 1 { "" } else { "s" }
            ),
        });
    }

    ToolContext {
        state: Some(StateTokens {
            last_file_read: Some(note.id.clone()),
            last_file_outgoing: Some(note.outgoing.clone()),
            last_file_unresolved_links: Some(note.unresolved_links.clone()),
            ..StateTokens::default()
        }),
        next_actions: Some(next_actions),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindConnectionsHintNeighbor {
    pub id: String,
    pub title: Option<String>,
}

pub fn compute_find_connections_hints(
    root: &str,
    connections: &[FindConnectionsHintNeighbor],
) -> ToolContext {
    let mut next_actions = Vec::new();
    if let Some(furthest) = connections.last()
        && furthest.id != root
    {
        next_actions.push(NextAction {
            description: format!(
                "Trace path from {root} to {}",
                furthest.title.as_deref().unwrap_or(&furthest.id)
            ),
            tool: "find_path_between".into(),
            args: args(json!({ "source": root, "target": furthest.id })),
            reason: "Find the shortest link chain to the furthest neighbour in this subgraph"
                .into(),
        });
    }

    ToolContext {
        state: Some(StateTokens {
            last_connections_root: Some(root.to_string()),
            last_connections_count: Some(connections.len()),
            ..StateTokens::default()
        }),
        next_actions: Some(next_actions),
    }
}

static QUOTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']"#).expect("valid pattern"));
static WS_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[{}]+", js::WS)).expect("valid pattern"));

/// Drop quotes and common stopwords. Just enough for a looser query when
/// the literal one returned no hits.
fn simplify_query(query: &str) -> String {
    let unquoted = QUOTES.replace_all(query, "");
    let stripped = js::trim(&unquoted);
    let stopwords: HashSet<&str> = [
        "the", "a", "an", "of", "on", "in", "and", "or", "to", "for", "with", "about",
    ]
    .into_iter()
    .collect();
    let tokens: Vec<&str> = WS_RUN
        .split(stripped)
        .filter(|t| !t.is_empty() && !stopwords.contains(t.to_lowercase().as_str()))
        .collect();
    if tokens.len() <= 2 {
        return stripped.to_string();
    }
    let keep = 2.max(tokens.len().div_ceil(2));
    tokens[..keep].join(" ")
}
