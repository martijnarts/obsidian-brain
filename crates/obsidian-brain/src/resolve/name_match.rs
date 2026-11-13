//! Matching a name against every node, by priority.

use rusqlite::Connection;
use serde_json::Value;

use crate::store::Result;
use crate::types::{Frontmatter, MatchType, NameMatch};

struct Row {
    id: String,
    title: String,
    frontmatter: String,
}

fn matches(rows: &[&Row], match_type: MatchType) -> Vec<NameMatch> {
    rows.iter()
        .map(|n| NameMatch {
            node_id: n.id.clone(),
            title: n.title.clone(),
            match_type,
        })
        .collect()
}

/// The string entries of `frontmatter.aliases`. A single string counts as
/// one alias.
fn aliases(frontmatter: &Frontmatter) -> Vec<&str> {
    match frontmatter.get("aliases") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(s)) => vec![s.as_str()],
        _ => Vec::new(),
    }
}

/// Every node that matches `name`, from the first tier that matches
/// anything: id (with or without `.md`), exact title, file basename,
/// case-insensitive title, alias, then title substring.
pub fn resolve_node_name(name: &str, conn: &Connection) -> Result<Vec<NameMatch>> {
    let mut stmt = conn.prepare("SELECT id, title, frontmatter FROM nodes")?;
    let all: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                title: r.get(1)?,
                frontmatter: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let name_with_md = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    let id_match: Vec<&Row> = all
        .iter()
        .filter(|n| n.id == name || n.id == name_with_md)
        .collect();
    if !id_match.is_empty() {
        return Ok(matches(&id_match, MatchType::Id));
    }

    let exact: Vec<&Row> = all.iter().filter(|n| n.title == name).collect();
    if !exact.is_empty() {
        return Ok(matches(&exact, MatchType::Exact));
    }

    // Obsidian users name notes by the last path segment without `.md`,
    // which can differ from the indexed title after a move.
    let stripped = name.strip_suffix(".md").unwrap_or(name);
    let stripped_md = format!("{stripped}.md");
    let basename: Vec<&Row> = all
        .iter()
        .filter(|n| {
            let base = n.id.rsplit('/').next().unwrap_or(&n.id);
            base == name || base == stripped_md
        })
        .collect();
    if !basename.is_empty() {
        return Ok(matches(&basename, MatchType::Id));
    }

    let lower = name.to_lowercase();
    let case_insensitive: Vec<&Row> = all
        .iter()
        .filter(|n| n.title.to_lowercase() == lower)
        .collect();
    if !case_insensitive.is_empty() {
        return Ok(matches(&case_insensitive, MatchType::CaseInsensitive));
    }

    let mut alias_matches = Vec::new();
    for n in &all {
        let fm: Frontmatter = serde_json::from_str(&n.frontmatter)?;
        if aliases(&fm).iter().any(|a| a.to_lowercase() == lower) {
            alias_matches.push(n);
        }
    }
    if !alias_matches.is_empty() {
        return Ok(matches(&alias_matches, MatchType::Alias));
    }

    let substring: Vec<&Row> = all
        .iter()
        .filter(|n| n.title.to_lowercase().contains(&lower))
        .collect();
    Ok(matches(&substring, MatchType::Substring))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::open_db;
    use crate::store::nodes::upsert_node;
    use crate::types::ParsedNode;
    use serde_json::json;

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        for (id, title, fm) in [
            (
                "People/Alice Smith.md",
                "Alice Smith",
                json!({"aliases": ["A. Smith"]}),
            ),
            (
                "Concepts/Widget Theory.md",
                "Widget Theory",
                json!({"aliases": ["Widget Framework", "WT"]}),
            ),
        ] {
            upsert_node(
                &db,
                &ParsedNode {
                    id: id.into(),
                    title: title.into(),
                    content: String::new(),
                    frontmatter: fm.as_object().unwrap().clone(),
                },
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn matches_exact_title() {
        let m = resolve_node_name("Alice Smith", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::Exact);
    }

    #[test]
    fn matches_case_insensitively() {
        let m = resolve_node_name("alice smith", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::CaseInsensitive);
    }

    #[test]
    fn matches_aliases() {
        let m = resolve_node_name("WT", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::Alias);
        assert_eq!(m[0].node_id, "Concepts/Widget Theory.md");
    }

    #[test]
    fn matches_a_single_string_alias() {
        let db = setup();
        upsert_node(
            &db,
            &ParsedNode {
                id: "x.md".into(),
                title: "X".into(),
                content: String::new(),
                frontmatter: json!({"aliases": "Ex"}).as_object().unwrap().clone(),
            },
        )
        .unwrap();
        let m = resolve_node_name("ex", &db).unwrap();
        assert_eq!(m[0].node_id, "x.md");
        assert_eq!(m[0].match_type, MatchType::Alias);
    }

    #[test]
    fn matches_substrings() {
        let m = resolve_node_name("Widget", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::Substring);
    }

    #[test]
    fn returns_empty_for_no_match() {
        assert!(
            resolve_node_name("Nonexistent", &setup())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn matches_by_exact_node_id() {
        let m = resolve_node_name("People/Alice Smith.md", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::Id);
        assert_eq!(m[0].node_id, "People/Alice Smith.md");
    }

    #[test]
    fn matches_by_node_id_without_extension() {
        let m = resolve_node_name("Concepts/Widget Theory", &setup()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].match_type, MatchType::Id);
        assert_eq!(m[0].node_id, "Concepts/Widget Theory.md");
    }
}
