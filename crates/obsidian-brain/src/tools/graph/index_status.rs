//! `index_status`: counts and embedder state read straight from the
//! index. Never touches the embedder beyond its identity.

use std::sync::LazyLock;

use regex::Regex;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::context::VaultContext;
use crate::mcp::tool::{NoArgs, ToolSpec};
use crate::store::metadata::get_metadata;
use crate::util::js;

const DESCRIPTION: &str = "Report the current state of the vault index: embedder model + dim, count of notes / chunks indexed, chunks skipped during the last reindex (if any), advertised vs discovered max tokens, last reindex reasons, whether a reindex is currently in flight, and the last init error. Read-only — does not mutate anything.";

static NO_SUCH_TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)no such table").expect("valid pattern"));

/// `default` when the table is missing (an index from before it existed);
/// every other SQL error propagates.
fn tolerate_missing_table<T>(r: rusqlite::Result<T>, default: T) -> rusqlite::Result<T> {
    match r {
        Err(e) if NO_SUCH_TABLE.is_match(&e.to_string()) => Ok(default),
        other => other,
    }
}

fn sql_value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(x) => js::number_value(x),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned().into(),
    }
}

fn count(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<i64> {
    conn.query_row(sql, params, |r| r.get(0))
}

/// The live context state the report needs besides the index.
struct Live {
    model: String,
    dim: anyhow::Result<usize>,
    provider: &'static str,
    reasons: Vec<String>,
    reindex_in_progress: bool,
    embedder_ready: bool,
    init_error: Option<String>,
}

impl Live {
    fn of(ctx: &VaultContext) -> Self {
        let mut reasons = ctx
            .bootstrap_result()
            .map(|b| b.reasons)
            .unwrap_or_default();
        // A user-triggered reindex shows up here too, not only the
        // migrations bootstrap starts.
        if let Some(reason) = ctx.last_manual_reindex_reason() {
            reasons.push(reason);
        }
        Self {
            model: ctx.embedder.model_identifier(),
            dim: ctx.embedder.dimensions(),
            provider: ctx.embedder.provider_name(),
            reasons,
            reindex_in_progress: ctx.reindex_in_progress(),
            embedder_ready: ctx.embedder_ready(),
            init_error: ctx.init_error(),
        }
    }
}

fn report(conn: &Connection, live: Live) -> anyhow::Result<Value> {
    let notes_total = count(
        conn,
        "SELECT COUNT(*) AS n FROM nodes WHERE frontmatter NOT LIKE ?",
        ["%_stub\":true%"],
    )?;
    let notes_with_embeddings = count(
        conn,
        "SELECT COUNT(DISTINCT node_id) AS n FROM chunks JOIN chunks_vec ON chunks.rowid = chunks_vec.rowid",
        [],
    )?;
    let chunks_total = count(conn, "SELECT COUNT(*) AS n FROM chunks", [])?;

    // Notes the indexer recorded as having nothing to embed, kept apart from
    // the ones that failed, so "X / Y indexed" is honest about daily-note
    // stubs.
    let notes_no_embeddable_content = tolerate_missing_table(
        count(
            conn,
            "SELECT COUNT(DISTINCT note_id) AS n FROM failed_chunks WHERE reason = 'no-embeddable-content'",
            [],
        ),
        0,
    )?;

    let failed_chunks: Vec<Value> = tolerate_missing_table(
        conn.prepare(
            "SELECT note_id AS note, reason, failed_at AS at FROM failed_chunks ORDER BY failed_at DESC LIMIT 50",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |r| {
                Ok(json!({
                    "note": sql_value(r.get_ref(0)?),
                    "reason": sql_value(r.get_ref(1)?),
                    "at": sql_value(r.get_ref(2)?),
                }))
            })?
            .collect()
        }),
        Vec::new(),
    )?;

    let (advertised_max_tokens, discovered_max_tokens) = tolerate_missing_table(
        conn.query_row(
            "SELECT advertised_max_tokens AS adv, discovered_max_tokens AS disc FROM embedder_capability WHERE embedder_id = ?",
            [&live.model],
            |r| Ok((sql_value(r.get_ref(0)?), sql_value(r.get_ref(1)?))),
        )
        .optional()
        .map(|cap| cap.unwrap_or((Value::Null, Value::Null))),
        (Value::Null, Value::Null),
    )?;

    let notes_missing_embeddings =
        0.max(notes_total - notes_with_embeddings - notes_no_embeddable_content);
    let mut summary = format!("{notes_with_embeddings} / {notes_total} notes indexed");
    if notes_no_embeddable_content > 0 {
        summary.push_str(&format!(
            "; {notes_no_embeddable_content} have no embeddable content (empty / frontmatter-only)"
        ));
    }
    if notes_missing_embeddings > 0 {
        summary.push_str(&format!("; {notes_missing_embeddings} failed to embed"));
    }

    let embedding_model = match get_metadata(conn, "embedding_model")? {
        Some(m) => m,
        None => live.model.clone(),
    };
    let embedding_dim = match get_metadata(conn, "embedding_dim")? {
        Some(d) => js::number_value(js::number(Some(&d))),
        None => js::number_value(live.dim? as f64),
    };
    let provider = match get_metadata(conn, "embedder_provider")? {
        Some(p) => p,
        None => live.provider.to_string(),
    };

    let failed_total = failed_chunks.len();
    Ok(json!({
        "embeddingModel": embedding_model,
        "embeddingDim": embedding_dim,
        "provider": provider,
        "notesTotal": notes_total,
        "notesWithEmbeddings": notes_with_embeddings,
        "notesNoEmbeddableContent": notes_no_embeddable_content,
        "notesMissingEmbeddings": notes_missing_embeddings,
        "summary": summary,
        "chunksTotal": chunks_total,
        "chunksSkippedInLastRun": failed_total,
        "failedChunks": failed_chunks.into_iter().take(10).collect::<Vec<_>>(),
        "failedChunksTotal": failed_total,
        "advertisedMaxTokens": advertised_max_tokens,
        "discoveredMaxTokens": discovered_max_tokens,
        "lastReindexReasons": live.reasons,
        "reindexInProgress": live.reindex_in_progress,
        "embedderReady": live.embedder_ready,
        "initError": live.init_error,
    }))
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<NoArgs, _, _>("index_status", DESCRIPTION, |ctx, _args| async move {
        let live = Live::of(&ctx);
        ctx.db.run(move |c| report(c, live)).await
    })
}
