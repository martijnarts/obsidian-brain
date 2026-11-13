//! A fake embedder, a context over a temp vault, and a way to call a tool
//! the way the server does.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::{Embedder, TaskType, share_embedder};
use obsidian_brain::store::Db;
use obsidian_brain::store::edges::insert_edge;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use serde_json::{Map, Value};
use tokio::sync::watch;

/// Zero vectors of dim 384. A gated one finishes `init` only when
/// [`TestEmbedder::release`] is called.
pub struct TestEmbedder {
    gate: Option<watch::Sender<bool>>,
}

impl TestEmbedder {
    pub fn instant() -> Self {
        Self { gate: None }
    }

    pub fn gated() -> Self {
        Self {
            gate: Some(watch::channel(false).0),
        }
    }

    pub fn release(&self) {
        if let Some(g) = &self.gate {
            g.send_replace(true);
        }
    }
}

#[async_trait]
impl Embedder for TestEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        if let Some(g) = &self.gate {
            let mut rx = g.subscribe();
            rx.wait_for(|open| *open).await?;
        }
        Ok(())
    }
    async fn embed(&self, _text: &str, _task: TaskType) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
    fn dimensions(&self) -> anyhow::Result<usize> {
        Ok(384)
    }
    fn model_identifier(&self) -> String {
        "mock/instant".into()
    }
    fn provider_name(&self) -> &'static str {
        "mock"
    }
    async fn dispose(&self) {}
}

pub struct Vault {
    pub dir: tempfile::TempDir,
    pub ctx: Arc<VaultContext>,
}

impl Vault {
    pub fn new() -> Self {
        Self::with_embedder(Arc::new(TestEmbedder::instant()))
    }

    pub fn with_embedder(embedder: Arc<dyn Embedder>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("vault")).unwrap();
        let config = VaultConfig::new("test", &dir.path().join("vault"), &dir.path().join("data"));
        let ctx = VaultContext::from_parts(
            config,
            Db::open_in_memory().unwrap(),
            share_embedder(embedder),
            true,
        );
        Self { dir, ctx }
    }

    pub fn path(&self) -> &Path {
        &self.ctx.config.vault_path
    }

    pub fn db(&self) -> &Db {
        &self.ctx.db
    }

    pub fn write(&self, rel: &str, content: &str) {
        let path = self.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path().join(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.path().join(rel).exists()
    }

    /// Index the vault as the startup index does.
    pub async fn index(&self) {
        self.ctx.ensure_embedder_ready().await.unwrap();
        self.ctx.pipeline.index(self.path()).await.unwrap();
    }

    /// A node row with no file behind it, as the TS tests seed with `upsertNode`.
    pub fn node(&self, id: &str, title: &str) {
        let mut fm = Map::new();
        if id.starts_with("_stub/") {
            fm.insert("_stub".into(), true.into());
        }
        self.db().with(|c| {
            upsert_node(
                c,
                &ParsedNode {
                    id: id.into(),
                    title: title.into(),
                    content: String::new(),
                    frontmatter: fm,
                },
            )
            .unwrap()
        });
    }

    pub fn edge(&self, source: &str, target: &str, context: &str) {
        self.db().with(|c| {
            insert_edge(
                c,
                &ParsedEdge {
                    source_id: source.into(),
                    target_id: target.into(),
                    context: context.into(),
                    target_subpath: None,
                },
            )
            .unwrap()
        });
    }

    /// Call a tool; an error comes back as the text after `Error: `.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        call(&self.ctx, name, args).await
    }

    pub async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e}"))
    }

    pub async fn err(&self, name: &str, args: Value) -> String {
        match self.call(name, args).await {
            Ok(v) => panic!("{name} succeeded: {v}"),
            Err(e) => e,
        }
    }
}

pub async fn call(ctx: &Arc<VaultContext>, name: &str, args: Value) -> Result<Value, String> {
    let spec = obsidian_brain::tools::all()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no tool {name}"));
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    spec.call(ctx.clone(), args)
        .await
        .map_err(|e| format!("{e:#}"))
}
