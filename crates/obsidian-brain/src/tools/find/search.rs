//! `search`: hybrid, semantic or full-text search over the vault.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::check_range;
use crate::mcp::tool::ToolSpec;
use crate::search::SearchUnique;
use crate::tools::support::{SearchHintHit, compute_search_hints, describe_embedder_preparing};

const DESCRIPTION: &str = "Search vault notes. `hybrid` (default) fuses semantic + full-text ranks via Reciprocal Rank Fusion — no tuning needed, best for most queries. `semantic` is concept-only (better for abstract/paraphrased queries). `fulltext` is literal-token (better when you know the exact phrase exists). Semantic search is chunk-level — results are deduped to one-per-note by default. Set `unique: \"chunks\"` to return chunk-level hits with `chunkHeading`, `chunkStartLine`, and `chunkExcerpt`; supported by `semantic` and `hybrid` modes. `fulltext` is note-level only and ignores `unique` (full-text chunk search is not yet supported). Response is wrapped as `{data, context}` where `context.next_actions` suggests the agent's most useful follow-up call (read top hit, explore connections, or retry with broader phrasing on zero hits).";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Hybrid,
    Semantic,
    Fulltext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Unique {
    Notes,
    Chunks,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Natural-language query or keyword phrase.
    query: String,
    /// Default `hybrid`. Semantic-only queries chunk vectors; fulltext-only queries FTS5.
    mode: Option<Mode>,
    /// Max results to return. Default 20.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
    /// Default `"notes"` (one row per note). Set `"chunks"` for raw chunk rows with chunkHeading, chunkStartLine, chunkExcerpt.
    unique: Option<Unique>,
}

/// Appended to the preparing message: full-text search needs no embedder.
const FULLTEXT_HINT: &str =
    " You can fall back to search({mode:'fulltext'}) which works without the embedder.";

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("search", DESCRIPTION, |ctx, args| async move {
        check_range("limit", args.limit, 1, None)?;
        let mode = args.mode.unwrap_or(Mode::Hybrid);
        let limit = args.limit.unwrap_or(20) as usize;
        let query = args.query;

        // Semantic and hybrid need the embedder. Answer at once while it
        // loads instead of blocking, which could time the client out on a
        // first-run model download or an Ollama pull.
        if mode != Mode::Fulltext
            && let Some(mut preparing) = describe_embedder_preparing(&ctx)
        {
            preparing.message.push_str(FULLTEXT_HINT);
            return Ok(serde_json::to_value(preparing)?);
        }

        let (results, hits): (Value, Vec<SearchHintHit>) = if mode == Mode::Fulltext {
            let results = ctx.search.fulltext(&query, limit)?;
            let hits = results
                .iter()
                .map(|r| hint(&r.node_id, &r.title, r.score))
                .collect();
            (serde_json::to_value(results)?, hits)
        } else {
            ctx.ensure_embedder_ready().await?;
            let chunks = args.unique == Some(Unique::Chunks);
            let results = if mode == Mode::Semantic {
                if chunks {
                    ctx.search
                        .semantic_chunks(&query, limit, SearchUnique::Chunks)
                        .await?
                } else {
                    ctx.search.semantic(&query, limit).await?
                }
            } else {
                let unique = if chunks {
                    SearchUnique::Chunks
                } else {
                    SearchUnique::Notes
                };
                ctx.search.hybrid(&query, limit, unique).await?
            };
            let hits = results
                .iter()
                .map(|r| hint(&r.node_id, &r.title, r.score))
                .collect();
            (serde_json::to_value(results)?, hits)
        };

        let context = compute_search_hints(&query, &hits);
        Ok(json!({ "data": results, "context": context }))
    })
}

fn hint(node_id: &str, title: &str, score: f64) -> SearchHintHit {
    SearchHintHit {
        node_id: node_id.to_string(),
        title: Some(title.to_string()),
        score: Some(score),
    }
}
