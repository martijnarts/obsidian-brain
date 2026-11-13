//! `list_attachments`: every non-markdown file in the vault, with how many
//! notes point at it. The index stores only `[[wikilink]]` edges, so the
//! count comes from scanning note bodies in the `nodes` table for embeds and
//! markdown links too. A link target matches an attachment the way Obsidian
//! resolves it: by vault-relative path, by path relative to the note, or by
//! bare filename (or a path suffix) anywhere in the vault.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::vault::vault_path::resolve_folder;

const DESCRIPTION: &str = "List non-markdown files (images, PDFs, ...) with their size and `references`: how many notes link to or embed each one. `unreferencedOnly` finds orphans.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Vault-relative folder to list, recursively. Default: the whole vault.
    folder: Option<String>,
    /// Keep only these extensions, case-insensitive, e.g. `["png", ".pdf"]`.
    extensions: Option<Vec<String>>,
    /// Return only attachments no note references.
    unreferenced_only: Option<bool>,
    /// Max results. Default 100.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
    /// Results to skip, for paging. Default 0.
    offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct Attachment {
    path: String,
    extension: String,
    size: u64,
    references: usize,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("list_attachments", DESCRIPTION, |ctx, args| async move {
        let limit = args.limit.unwrap_or(100) as usize;
        let offset = args.offset.unwrap_or(0) as usize;

        let scope = match &args.folder {
            Some(folder) => resolve_folder(&ctx.config.vault_path, Some(folder))?.rel,
            None => String::new(),
        };

        // References resolve against every attachment in the vault, not just
        // the listed folder: a bare filename may match a file elsewhere.
        let mut all = Vec::new();
        collect_attachments(&ctx.config.vault_path, "", &mut all)?;
        all.sort_by(|a, b| js::utf16_cmp(&a.path, &b.path));
        let paths: Vec<String> = all.iter().map(|a| a.path.clone()).collect();
        let counts = ctx.db.with(|conn| count_references(conn, &paths))?;

        let wanted: Option<Vec<String>> = args.extensions.as_ref().map(|list| {
            list.iter()
                .map(|e| {
                    let e = e.to_lowercase();
                    e.strip_prefix('.').map(str::to_string).unwrap_or(e)
                })
                .collect()
        });
        let listed: Vec<Attachment> = all
            .into_iter()
            .filter(|a| scope.is_empty() || a.path.starts_with(&format!("{scope}/")))
            .filter(|a| wanted.as_ref().is_none_or(|w| w.contains(&a.extension)))
            .map(|mut a| {
                a.references = counts.get(&a.path).copied().unwrap_or(0);
                a
            })
            .filter(|a| args.unreferenced_only != Some(true) || a.references == 0)
            .collect();

        let page: Vec<&Attachment> = listed.iter().skip(offset).take(limit).collect();
        Ok(json!({
            "total": listed.len(),
            "offset": offset,
            "limit": limit,
            "attachments": page,
        }))
    })
}

/// Non-markdown files under the vault. Skips hidden entries and does not
/// follow symlinks.
fn collect_attachments(vault: &Path, subdir: &str, out: &mut Vec<Attachment>) -> Result<()> {
    let dir = vault.join(subdir);
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "scandir", &dir, None)))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if subdir.is_empty() {
            name.clone()
        } else {
            format!("{subdir}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_attachments(vault, &rel, out)?;
        } else if kind.is_file() && !name.to_lowercase().ends_with(".md") {
            let abs = vault.join(&rel);
            let size = std::fs::metadata(&abs)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "stat", &abs, None)))?
                .len();
            let extension = match name.rfind('.') {
                Some(dot) if dot > 0 => name[dot + 1..].to_lowercase(),
                _ => String::new(),
            };
            out.push(Attachment {
                path: rel,
                extension,
                size,
                references: 0,
            });
        }
    }
    Ok(())
}

/// Map each attachment path to the number of distinct notes that reference it.
fn count_references(conn: &Connection, paths: &[String]) -> Result<HashMap<String, usize>> {
    let mut by_path: HashMap<String, String> = HashMap::new();
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for p in paths {
        by_path.insert(p.to_lowercase(), p.clone());
        by_name
            .entry(js::basename(p, "").to_lowercase())
            .or_default()
            .push(p.clone());
    }

    let mut stmt =
        conn.prepare("SELECT id, content FROM nodes WHERE id NOT LIKE '\\_stub/%' ESCAPE '\\'")?;
    let notes = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (id, content) in notes {
        let mut hits = HashSet::new();
        for target in link_targets(content.as_deref().unwrap_or("")) {
            if let Some(hit) = resolve_attachment(&target, &id, &by_path, &by_name) {
                hits.insert(hit);
            }
        }
        for hit in hits {
            *counts.entry(hit).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

static FENCED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?```").expect("valid"));
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]+`").expect("valid"));
static WIKI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").expect("valid"));
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[[^\]]*\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\s*\)"#).expect("valid")
});
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").expect("valid"));

