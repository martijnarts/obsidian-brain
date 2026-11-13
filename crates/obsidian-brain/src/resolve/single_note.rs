//! Resolving a name to exactly one note.

use rusqlite::Connection;

use super::name_match::resolve_node_name;
use crate::store::StoreError;
use crate::types::MatchType;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("\"{0}\" is an unresolved link target, not a note on disk.")]
    Stub(String),
    #[error("No note found matching \"{0}\"")]
    NotFound(String),
    #[error("Multiple notes match \"{name}\". Please be more specific. Candidates:\n{candidates}")]
    Ambiguous { name: String, candidates: String },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Resolve `name` to one indexed path. Fails when nothing matches, or when
/// the best match is a loose one (substring, case-insensitive, alias)
/// shared by several notes.
///
/// Unresolved link targets (`_stub/` ids) have no file on disk, so they
/// are refused unless `allow_stubs` is set. Index-only tools (graph
/// queries, index reads, delete) set it.
pub fn resolve_single_note(
    name: &str,
    conn: &Connection,
    allow_stubs: bool,
) -> Result<String, ResolveError> {
    let mut matches = resolve_node_name(name, conn)?;
    if !allow_stubs {
        let first_stub = matches.first().map(|m| m.node_id.clone());
        matches.retain(|m| !m.node_id.starts_with("_stub/"));
        if matches.is_empty()
            && let Some(id) = first_stub
        {
            return Err(ResolveError::Stub(id));
        }
    }
    let Some(first) = matches.first() else {
        return Err(ResolveError::NotFound(name.to_string()));
    };
    let ambiguous = matches.len() > 1
        && matches!(
            first.match_type,
            MatchType::Substring | MatchType::CaseInsensitive | MatchType::Alias
        );
    if ambiguous {
        let candidates = matches
            .iter()
            .take(10)
            .map(|m| format!("- {} ({})", m.title, m.node_id))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ResolveError::Ambiguous {
            name: name.to_string(),
            candidates,
        });
    }
    Ok(first.node_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::open_db;
    use crate::store::nodes::upsert_node;
    use crate::types::ParsedNode;
    use serde_json::{Value, json};

    fn node(db: &Connection, id: &str, title: &str, fm: Value) {
        upsert_node(
            db,
            &ParsedNode {
                id: id.into(),
                title: title.into(),
                content: String::new(),
                frontmatter: fm.as_object().unwrap().clone(),
            },
        )
        .unwrap();
    }

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        node(
            &db,
            "Projects/Alpha one.md",
            "Alpha one",
            json!({"aliases": ["A1"]}),
        );
        node(
            &db,
            "Projects/Alpha two.md",
            "Alpha two",
            json!({"aliases": ["a1"]}),
        );
        node(&db, "Notes/Beta.md", "Beta", json!({}));
        node(&db, "_stub/Ghost.md", "Ghost", json!({"_stub": true}));
        db
    }

    fn err(name: &str, db: &Connection, allow_stubs: bool) -> String {
        resolve_single_note(name, db, allow_stubs)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn resolves_an_exact_path() {
        let db = setup();
        assert_eq!(
            resolve_single_note("Notes/Beta.md", &db, false).unwrap(),
            "Notes/Beta.md"
        );
        assert_eq!(
            resolve_single_note("Notes/Beta", &db, false).unwrap(),
            "Notes/Beta.md"
        );
    }

    #[test]
    fn resolves_an_exact_title_and_a_basename() {
        let db = setup();
        assert_eq!(
            resolve_single_note("Beta", &db, false).unwrap(),
            "Notes/Beta.md"
        );
        assert_eq!(
            resolve_single_note("Alpha one.md", &db, false).unwrap(),
            "Projects/Alpha one.md"
        );
    }

    #[test]
    fn resolves_a_fuzzy_match_with_one_candidate() {
        let db = setup();
        assert_eq!(
            resolve_single_note("beta", &db, false).unwrap(),
            "Notes/Beta.md"
        );
        assert_eq!(
            resolve_single_note("pha tw", &db, false).unwrap(),
            "Projects/Alpha two.md"
        );
    }

    #[test]
    fn lists_candidates_when_ambiguous() {
        let db = setup();
        assert_eq!(
            err("alpha", &db, false),
            "Multiple notes match \"alpha\". Please be more specific. Candidates:\n\
             - Alpha one (Projects/Alpha one.md)\n- Alpha two (Projects/Alpha two.md)"
        );
        assert!(err("A1", &db, false).starts_with("Multiple notes match \"A1\""));
    }

    #[test]
    fn caps_the_candidate_list_at_ten() {
        let db = setup();
        for i in 0..12 {
            node(
                &db,
                &format!("Many/Gamma {i}.md"),
                &format!("Gamma {i}"),
                json!({}),
            );
        }
        let msg = err("gamma", &db, false);
        assert_eq!(msg.lines().filter(|l| l.starts_with("- ")).count(), 10);
    }

    #[test]
    fn errors_when_nothing_matches() {
        assert_eq!(
            err("nothing here", &setup(), false),
            "No note found matching \"nothing here\""
        );
    }

    #[test]
    fn refuses_a_stub_unless_allowed() {
        let db = setup();
        assert_eq!(
            err("_stub/Ghost.md", &db, false),
            "\"_stub/Ghost.md\" is an unresolved link target, not a note on disk."
        );
        assert!(err("Ghost", &db, false).contains("unresolved link target"));
        assert_eq!(
            resolve_single_note("Ghost", &db, true).unwrap(),
            "_stub/Ghost.md"
        );
    }

    #[test]
    fn drops_stubs_before_the_ambiguity_check() {
        let db = setup();
        node(&db, "_stub/Delta.md", "Delta", json!({"_stub": true}));
        node(&db, "Notes/Delta.md", "Delta", json!({}));
        assert_eq!(
            resolve_single_note("delta", &db, false).unwrap(),
            "Notes/Delta.md"
        );
        assert!(err("delta", &db, true).contains("Multiple notes match"));
    }
}
