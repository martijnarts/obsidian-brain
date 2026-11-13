//! A temp vault, an in-memory index, a stub embedder, and a way to call
//! one tool the way the server does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::{Embedder, TaskType, share_embedder};
use obsidian_brain::store::Db;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::ParsedNode;
use obsidian_brain::vault::parser::parse_vault;
use serde_json::{Value, json};

/// Zero vectors of dim 384; loads nothing.
pub struct StubEmbedder;

#[async_trait]
impl Embedder for StubEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn embed(&self, _text: &str, _task: TaskType) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
    fn dimensions(&self) -> anyhow::Result<usize> {
        Ok(384)
    }
    fn model_identifier(&self) -> String {
        "Xenova/all-MiniLM-L6-v2".into()
    }
    fn provider_name(&self) -> &'static str {
        "transformers.js"
    }
    async fn dispose(&self) {}
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub ctx: Arc<VaultContext>,
}

impl Fixture {
    pub fn vault(&self) -> PathBuf {
        self.dir.path().join("vault")
    }

    pub fn db(&self) -> &Db {
        &self.ctx.db
    }

    /// Call `tool` with `args`. `Err` holds the text a client sees after
    /// `Error: `.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        call(&self.ctx, tool, args).await
    }

    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        self.call(tool, args.clone())
            .await
            .unwrap_or_else(|e| panic!("{tool} {args} failed: {e}"))
    }

    pub async fn err(&self, tool: &str, args: Value) -> String {
        match self.call(tool, args.clone()).await {
            Ok(v) => panic!("{tool} {args} succeeded: {v}"),
            Err(e) => e,
        }
    }

    /// True when the arguments pass validation (the call may still fail).
    pub async fn accepts(&self, tool: &str, args: Value) -> bool {
        match self.call(tool, args).await {
            Ok(_) => true,
            Err(e) => !e.starts_with("Input validation error"),
        }
    }

    pub fn write(&self, files: &[(&str, &str)]) {
        write(&self.vault(), files);
    }

    /// Index the vault on disk: one node per note plus a stub per
    /// unresolved link, as the TS `indexVault` helper did.
    pub fn index(&self) {
        let parsed = parse_vault(&self.vault()).unwrap();
        self.db().with(|c| {
            for node in &parsed.nodes {
                upsert_node(c, node).unwrap();
            }
            for id in &parsed.stub_ids {
                let title = &id["_stub/".len()..id.len() - 3];
                upsert_node(c, &node(id, title, json!({"_stub": true}))).unwrap();
            }
        });
    }

    pub fn upsert(&self, id: &str, title: &str, frontmatter: Value) {
        self.db()
            .with(|c| upsert_node(c, &node(id, title, frontmatter)))
            .unwrap();
    }
}

pub fn node(id: &str, title: &str, frontmatter: Value) -> ParsedNode {
    ParsedNode {
        id: id.into(),
        title: title.into(),
        content: String::new(),
        frontmatter: frontmatter.as_object().unwrap().clone(),
    }
}

pub fn write(root: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
}

/// A fixture over `db`, with `files` in its vault.
pub fn fixture_with_db(db: Db, files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    write(&vault, files);
    let config = VaultConfig::new("test", &vault, &dir.path().join("data"));
    let ctx = VaultContext::from_parts(config, db, share_embedder(Arc::new(StubEmbedder)), true);
    Fixture { dir, ctx }
}

pub fn fixture(files: &[(&str, &str)]) -> Fixture {
    fixture_with_db(Db::open_in_memory().unwrap(), files)
}

pub async fn call(ctx: &Arc<VaultContext>, tool: &str, args: Value) -> Result<Value, String> {
    let spec = obsidian_brain::tools::all()
        .into_iter()
        .find(|t| t.name == tool)
        .unwrap_or_else(|| panic!("no tool {tool}"));
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    spec.call(ctx.clone(), args)
        .await
        .map_err(|e| format!("{e:#}"))
}

pub fn ids(out: &Value) -> Vec<String> {
    out.as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

pub fn paths(out: &Value) -> Vec<String> {
    out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect()
}