/// Targets of `[[x]]`, `![[x]]`, `[](x)` and `![](x)`, outside code,
/// without `#heading`, `|alias` or URL schemes.
fn link_targets(markdown: &str) -> Vec<String> {
    let text = FENCED.replace_all(markdown, "");
    let text = INLINE_CODE.replace_all(&text, "");
    let mut targets = Vec::new();
    for m in WIKI.captures_iter(&text) {
        let inner = &m[1];
        let target = inner.split('|').next().unwrap_or("");
        let target = target.split(['#', '^']).next().unwrap_or("");
        targets.push(js::trim(target).to_string());
    }
    for m in MD_LINK.captures_iter(&text) {
        let raw = &m[1];
        let raw = raw.strip_prefix('<').unwrap_or(raw);
        let raw = raw.strip_suffix('>').unwrap_or(raw);
        if SCHEME.is_match(raw) {
            continue;
        }
        let target = raw.split(['#', '?']).next().unwrap_or("");
        // Keep the raw target when it holds a stray `%`.
        let target = decode_uri(target).unwrap_or_else(|| target.to_string());
        targets.push(js::trim(&target).to_string());
    }
    targets.retain(|t| !t.is_empty());
    targets
}

/// `decodeURI`: decode `%XX` escapes that form UTF-8, except those of the
/// reserved characters `;/?:@&=+$,#`. `None` where JS throws a `URIError`.
fn decode_uri(s: &str) -> Option<String> {
    const RESERVED: &[u8] = b";/?:@&=+$,#";
    let bytes = s.as_bytes();
    let hex = |i: usize| -> Option<u8> {
        if bytes.get(i) != Some(&b'%') {
            return None;
        }
        let h = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
        u8::from_str_radix(h, 16).ok()
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let lead = hex(i)?;
        if lead < 0x80 {
            if RESERVED.contains(&lead) {
                out.extend_from_slice(&bytes[i..i + 3]);
            } else {
                out.push(lead);
            }
            i += 3;
            continue;
        }
        let len = match lead {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return None,
        };
        let mut seq = vec![lead];
        for k in 1..len {
            let b = hex(i + 3 * k)?;
            if b & 0xC0 != 0x80 {
                return None;
            }
            seq.push(b);
        }
        std::str::from_utf8(&seq).ok()?;
        out.extend_from_slice(&seq);
        i += 3 * len;
    }
    String::from_utf8(out).ok()
}

fn resolve_attachment(
    target: &str,
    note_id: &str,
    by_path: &HashMap<String, String>,
    by_name: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let t = target.trim_start_matches('/').to_lowercase();
    if let Some(exact) = by_path.get(&posix_normalize(&t)) {
        return Some(exact.clone());
    }

    let note_dir = dirname(note_id).to_lowercase();
    if let Some(relative) = by_path.get(&posix_normalize(&format!("{note_dir}/{t}"))) {
        return Some(relative.clone());
    }

    let suffix = format!("/{t}");
    let candidates: Vec<&String> = by_name
        .get(&js::basename(&t, "").to_lowercase())
        .map(|list| {
            list.iter()
                .filter(|p| !t.contains('/') || p.to_lowercase().ends_with(&suffix))
                .collect()
        })
        .unwrap_or_default();
    // Several files share the name: Obsidian prefers the one beside the note.
    candidates
        .iter()
        .find(|p| dirname(p).to_lowercase() == note_dir)
        .or(candidates.first())
        .map(|p| (*p).clone())
}

/// `path.posix.normalize`.
fn posix_normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let absolute = p.starts_with('/');
    let trailing = p.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if out.is_empty() {
        if absolute {
            return "/".into();
        }
        return if trailing { "./".into() } else { ".".into() };
    }
    if trailing {
        out.push('/');
    }
    if absolute { format!("/{out}") } else { out }
}

/// `path.posix.dirname`.
fn dirname(p: &str) -> &str {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return if p.starts_with('/') { "/" } else { "." };
    }
    match trimmed.rfind('/') {
        None => ".",
        Some(0) => "/",
        Some(i) => &trimmed[..i],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_helpers_match_node() {
        assert_eq!(posix_normalize("a/./b/../c"), "a/c");
        assert_eq!(posix_normalize("./x.png"), "x.png");
        assert_eq!(posix_normalize("../x"), "../x");
        assert_eq!(posix_normalize("a/"), "a/");
        assert_eq!(posix_normalize(""), ".");
        assert_eq!(dirname("a/b/c.md"), "a/b");
        assert_eq!(dirname("c.md"), ".");
        assert_eq!(js::basename("a/b.png", ""), "b.png");
        assert_eq!(js::basename("b.png", ""), "b.png");
    }

    #[test]
    fn decode_uri_matches_js() {
        assert_eq!(decode_uri("my%20file.pdf").as_deref(), Some("my file.pdf"));
        assert_eq!(decode_uri("a%2Fb").as_deref(), Some("a%2Fb"));
        assert_eq!(decode_uri("caf%C3%A9").as_deref(), Some("café"));
        assert_eq!(decode_uri("100%"), None);
        assert_eq!(decode_uri("%C3"), None);
        assert_eq!(decode_uri("%zz"), None);
    }
}
