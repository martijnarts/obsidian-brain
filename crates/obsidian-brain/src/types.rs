//! The records that pass between the vault parser, the store and the tools.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Frontmatter as JSON. YAML dates become ISO strings, as `JSON.stringify`
/// made them in the TypeScript server.
pub type Frontmatter = Map<String, Value>;

/// One note, or one stub for an unresolved wiki-link target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedNode {
    /// Path relative to the vault root, with `/` separators.
    pub id: String,
    /// From the frontmatter `title`, else the file name without extension.
    pub title: String,
    /// The markdown body, without the frontmatter block.
    pub content: String,
    pub frontmatter: Frontmatter,
}

/// One outbound link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedEdge {
    pub source_id: String,
    /// The resolved path, without a `#heading` or `^block` suffix.
    pub target_id: String,
    /// The paragraph that holds the link.
    pub context: String,
    /// The `#heading` or `^block` suffix without its `#` or `^`, as in
    /// Obsidian's `LinkCache.subpath`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_subpath: Option<String>,
}

/// One embedding-sized piece of a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// The ordinal within the note.
    pub chunk_index: usize,
    pub heading: Option<String>,
    pub heading_level: Option<u8>,
    pub content: String,
    /// A fingerprint over heading and content, so an unchanged chunk is not
    /// embedded again.
    pub content_hash: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub node_id: String,
    pub title: String,
    pub score: f64,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathEdge {
    pub source_id: String,
    pub target_id: String,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    /// The node ids along the path, in order.
    pub nodes: Vec<String>,
    /// The edge context of each hop.
    pub edges: Vec<PathEdge>,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgraphNode {
    pub id: String,
    pub title: String,
    pub frontmatter: Frontmatter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgraphResult {
    pub nodes: Vec<SubgraphNode>,
    pub edges: Vec<PathEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchType {
    Id,
    Exact,
    CaseInsensitive,
    Alias,
    Substring,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameMatch {
    pub node_id: String,
    pub title: String,
    pub match_type: MatchType,
}
