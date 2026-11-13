//! What every tool test uses: a temp vault with an in-memory index, the
//! fake embedder of the core tests, and a way to call a tool the way the
//! server does.

#[allow(dead_code)]
#[path = "../core/support.rs"]
mod fakes;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::{Embedder, share_embedder};
use obsidian_brain::mcp::Server;
use obsidian_brain::store::Db;
use obsidian_brain::store::edges::insert_edge;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use obsidian_brain::vault::parser::parse_vault;
use obsidian_brain::vaults::Vaults;
use serde_json::{Map, Value, json};

pub use fakes::{FakeEmbedder, context, copy_dir, fixture_vault, write};

/// The start of every argument error.
pub const INVALID: &str = "MCP error -32602: Input validation error: Invalid arguments for tool ";

/// A temp directory with the vault in `vault/` and the data in `data/`.
pub struct Vault {
    pub dir: tempfile::TempDir,
    pub ctx: Arc<VaultContext>,
}

impl Vault {
    pub fn new() -> Self {
        Self::with_embedder(Arc::new(FakeEmbedder::instant()))
    }

    pub fn with_embedder(embedder: Arc<dyn Embedder>) -> Self {
        Self::build(Db::open_in_memory().unwrap(), embedder)
    }

    fn build(db: Db, embedder: Arc<dyn Embedder>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let config = VaultConfig::new("test", &vault, &dir.path().join("data"));
        let ctx = VaultContext::from_parts(config, db, share_embedder(embedder), true);
        Self { dir, ctx }
    }

    pub fn path(&self) -> &Path {
        &self.ctx.config.vault_path
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    pub fn db(&self) -> &Db {
        &self.ctx.db
    }

    pub fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.abs(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    pub fn write_all(&self, files: &[(&str, &str)]) {
        for (rel, content) in files {
            self.write(rel, content);
        }
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.abs(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.abs(rel).exists()
    }

    /// Index the vault as the startup index does.
    pub async fn index(&self) {
        self.ctx.ensure_embedder_ready().await.unwrap();
        self.ctx.pipeline.index(self.path()).await.unwrap();
    }

    /// One node per note plus a stub per unresolved link, without the
    /// embedder, as the TS `indexVault` helper did.
    pub fn seed_index(&self) {
        let parsed = parse_vault(self.path()).unwrap();
        self.db().with(|c| {
            for node in &parsed.nodes {
                upsert_node(c, node).unwrap();
            }
            for id in &parsed.stub_ids {
                let title = &id["_stub/".len()..id.len() - 3];
                upsert_node(c, &parsed_node(id, title, "", json!({"_stub": true}))).unwrap();
            }
        });
    }

    /// A node row with no file behind it.
    pub fn node(&self, id: &str, title: &str, content: &str, frontmatter: Value) {
        let node = parsed_node(id, title, content, frontmatter);
        self.db().with(|c| upsert_node(c, &node)).unwrap();
    }

    /// [`Self::node`] with no content.
    pub fn upsert(&self, id: &str, title: &str, frontmatter: Value) {
        self.node(id, title, "", frontmatter);
    }

    /// [`Self::upsert`] with an empty frontmatter, or `_stub: true` for a
    /// `_stub/` id.
    pub fn row(&self, id: &str, title: &str) {
        let mut fm = Map::new();
        if id.starts_with("_stub/") {
            fm.insert("_stub".into(), true.into());
        }
        self.upsert(id, title, Value::Object(fm));
    }

    pub fn edge(&self, source: &str, target: &str, context: &str) {
        edge(&self.ctx, source, target, context);
    }

    /// How many background reindexes the tools queued.
    pub fn reindexes(&self) -> usize {
        self.ctx.background_reindex_count()
    }

    /// Waits for every queued reindex, so the vault can go.
    pub async fn settle(&self) {
        self.ctx.wait_for_background_reindex().await;
    }

    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        call(&self.ctx, tool, args).await
    }

    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        ok(&self.ctx, tool, args).await
    }

    pub async fn err(&self, tool: &str, args: Value) -> String {
        err(&self.ctx, tool, args).await
    }

    /// True when the arguments pass validation (the call may still fail).
    pub async fn accepts(&self, tool: &str, args: Value) -> bool {
        match self.call(tool, args).await {
            Ok(_) => true,
            Err(e) => !e.starts_with(INVALID),
        }
    }
}

/// A vault with `files`, whose embedder has the identity of the TS
/// default model.
pub fn fixture(files: &[(&str, &str)]) -> Vault {
    fixture_with_db(Db::open_in_memory().unwrap(), files)
}

/// [`fixture`] over `db`.
pub fn fixture_with_db(db: Db, files: &[(&str, &str)]) -> Vault {
    let embedder = FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "transformers.js");
    let v = Vault::build(db, Arc::new(embedder));
    v.write_all(files);
    v
}

pub fn parsed_node(id: &str, title: &str, content: &str, frontmatter: Value) -> ParsedNode {
    ParsedNode {
        id: id.into(),
        title: title.into(),
        content: content.into(),
        frontmatter: frontmatter.as_object().unwrap().clone(),
    }
}

pub fn edge(ctx: &VaultContext, source: &str, target: &str, context: &str) {
    let edge = ParsedEdge {
        source_id: source.into(),
        target_id: target.into(),
        context: context.into(),
        target_subpath: None,
    };
    ctx.db.with(|c| insert_edge(c, &edge)).unwrap();
}

/// Call a tool through the MCP server, as a client does: `vault` is added
/// when absent. Returns `isError` and the text of the result.
pub async fn call_text(ctx: &Arc<VaultContext>, tool: &str, args: Value) -> (bool, String) {
    let Value::Object(mut args) = args else {
        panic!("arguments must be an object")
    };
    if !args.contains_key("vault") {
        args.insert("vault".into(), ctx.config.name.clone().into());
    }
    let server = Server::new(
        Arc::new(Vaults::single(ctx.clone())),
        obsidian_brain::tools::all(),
    );
    let result = server.call(tool, args).await;
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    (result.is_error == Some(true), text)
}

/// [`call_text`] with the text decoded. `Ok` holds the result parsed back
/// from its JSON, or the text when it is not an object or a list. `Err`
/// holds the error text, after `Error: ` for a handler error.
pub async fn call(ctx: &Arc<VaultContext>, tool: &str, args: Value) -> Result<Value, String> {
    let (is_error, text) = call_text(ctx, tool, args).await;
    if is_error {
        return Err(text.strip_prefix("Error: ").unwrap_or(&text).to_string());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v @ (Value::Object(_) | Value::Array(_))) => Ok(v),
        _ => Ok(Value::String(text)),
    }
}

/// [`call`] that must succeed.
pub async fn ok(ctx: &Arc<VaultContext>, tool: &str, args: Value) -> Value {
    match call(ctx, tool, args.clone()).await {
        Ok(v) => v,
        Err(e) => panic!("{tool} {args} failed: {e}"),
    }
}

/// [`call`] that must fail; returns the error text.
pub async fn err(ctx: &Arc<VaultContext>, tool: &str, args: Value) -> String {
    match call(ctx, tool, args.clone()).await {
        Ok(v) => panic!("{tool} {args} succeeded: {v}"),
        Err(e) => e,
    }
}

/// The `id` of every entry of a list.
pub fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

/// The `path` of every entry of `results`.
pub fn paths(out: &Value) -> Vec<String> {
    out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect()
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
