//! Create notes, append to them and add links, keeping the store in step.
//!
//! The methods take the store connection from the caller, so a tool can
//! run them inside its own lock or transaction.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use rusqlite::Connection;
use serde_json::Value;

use super::frontmatter;
use super::parser::node_title;
use super::vault_path::{read_utf8, resolve_vault_path};
use crate::store::edges::insert_edge;
use crate::store::nodes::upsert_node;
use crate::types::{Frontmatter, ParsedEdge, ParsedNode};
use crate::util::errors::node_fs_error_message;
use crate::util::js;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateNodeOptions {
    pub title: String,
    pub directory: Option<String>,
    pub frontmatter: Frontmatter,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct VaultWriter {
    vault_path: PathBuf,
}

impl VaultWriter {
    pub fn new(vault_path: impl Into<PathBuf>) -> Self {
        Self {
            vault_path: vault_path.into(),
        }
    }

    pub fn vault_path(&self) -> &Path {
        &self.vault_path
    }

    /// Write a new note and index it. Returns its vault-relative path.
    pub fn create_node(&self, conn: &Connection, opts: &CreateNodeOptions) -> Result<String> {
        // The directory and the title both come from the client, and the title
        // becomes the file name, so the guard checks the whole path.
        let filename = format!("{}.md", opts.title);
        let target = match opts.directory.as_deref().filter(|d| !d.is_empty()) {
            Some(dir) => format!("{dir}/{filename}"),
            None => filename,
        };
        let resolved = resolve_vault_path(&self.vault_path, &target)?;
        if let Some(parent) = resolved.abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "mkdir", parent, None)))?;
        }
        if resolved.abs.exists() {
            bail!("File already exists: {}", resolved.rel);
        }

        // Default: inject `title` matching the note's title. A caller that
        // passes `title: null` opts out (no key is written); a caller that
        // passes its own title keeps it.
        let mut fm = opts.frontmatter.clone();
        match fm.get("title") {
            None => {
                fm.insert("title".into(), Value::String(opts.title.clone()));
            }
            Some(Value::Null) => {
                fm.shift_remove("title");
            }
            Some(_) => {}
        }
        js::js_object_order(&mut fm);
        let file_content = frontmatter::stringify(&opts.content, &fm);
        std::fs::write(&resolved.abs, file_content)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &resolved.abs, None)))?;

        self.index_file(conn, &resolved.rel)?;
        Ok(resolved.rel)
    }

    /// Append `content` to an existing note and index it again.
    pub fn annotate_node(&self, conn: &Connection, node_id: &str, content: &str) -> Result<()> {
        let abs = resolve_vault_path(&self.vault_path, node_id)?.abs;
        if !abs.exists() {
            bail!("Node not found: {node_id}");
        }
        append(&abs, content)?;
        self.index_file(conn, node_id)
    }

    /// Append `\n<context> [[target_ref]]` to the source note, index it again
    /// and add the edge.
    pub fn add_link(
        &self,
        conn: &Connection,
        source_id: &str,
        target_ref: &str,
        context: &str,
    ) -> Result<()> {
        let abs = resolve_vault_path(&self.vault_path, source_id)?.abs;
        if !abs.exists() {
            bail!("Source node not found: {source_id}");
        }
        append(&abs, &format!("\n{context} [[{target_ref}]]"))?;
        self.index_file(conn, source_id)?;

        let target_id = if target_ref.ends_with(".md") {
            target_ref.to_string()
        } else {
            format!("{target_ref}.md")
        };
        insert_edge(
            conn,
            &ParsedEdge {
                source_id: source_id.to_string(),
                target_id,
                context: context.to_string(),
                target_subpath: None,
            },
        )?;
        Ok(())
    }

    fn index_file(&self, conn: &Connection, rel_path: &str) -> Result<()> {
        let abs = resolve_vault_path(&self.vault_path, rel_path)?.abs;
        let raw =
            read_utf8(&abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
        let (fm, content) = match frontmatter::parse(&raw) {
            Ok(p) => (p.data, p.content.to_string()),
            Err(_) => (Frontmatter::new(), raw.clone()),
        };
        let title = node_title(&fm, rel_path);
        upsert_node(
            conn,
            &ParsedNode {
                id: rel_path.to_string(),
                title,
                content,
                frontmatter: fm,
            },
        )?;
        Ok(())
    }
}

fn append(abs: &Path, content: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(abs)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "open", abs, None)))?;
    file.write_all(content.as_bytes())
        .map_err(|e| anyhow!(node_fs_error_message(&e, "write", abs, None)))?;
    Ok(())
}
