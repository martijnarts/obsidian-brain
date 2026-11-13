//! Temp vault and in-memory index for the maintenance tool tests. Files
//! are written to disk, parsed with the real `parse_vault`, and loaded into
//! the store with stub nodes, as the indexer does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::config::VaultConfig;
use crate::context::VaultContext;
use crate::embeddings::{Embedder, TaskType, share_embedder};
use crate::mcp::Server;
use crate::mcp::tool::ToolSpec;
use crate::pipeline::indexer::materialise_stubs;
use crate::store::Db;
use crate::store::edges::insert_edge;
use crate::store::nodes::upsert_node;
use crate::vault::parser::parse_vault;
use crate::vaults::Vaults;

struct ZeroEmbedder;

#[async_trait]
impl Embedder for ZeroEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn embed(&self, _text: &str, _task: TaskType) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }
    fn dimensions(&self) -> anyhow::Result<usize> {
        Ok(8)
    }
    fn model_identifier(&self) -> String {
        "mock/zero".into()
    }
    fn provider_name(&self) -> &'static str {
        "mock"
    }
    async fn dispose(&self) {}
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub ctx: Arc<VaultContext>,
}

impl Fixture {
    pub async fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        for (rel, content) in files {
            let abs = vault.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(abs, content).unwrap();
        }
        let db = Db::open_in_memory().unwrap();
        let parsed = parse_vault(&vault).unwrap();
        db.with(|conn| {
            for node in &parsed.nodes {
                upsert_node(conn, node).unwrap();
            }
            for edge in &parsed.edges {
                insert_edge(conn, edge).unwrap();
            }
            materialise_stubs(conn, &parsed.stub_ids).unwrap();
        });
        let config = VaultConfig::new("test", &vault, &dir.path().join("data"));
        let ctx =
            VaultContext::from_parts(config, db, share_embedder(Arc::new(ZeroEmbedder)), true);
        Self { dir, ctx }
    }

    pub fn vault(&self) -> PathBuf {
        self.ctx.config.vault_path.clone()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.vault().join(rel)
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap()
    }

    pub fn reindex_calls(&self) -> usize {
        self.ctx.background_reindex_count()
    }

    /// The tool's result through the MCP server, or its error message.
    pub async fn call(&self, spec: &ToolSpec, args: Value) -> Result<Value, String> {
        let mut args: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        args.insert("vault".into(), self.ctx.config.name.clone().into());
        let vaults = Arc::new(Vaults::single(self.ctx.clone()));
        let result = Server::new(vaults, vec![spec.clone()])
            .call(spec.name, args)
            .await;
        self.ctx.wait_for_background_reindex().await;
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect();
        if result.is_error == Some(true) {
            return Err(text.strip_prefix("Error: ").unwrap_or(&text).to_string());
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn ok(&self, spec: &ToolSpec, args: Value) -> Value {
        self.call(spec, args).await.unwrap()
    }

    pub async fn err(&self, spec: &ToolSpec, args: Value) -> String {
        self.call(spec, args).await.unwrap_err()
    }
}

/// Mode bits of a directory, set for the life of the guard.
pub struct Chmod(PathBuf);

impl Chmod {
    pub fn new(dir: &Path, mode: u32) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        Self(dir.to_path_buf())
    }
}

impl Drop for Chmod {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}
