//! Parse notes into nodes, outbound edges and stub targets.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::{Captures, Regex};
use serde_json::Value;

use super::frontmatter;
use super::vault_path::read_utf8;
use super::wiki_links::{
    StemLookup, build_stem_lookup, extract_wiki_links, resolve_link, strip_code,
};
use crate::types::{Frontmatter, ParsedEdge, ParsedNode};
use crate::util::errors::node_fs_error_message;
use crate::util::js;

const EXCLUDED_DIRS: [&str; 3] = [".obsidian", "_FileOrganizer2000", "attachments"];

static PARAGRAPH_BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n\n+").expect("valid pattern"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("valid pattern"));
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#([a-zA-Z][A-Za-z0-9_\-/]*)").expect("valid pattern"));
static INLINE_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?mR)^([A-Za-z_][A-Za-z0-9_-]*)::[ \t]*(.+?)[ \t]*$").expect("valid pattern")
});

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParseResult {
    pub nodes: Vec<ParsedNode>,
    pub edges: Vec<ParsedEdge>,
    /// Unique, in first-seen order.
    pub stub_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFile {
    pub node: ParsedNode,
    pub edges: Vec<ParsedEdge>,
    /// Unique, in first-seen order.
    pub stub_ids: Vec<String>,
}

/// `basename(relPath, '.md')`.
fn basename_md(rel_path: &str) -> &str {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match base.strip_suffix(".md") {
        Some(stem) if !stem.is_empty() => stem,
        _ => base,
    }
}

/// The node title: the frontmatter `title` (any non-null value, as its JS
/// string), else the file name without `.md`.
pub fn node_title(frontmatter: &Frontmatter, rel_path: &str) -> String {
    match frontmatter.get("title") {
        None | Some(Value::Null) => basename_md(rel_path).to_string(),
        Some(v) => js::to_js_string(v),
    }
}

/// Parse a single file's raw contents into a node, its outbound edges and
/// the stub targets those edges point at. Pure: no I/O. Shared by
/// `parse_vault` and the watcher's single-file reindex.
///
/// `node.content` is the body without frontmatter, as gray-matter's
/// `content` was in the TypeScript server.
pub fn parse_file_from_content(
    rel_path: &str,
    raw: &str,
    stem_lookup: &StemLookup,
    all_paths: &HashSet<String>,
    mut warned_ambiguous: Option<&mut HashSet<String>>,
) -> ParsedFile {
    let (fm, content) = match frontmatter::parse(raw) {
        Ok(p) => (p.data, p.content.to_string()),
        Err(_) => {
            tracing::warn!(
                path = rel_path,
                "Malformed frontmatter in {rel_path}, treating as plain markdown"
            );
            (Frontmatter::new(), raw.to_string())
        }
    };

    let title = node_title(&fm, rel_path);
    let inline_tags = extract_inline_tags(&content);
    // YAML frontmatter wins over inline `key:: value`: explicit structured
    // metadata takes precedence over body-level convenience fields.
    let mut frontmatter = extract_inline_fields(&content);
    for (k, v) in fm {
        frontmatter.insert(k, v);
    }
    js::js_object_order(&mut frontmatter);
    if !inline_tags.is_empty() {
        frontmatter.insert("inline_tags".into(), Value::from(inline_tags));
    }

    let mut edges = Vec::new();
    let mut stub_ids: Vec<String> = Vec::new();
    let paragraphs: Vec<&str> = PARAGRAPH_BREAK.split(&content).collect();

    for link in extract_wiki_links(&content) {
        // Split the heading (#) or block (^) suffix off, so only the bare stem
        // is resolved and used as the stub id. The fragment rides on the edge.
        let (bare_raw, fragment) = match link.raw.find(['#', '^']) {
            Some(i) => (&link.raw[..i], Some(link.raw[i + 1..].to_string())),
            None => (link.raw.as_str(), None),
        };

        let target_id = resolve_link(
            bare_raw,
            stem_lookup,
            Some(all_paths),
            Some(rel_path),
            warned_ambiguous.as_deref_mut(),
        );
        let resolved = target_id
            .clone()
            .unwrap_or_else(|| format!("_stub/{bare_raw}.md"));
        if target_id.is_none() && !stub_ids.contains(&resolved) {
            stub_ids.push(resolved.clone());
        }

        let open = format!("[[{}", link.raw);
        let shown = link.display.as_deref().unwrap_or(&link.raw);
        let paragraph = paragraphs
            .iter()
            .find(|p| p.contains(&open))
            .or_else(|| paragraphs.iter().find(|p| p.contains(shown)))
            .copied()
            .unwrap_or("");

        // Keep the edge context free of wiki-link syntax for its own target:
        // a trailing `[[Target]]` (what link_notes appends) is dropped, and an
        // inline one is replaced by its display text or raw link text.
        let esc = regex::escape(&link.raw);
        let trailing =
            Regex::new(&format!(r"\s*!?\[\[{esc}(?:\|[^\]]+)?\]\]\s*$")).expect("escaped pattern");
        let inline =
            Regex::new(&format!(r"!?\[\[{esc}(?:\|([^\]]+))?\]\]")).expect("escaped pattern");
        let context = trailing.replace(paragraph, "");
        let context = inline.replace_all(&context, |c: &Captures| {
            c.get(1)
                .map_or(link.raw.clone(), |d| d.as_str().to_string())
        });
        let context = SPACES.replace_all(&context, " ");

        edges.push(ParsedEdge {
            source_id: rel_path.to_string(),
            target_id: resolved,
            context: js::trim(&context).to_string(),
            target_subpath: fragment,
        });
    }

    ParsedFile {
        node: ParsedNode {
            id: rel_path.to_string(),
            title,
            content,
            frontmatter,
        },
        edges,
        stub_ids,
    }
}

/// Read and parse a single file on disk, resolving links against
/// `all_paths` (typically the sync table).
pub fn parse_single_file(
    vault_path: &Path,
    rel_path: &str,
    all_paths: &[String],
) -> Result<ParsedFile> {
    let abs = vault_path.join(rel_path);
    let raw =
        read_utf8(&abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
    let stem_lookup = build_stem_lookup(all_paths);
    let set: HashSet<String> = all_paths.iter().cloned().collect();
    Ok(parse_file_from_content(
        rel_path,
        &raw,
        &stem_lookup,
        &set,
        None,
    ))
}

pub fn parse_vault(vault_path: &Path) -> Result<ParseResult> {
    let md_paths = collect_markdown_files(vault_path, "")?;
    let stem_lookup = build_stem_lookup(&md_paths);
    let set: HashSet<String> = md_paths.iter().cloned().collect();
    // One warning per distinct ambiguous stem per pass.
    let mut warned = HashSet::new();
    let mut result = ParseResult::default();
    let mut stubs = HashSet::new();
    for rel in &md_paths {
        let abs = vault_path.join(rel);
        let raw =
            read_utf8(&abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
        let parsed = parse_file_from_content(rel, &raw, &stem_lookup, &set, Some(&mut warned));
        result.nodes.push(parsed.node);
        result.edges.extend(parsed.edges);
        for stub in parsed.stub_ids {
            if stubs.insert(stub.clone()) {
                result.stub_ids.push(stub);
            }
        }
    }
    Ok(result)
}

/// Body `#tags`, unique, in first-seen order. A `#` right after a word
/// character is not a tag.
pub fn extract_inline_tags(content: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for m in INLINE_TAG.captures_iter(content) {
        let at = m.get(0).expect("whole match").start();
        if content[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        if !tags.iter().any(|t| t == &m[1]) {
            tags.push(m[1].to_string());
        }
    }
    tags
}

/// Dataview-style inline fields: `key:: value` at the start of a line. Covers
/// the common metadata case (status, priority) without Obsidian or Dataview
/// running. Values land flat on the node's frontmatter so filters and search
/// see them. Bracketed `[key:: value]` fields and Dataview's task metadata
/// are not handled. The first occurrence of a key wins.
fn extract_inline_fields(content: &str) -> Frontmatter {
    let mut fields = Frontmatter::new();
    let without_code = strip_code(content);
    for m in INLINE_FIELD.captures_iter(&without_code) {
        if !fields.contains_key(&m[1]) {
            fields.insert(m[1].to_string(), Value::String(m[2].to_string()));
        }
    }
    fields
}

/// Every `.md` file under `subdir`, vault-relative, in name order (as Node's
/// `readdir` lists them). Hidden entries and the excluded folders are
/// skipped; symlinks are not followed into.
pub fn collect_markdown_files(vault_path: &Path, subdir: &str) -> Result<Vec<String>> {
    let dir = if subdir.is_empty() {
        vault_path.to_path_buf()
    } else {
        vault_path.join(subdir)
    };
    let mut entries: Vec<(String, std::fs::FileType)> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "scandir", &dir, None)))?
        .map(|e| {
            let e = e?;
            Ok((e.file_name().to_string_lossy().into_owned(), e.file_type()?))
        })
        .collect::<std::io::Result<_>>()?;
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let mut results = Vec::new();
    for (name, kind) in entries {
        if name.starts_with('.') || (kind.is_dir() && EXCLUDED_DIRS.contains(&name.as_str())) {
            continue;
        }
        let rel = if subdir.is_empty() {
            name.clone()
        } else {
            format!("{subdir}/{name}")
        };
        if kind.is_dir() {
            results.extend(collect_markdown_files(vault_path, &rel)?);
        } else if name.ends_with(".md") {
            results.push(rel);
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_vault() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/vault")
    }

    #[test]
    fn finds_all_md_files_and_skips_excluded_directories() {
        let ids: Vec<String> = parse_vault(&fixture_vault())
            .unwrap()
            .nodes
            .into_iter()
            .map(|n| n.id)
            .collect();
        for want in [
            "People/Alice Smith.md",
            "People/Bob Jones.md",
            "Concepts/Widget Theory.md",
            "orphan.md",
        ] {
            assert!(ids.contains(&want.to_string()), "{want}");
        }
        assert!(
            ids.iter()
                .all(|id| !id.starts_with(".obsidian/") && !id.starts_with("attachments/"))
        );
    }

    #[test]
    fn parses_frontmatter_and_titles() {
        let nodes = parse_vault(&fixture_vault()).unwrap().nodes;
        let alice = nodes
            .iter()
            .find(|n| n.id == "People/Alice Smith.md")
            .unwrap();
        assert_eq!(alice.title, "Alice Smith");
        assert_eq!(alice.frontmatter["type"], "person");
        assert!(
            alice.frontmatter["aliases"]
                .as_array()
                .unwrap()
                .contains(&json!("A. Smith"))
        );
        assert_eq!(
            nodes.iter().find(|n| n.id == "no-title.md").unwrap().title,
            "no-title"
        );
    }

    #[test]
    fn extracts_resolved_edges_stubs_and_inline_tags() {
        let r = parse_vault(&fixture_vault()).unwrap();
        let e = r
            .edges
            .iter()
            .find(|e| {
                e.source_id == "People/Alice Smith.md" && e.target_id == "Concepts/Widget Theory.md"
            })
            .unwrap();
        assert!(e.context.contains("Widget Theory"));
        assert!(
            r.edges
                .iter()
                .any(|e| e.target_id.contains("Nonexistent Page"))
        );
        assert!(!r.stub_ids.is_empty());
        let bob = r
            .nodes
            .iter()
            .find(|n| n.id == "People/Bob Jones.md")
            .unwrap();
        let tags = bob.frontmatter["inline_tags"].as_array().unwrap();
        assert!(tags.contains(&json!("research")) && tags.contains(&json!("published")));
    }

    fn parse(raw: &str) -> ParsedFile {
        parse_file_from_content("Example.md", raw, &StemLookup::new(), &HashSet::new(), None)
    }

    #[test]
    fn parses_inline_dataview_fields() {
        let node = parse(
            "---\ntitle: Example\n---\n\nstatus:: reading\npriority:: high\n\nSome body text.",
        )
        .node;
        assert_eq!(node.frontmatter["status"], "reading");
        assert_eq!(node.frontmatter["priority"], "high");
        let node = parse("# Title\n\n```ts\nconst x:: number = 1;\n```\n\nreal:: field").node;
        assert_eq!(node.frontmatter["real"], "field");
        assert!(!node.frontmatter.contains_key("x"));
        assert_eq!(
            parse("---\nstatus: done\n---\n\nstatus:: reading")
                .node
                .frontmatter["status"],
            "done"
        );
        assert_eq!(
            parse("a:: b \r\nc:: d\r\n").node.frontmatter,
            json!({"a": "b", "c": "d"}).as_object().unwrap().clone()
        );
    }

    #[test]
    fn malformed_frontmatter_falls_back_to_plain_markdown() {
        let p = parse("---\ntitle: unclosed\nBody with no closing marker\n");
        assert_eq!(p.node.title, "Example");
        assert!(p.node.content.contains("Body with no closing marker"));
        let p = parse("---\ntitle: unclosed\nSee [[Target]] for details.\n");
        assert!(p.edges.iter().any(|e| e.target_id.contains("Target")));
    }

    #[test]
    fn tab_indented_yaml_still_parses() {
        let p = parse("---\ntitle: Test\nnested:\n\tvalue: ok\n---\n\nBody.");
        assert_eq!(p.node.title, "Test");
        assert_eq!(p.node.content, "\nBody.");
    }

    #[test]
    fn edges_carry_subpaths_and_clean_context() {
        let lookup = build_stem_lookup(&["BMW.md"]);
        let paths: HashSet<String> = ["BMW.md".to_string()].into();
        let p = parse_file_from_content(
            "n.md",
            "I drive a [[BMW#Specs|beemer]]   daily.\n\nsee related note [[Missing^b1]]",
            &lookup,
            &paths,
            None,
        );
        assert_eq!(p.edges[0].target_id, "BMW.md");
        assert_eq!(p.edges[0].target_subpath.as_deref(), Some("Specs"));
        assert_eq!(p.edges[0].context, "I drive a beemer daily.");
        assert_eq!(p.edges[1].target_id, "_stub/Missing.md");
        assert_eq!(p.edges[1].context, "see related note");
        assert_eq!(p.stub_ids, ["_stub/Missing.md"]);
    }

    #[test]
    fn titles_from_non_string_values() {
        assert_eq!(parse("---\ntitle: 2024\n---\n").node.title, "2024");
        assert_eq!(parse("---\ntitle: ~\n---\n").node.title, "Example");
        assert_eq!(parse("---\ntitle: ''\n---\n").node.title, "");
    }

    #[test]
    fn inline_tags_skip_word_prefixed_hashes() {
        assert_eq!(extract_inline_tags("#a x#b (#c/d-e) #1 #a"), ["a", "c/d-e"]);
    }
}
