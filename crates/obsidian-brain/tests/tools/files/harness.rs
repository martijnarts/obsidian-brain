//! A temp vault on disk, an in-memory index, and a caller that runs one
//! tool the way the MCP server does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::share_embedder;
use obsidian_brain::store::Db;
use obsidian_brain::store::edges::insert_edge;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use serde_json::Value;

use crate::support::FakeEmbedder;

pub struct Harness {
    _dir: tempfile::TempDir,
    pub vault: PathBuf,
    pub ctx: Arc<VaultContext>,
}

impl Harness {
    /// Must run inside a Tokio runtime.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let config = VaultConfig::new("test", &vault, &dir.path().join("data"));
        let ctx = VaultContext::from_parts(
            config,
            Db::open_in_memory().unwrap(),
            share_embedder(Arc::new(FakeEmbedder::instant())),
            true,
        );
        Self {
            _dir: dir,
            vault,
            ctx,
        }
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.vault.join(rel)
    }

    pub fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.abs(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.abs(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.abs(rel).exists()
    }

    pub fn node(&self, id: &str, title: &str, content: &str, frontmatter: Value) {
        let node = ParsedNode {
            id: id.into(),
            title: title.into(),
            content: content.into(),
            frontmatter: frontmatter.as_object().unwrap().clone(),
        };
        self.ctx.db.with(|c| upsert_node(c, &node)).unwrap();
    }

    pub fn edge(&self, source: &str, target: &str) {
        let edge = ParsedEdge {
            source_id: source.into(),
            target_id: target.into(),
            context: String::new(),
            target_subpath: None,
        };
        self.ctx.db.with(|c| insert_edge(c, &edge)).unwrap();
    }

    /// How many background reindexes the tools queued.
    pub fn reindexes(&self) -> usize {
        self.ctx.background_reindex_count()
    }

    /// Runs `tool` with `args`: the payload, or the error text.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        let spec = obsidian_brain::tools::all()
            .into_iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("no tool {tool}"));
        let args = args.as_object().cloned().unwrap_or_default();
        spec.call(self.ctx.clone(), args)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        match self.call(tool, args).await {
            Ok(v) => v,
            Err(e) => panic!("expected success, got: {e}"),
        }
    }

    pub async fn err(&self, tool: &str, args: Value) -> String {
        match self.call(tool, args).await {
            Ok(v) => panic!("expected an error, got: {v}"),
            Err(e) => e,
        }
    }

    /// Waits for every queued reindex, so the vault can go.
    pub async fn settle(&self) {
        self.ctx.wait_for_background_reindex().await;
    }
}

/// A fresh directory outside every vault.
pub fn outside() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

pub fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// Fails unless `text` contains every one of `parts`.
#[track_caller]
pub fn assert_contains(text: &str, parts: &[&str]) {
    for p in parts {
        assert!(text.contains(p), "{text:?} does not contain {p:?}");
    }
}

pub const INVALID: &str = "Input validation error";
