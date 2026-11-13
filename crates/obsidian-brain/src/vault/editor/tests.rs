//! Ported from `test/vault/editor/*`, `test/vault/editor-v122.test.ts` and
//! the pure parts of `test/tools/edit-note-expected-content.test.ts` and
//! `test/tools/update-properties.test.ts`.

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::vault::frontmatter;

const REL: &str = "note.md";

struct Vault(TempDir);

impl Vault {
    fn seed(initial: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(REL), initial).unwrap();
        Vault(dir)
    }

    fn write(&self, content: &str) {
        std::fs::write(self.0.path().join(REL), content).unwrap();
    }

    fn edit(&self, mode: EditMode) -> Result<EditResult> {
        edit_note(self.0.path(), REL, &mode, None)
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.0.path().join(REL)).unwrap()
    }
}

fn append(content: &str) -> EditMode {
    EditMode::Append {
        content: content.into(),
    }
}

fn heading(heading: &str, content: &str, op: HeadingOp) -> EditMode {
    EditMode::PatchHeading {
        heading: heading.into(),
        content: content.into(),
        op,
        scope: HeadingScope::Section,
        heading_index: None,
    }
}

fn window(search: &str, content: &str, fuzzy: bool, threshold: Option<f64>) -> EditMode {
    EditMode::ReplaceWindow {
        search: search.into(),
        content: content.into(),
        fuzzy,
        fuzzy_threshold: threshold,
    }
}

fn at_line_mode(line: i64, content: &str, op: LineOp) -> EditMode {
    EditMode::AtLine {
        line,
        content: content.into(),
        op,
    }
}

fn fm(key: &str, value: Option<Value>) -> EditMode {
    EditMode::PatchFrontmatter {
        key: key.into(),
        value,
    }
}

// --- append / prepend -------------------------------------------------------

#[test]
fn append_adds_content_to_end_of_file() {
    let v = Vault::seed("hello\n");
    let r = v.edit(append("world\n")).unwrap();
    assert!(r.bytes_written > 0);
    assert_eq!(v.read(), "hello\nworld\n");
}

#[test]
fn prepend_adds_content_to_start_of_file() {
    let v = Vault::seed("hello\n");
    v.edit(EditMode::Prepend {
        content: "intro\n".into(),
    })
    .unwrap();
    assert_eq!(v.read(), "intro\nhello\n");
}

#[test]
fn prepend_goes_after_the_frontmatter_block() {
    let v = Vault::seed("---\ntitle: foo\n---\n# Heading\nbody\n");
    v.edit(EditMode::Prepend {
        content: "<!-- banner -->\n".into(),
    })
    .unwrap();
    assert_eq!(
        v.read(),
        "---\ntitle: foo\n---\n<!-- banner -->\n# Heading\nbody\n"
    );

    let v = Vault::seed("# Heading\nbody\n");
    v.edit(EditMode::Prepend {
        content: "<!-- banner -->\n".into(),
    })
    .unwrap();
    assert_eq!(v.read(), "<!-- banner -->\n# Heading\nbody\n");

    let v = Vault::seed("---\ntitle: foo\n# Heading\nbody\n");
    v.edit(EditMode::Prepend {
        content: "<!-- banner -->\n".into(),
    })
    .unwrap();
    assert_eq!(
        v.read(),
        "<!-- banner -->\n---\ntitle: foo\n# Heading\nbody\n"
    );
}

#[test]
fn prepend_offset_matches_the_ts_regex() {
    assert_eq!(prepend_offset("---\r\na: 1\r\n---\r\nx"), 16);
    // An empty block has no line between the fences, which the regex needs.
    assert_eq!(prepend_offset("---\n---\nx"), 0);
    assert_eq!(prepend_offset("---\na\n----\nb\n---\nc"), 17);
}

#[test]
fn append_inserts_a_leading_newline_only_when_needed() {
    let v = Vault::seed("trailing text no newline");
    v.edit(append("added\n")).unwrap();
    assert_eq!(v.read(), "trailing text no newline\nadded\n");
    let v = Vault::seed("line1\nline2\n");
    v.edit(append("added\n")).unwrap();
    assert_eq!(v.read(), "line1\nline2\nadded\n");
    let v = Vault::seed("");
    v.edit(append("first\n")).unwrap();
    assert_eq!(v.read(), "first\n");
}

// --- at_line ----------------------------------------------------------------

#[test]
fn at_line_inserts_and_replaces() {
    for (op, want) in [
        (LineOp::Before, vec!["line1", "INSERTED", "line2"]),
        (LineOp::After, vec!["line1", "line2", "INSERTED"]),
        (LineOp::Replace, vec!["line1", "INSERTED", "line3"]),
    ] {
        let v = Vault::seed("line1\nline2\nline3\n");
        v.edit(at_line_mode(2, "INSERTED", op)).unwrap();
        let after = v.read();
        assert_eq!(
            after.split('\n').take(3).collect::<Vec<_>>(),
            want,
            "{op:?}"
        );
    }
}

#[test]
fn at_line_errors_on_out_of_range_line() {
    let v = Vault::seed("line1\nline2\nline3\n");
    let err = v
        .edit(at_line_mode(999, "x", LineOp::After))
        .unwrap_err()
        .to_string();
    assert_eq!(err, "[at_line] Invalid line 999; file has 4 lines");
    assert!(apply_edit("a", &at_line_mode(0, "x", LineOp::After)).is_err());
    // One past the last line appends.
    assert_eq!(
        apply_edit("a", &at_line_mode(2, "x", LineOp::After))
            .unwrap()
            .next,
        "a\nx"
    );
}

// --- patch_heading ----------------------------------------------------------

const HEADINGS: &str =
    "# Root\n\n## Alpha\nalpha body line 1\nalpha body line 2\n\n## Beta\nbeta body\n";

#[test]
fn patch_heading_replace_after_before() {
    let v = Vault::seed(HEADINGS);
    v.edit(heading("Alpha", "new alpha body", HeadingOp::Replace))
        .unwrap();
    let after = v.read();
    assert!(
        after.contains("## Alpha\nnew alpha body\n## Beta"),
        "{after}"
    );

    let v = Vault::seed(HEADINGS);
    v.edit(heading("Alpha", "INSERTED AFTER", HeadingOp::After))
        .unwrap();
    let after = v.read();
    assert!(after.contains("## Alpha\nINSERTED AFTER\n") && after.contains("alpha body line 1"));

    let v = Vault::seed(HEADINGS);
    v.edit(heading("Beta", "INSERTED BEFORE", HeadingOp::Before))
        .unwrap();
    assert!(v.read().contains("INSERTED BEFORE\n## Beta"));
}

#[test]
fn patch_heading_errors_when_heading_is_missing() {
    let v = Vault::seed(HEADINGS);
    assert_eq!(
        v.edit(heading("Missing", "x", HeadingOp::Replace))
            .unwrap_err()
            .to_string(),
        "[patch_heading] Heading not found: Missing"
    );
}

const DUPES: &str =
    "# Root\n\n## Notes\nfirst notes body\n\n## Other\nother body\n\n## Notes\nsecond notes body\n";

#[test]
fn patch_heading_multiple_matches() {
    let v = Vault::seed(DUPES);
    let err = v
        .edit(heading("Notes", "x", HeadingOp::Replace))
        .unwrap_err();
    let mme = err
        .downcast_ref::<MultipleMatchesError>()
        .expect("a MultipleMatchesError");
    assert_eq!(
        mme.matches.iter().map(|m| m.line).collect::<Vec<_>>(),
        [3, 9]
    );
    assert_eq!(
        err.to_string(),
        "[patch_heading] MultipleMatches: 2 headings match — pass headingIndex (0..1) to disambiguate:\n  [3] ## Notes\n  [9] ## Notes"
    );
}

fn indexed(index: i64, content: &str) -> EditMode {
    EditMode::PatchHeading {
        heading: "Notes".into(),
        content: content.into(),
        op: HeadingOp::Replace,
        scope: HeadingScope::Section,
        heading_index: Some(index),
    }
}

#[test]
fn patch_heading_index_picks_an_occurrence() {
    let v = Vault::seed(DUPES);
    v.edit(indexed(0, "PATCHED FIRST")).unwrap();
    let after = v.read();
    assert!(after.contains("## Notes\nPATCHED FIRST\n") && after.contains("second notes body"));

    let v = Vault::seed(DUPES);
    v.edit(indexed(1, "PATCHED SECOND")).unwrap();
    let after = v.read();
    assert!(
        after.contains("first notes body")
            && after.contains("## Notes\nPATCHED SECOND")
            && !after.contains("second notes body")
    );

    let v = Vault::seed(DUPES);
    assert_eq!(
        v.edit(indexed(5, "x")).unwrap_err().to_string(),
        "[patch_heading] headingIndex=5 out of range; 2 matches found (valid: 0..1)"
    );
    // An index is ignored when only one heading matches.
    let v = Vault::seed(DUPES);
    v.edit(EditMode::PatchHeading {
        heading: "Other".into(),
        content: "ONE AND ONLY".into(),
        op: HeadingOp::Replace,
        scope: HeadingScope::Section,
        heading_index: Some(0),
    })
    .unwrap();
    assert!(v.read().contains("## Other\nONE AND ONLY\n"));
}

const TRAILER: &str = "## Section A\nbody A\n\n## Section B\nbody B\n\nAppended trailer line.\n";

fn scoped(h: &str, content: &str, scope: HeadingScope) -> EditMode {
    EditMode::PatchHeading {
        heading: h.into(),
        content: content.into(),
        op: HeadingOp::Replace,
        scope,
        heading_index: None,
    }
}

#[test]
fn section_scope_eats_to_eof_on_the_last_heading() {
    let v = Vault::seed(TRAILER);
    let r = v
        .edit(scoped("Section B", "new-B", HeadingScope::Section))
        .unwrap();
    let after = v.read();
    assert!(after.contains("new-B") && !after.contains("Appended trailer line."));
    assert!(r.removed_len > 0);
}

#[test]
fn body_scope_stops_at_the_first_blank_line() {
    let v = Vault::seed(TRAILER);
    v.edit(scoped("Section B", "new-B", HeadingScope::Body))
        .unwrap();
    let after = v.read();
    assert!(
        after.contains("new-B")
            && after.contains("Appended trailer line.")
            && !after.contains("body B")
    );

    let v = Vault::seed("## A\nline1\nline2\n## B\nbody B\n");
    v.edit(scoped("A", "NEW", HeadingScope::Body)).unwrap();
    let after = v.read();
    assert!(after.contains("## B") && after.contains("NEW") && !after.contains("line1"));
}

#[test]
fn replace_keeps_the_blank_line_after_the_heading() {
    let v = Vault::seed("## H\n\nold body\n\n## Next\n");
    v.edit(scoped("H", "new body", HeadingScope::Section))
        .unwrap();
    assert_eq!(v.read(), "## H\n\nnew body\n## Next\n");
}

#[test]
fn body_scope_does_not_double_blank_lines_and_reports_removed_len() {
    let v = Vault::seed("## A\n\nold body\n\n## Next\n");
    v.edit(scoped("A", "new body\n", HeadingScope::Body))
        .unwrap();
    let after = v.read();
    assert!(!after.contains("\n\n\n") && after.contains("new body") && after.contains("## Next"));

    let v = Vault::seed("## A\n\nbody to remove\n\n## Next\n");
    assert!(
        v.edit(scoped("A", "replacement", HeadingScope::Body))
            .unwrap()
            .removed_len
            > 0
    );
}

#[test]
fn insert_only_heading_ops_report_zero_removed_len() {
    for op in [HeadingOp::Before, HeadingOp::After] {
        let v = Vault::seed("## A\nbody\n");
        assert_eq!(v.edit(heading("A", "INS", op)).unwrap().removed_len, 0);
    }
}

#[test]
fn patch_heading_matches_crlf_lines() {
    let r = apply_edit(
        "## A\r\nold\r\n## B\r\n",
        &heading("A", "new\r", HeadingOp::Replace),
    )
    .unwrap();
    assert_eq!(r.next, "## A\r\nnew\r\n## B\r\n");
}

// --- replace_window ---------------------------------------------------------

#[test]
fn replace_window_exact() {
    let v = Vault::seed("foo bar baz\nmore text here\n");
    v.edit(window("bar", "QUX", false, None)).unwrap();
    assert_eq!(v.read(), "foo QUX baz\nmore text here\n");
    assert!(
        v.edit(window("nonexistent", "X", false, None))
            .unwrap_err()
            .to_string()
            .contains("NoMatch")
    );
    v.write("dup dup dup\n");
    assert!(
        v.edit(window("dup", "X", false, None))
            .unwrap_err()
            .to_string()
            .contains("MultipleMatches")
    );
}

#[test]
fn replace_window_fuzzy() {
    let v = Vault::seed("The quick brown fox jumps.\n");
    v.edit(window("quikc brown", "nimble red", true, None))
        .unwrap();
    let after = v.read();
    assert!(after.contains("nimble red") && !after.contains("quick brown"));

    // Trailing sentence punctuation is swallowed so it is not doubled.
    v.write("Original content under section B.\n");
    v.edit(window(
        "original content under SECTION b",
        "Section B content was replaced via FUZZY replace_window.",
        true,
        None,
    ))
    .unwrap();
    assert_eq!(
        v.read(),
        "Section B content was replaced via FUZZY replace_window.\n"
    );
}

#[test]
fn replace_window_fuzzy_threshold() {
    let v = Vault::seed("The quick brown fox jumped.\n");
    let err = v
        .edit(window("quikk bworn", "REPLACED", true, Some(0.95)))
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "[replace_window] NoMatch: needle not found (fuzzy, threshold=0.95)"
    );

    v.write("Few small steps taken now.\n");
    v.edit(window("fw sml stps tkn", "REPLACED", true, Some(0.3)))
        .unwrap();
    assert!(v.read().contains("REPLACED"));

    v.write("The quick brown fox jumped.\n");
    v.edit(window("quikk bworn", "REPLACED", true, None))
        .unwrap();
    assert!(v.read().contains("REPLACED"));
}

#[test]
fn replace_window_fuzzy_reports_ambiguity_with_utf16_offsets() {
    let err = apply_edit(
        "é alpha beta\né alpha beta\n",
        &window("alpha betx", "Z", true, None),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        "[replace_window] MultipleMatches: 2 candidates near \"alpha beta\" @2, \"alpha beta\" @15"
    );
}

#[test]
fn replace_window_counts_removed_bytes() {
    let v = Vault::seed("Hello 🎉 world\n");
    let r = v
        .edit(window("Hello 🎉 world", "Hi there", false, None))
        .unwrap();
    assert_eq!(v.read(), "Hi there\n");
    assert_eq!(r.removed_len, 16);

    v.write("café con leche\n");
    let r = v.edit(window("café", "tea", false, None)).unwrap();
    assert_eq!(v.read(), "tea con leche\n");
    assert_eq!(r.removed_len, 5);

    v.write("before (.*+?) after\n");
    v.edit(window("(.*+?)", "LITERAL", false, None)).unwrap();
    assert_eq!(v.read(), "before LITERAL after\n");

    v.write("START then middle then end\n");
    v.edit(window("START", "BEGIN", false, None)).unwrap();
    assert_eq!(v.read(), "BEGIN then middle then end\n");
}

#[test]
fn diff_context_is_cut_in_utf16_units() {
    let v = Vault::seed(&format!("{}X{}", "é".repeat(300), "b".repeat(300)));
    let r = v.edit(window("X", "Y", false, None)).unwrap();
    assert_eq!(
        r.diff.before,
        format!("{}X{}", "é".repeat(250), "b".repeat(250))
    );
    assert_eq!(
        r.diff.after,
        format!("{}Y{}", "é".repeat(250), "b".repeat(250))
    );
}

// --- patch_frontmatter -------------------------------------------------------

fn data(text: &str) -> Frontmatter {
    frontmatter::parse(text).unwrap().data
}

fn body(text: &str) -> String {
    frontmatter::parse(text).unwrap().content.to_string()
}

#[test]
fn patch_frontmatter_set_and_clear() {
    let v = Vault::seed("---\ntitle: Test\ntags: [a]\n---\n\nBody content.\n");
    v.edit(fm("status", Some(json!("active")))).unwrap();
    assert_eq!(
        v.read(),
        "---\ntitle: Test\ntags: [a]\nstatus: active\n---\n\nBody content.\n"
    );

    let v = Vault::seed("---\ntitle: Test\ntags: [a]\n---\n\nBody content.\n");
    v.edit(fm("tags", Some(Value::Null))).unwrap();
    assert_eq!(v.read(), "---\ntitle: Test\n---\n\nBody content.\n");

    let v = Vault::seed("---\ntitle: Test\ntags: [a]\n---\n\nBody.\n");
    v.edit(fm("tags", None)).unwrap();
    assert_eq!(
        data(&v.read()),
        json!({"title": "Test"}).as_object().unwrap().clone()
    );
}

#[test]
fn patch_frontmatter_round_trips_arrays_and_nested_objects() {
    let v = Vault::seed("---\ntitle: Test\n---\n\nBody.\n");
    v.edit(fm("tags", Some(json!(["alpha", "beta", "gamma"]))))
        .unwrap();
    assert_eq!(data(&v.read())["tags"], json!(["alpha", "beta", "gamma"]));
    assert_eq!(js::trim(&body(&v.read())), "Body.");

    let v = Vault::seed("---\ntitle: Test\n---\n\nBody.\n");
    v.edit(fm(
        "meta",
        Some(json!({"source": "imported", "stats": {"count": 3, "depth": 2}})),
    ))
    .unwrap();
    assert_eq!(
        data(&v.read())["meta"],
        json!({"source": "imported", "stats": {"count": 3, "depth": 2}})
    );
}

#[test]
fn patch_frontmatter_overwrites_inserts_and_ignores_missing_keys() {
    let v = Vault::seed("---\ntitle: Old\ntags: [keep]\n---\n\nBody.\n");
    v.edit(fm("title", Some(json!("New")))).unwrap();
    assert_eq!(v.read(), "---\ntitle: New\ntags: [keep]\n---\n\nBody.\n");

    let v = Vault::seed("Just body, no frontmatter.\n");
    v.edit(fm("status", Some(json!("draft")))).unwrap();
    assert_eq!(
        v.read(),
        "---\nstatus: draft\n---\nJust body, no frontmatter.\n"
    );

    let v = Vault::seed("---\ntitle: Test\n---\n\nBody.\n");
    v.edit(fm("never_existed", None)).unwrap();
    assert_eq!(v.read(), "---\ntitle: Test\n---\n\nBody.\n");
}

#[test]
fn patch_frontmatter_leaves_the_body_alone() {
    let body_text = "\nParagraph 1 with **bold** and [link](x).\n\n- list item\n- another\n";
    let v = Vault::seed(&format!("---\ntitle: Test\n---{body_text}"));
    v.edit(fm("status", Some(json!("active")))).unwrap();
    assert_eq!(
        v.read(),
        format!("---\ntitle: Test\nstatus: active\n---{body_text}")
    );
}

#[test]
fn patch_frontmatter_reports_where_the_key_landed() {
    let r = apply_edit("---\na: 1\n---\n", &fm("b", Some(json!([1, 2])))).unwrap();
    assert_eq!((r.at, r.len, r.removed_len), (9, 3, 0));
    assert!(apply_edit("---\na: 1\na: 2\n---\n", &fm("b", Some(json!(1)))).is_err());
}

// --- update_frontmatter ------------------------------------------------------

const UP_BODY: &str = "# Heading\n\nBody text with [[link]].\n";

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn update_sets_and_removes_several_keys() {
    let raw = format!("---\ntitle: Note\nstatus: draft\nold: 1\ntags:\n  - a\n---\n{UP_BODY}");
    let out = update_frontmatter(
        &raw,
        &obj(json!({"status": "done", "rating": 4, "tags": ["a", "b"]})),
        &["old".into()],
    )
    .unwrap();
    assert_eq!(out.set, ["status", "rating", "tags"]);
    assert_eq!(out.removed, ["old"]);
    assert_eq!(
        serde_json::to_string(&out.frontmatter).unwrap(),
        r#"{"title":"Note","status":"done","tags":["a","b"],"rating":4}"#
    );
    assert_eq!(
        out.next,
        format!("---\ntitle: Note\nstatus: done\ntags:\n  - a\n  - b\nrating: 4\n---\n{UP_BODY}")
    );
}

#[test]
fn update_creates_and_drops_the_block() {
    assert_eq!(
        update_frontmatter(UP_BODY, &obj(json!({"status": "new"})), &[])
            .unwrap()
            .next,
        format!("---\nstatus: new\n---\n{UP_BODY}")
    );
    assert_eq!(
        update_frontmatter(
            &format!("---\nk: v\n---\n{UP_BODY}"),
            &Map::new(),
            &["k".into()]
        )
        .unwrap()
        .next,
        UP_BODY
    );
}

#[test]
fn update_null_removes_and_set_wins_over_remove() {
    let out = update_frontmatter(
        &format!("---\na: 1\nb: 2\n---\n{UP_BODY}"),
        &obj(json!({"a": null, "b": 3})),
        &["b".into()],
    )
    .unwrap();
    assert_eq!(out.removed, ["a"]);
    assert_eq!(out.set, ["b"]);
    assert_eq!(out.frontmatter, obj(json!({"b": 3})));
}

#[test]
fn update_reports_absent_keys_and_keeps_the_file() {
    let original = format!("---\nz:   'spaced'\n---\n{UP_BODY}");
    let out = update_frontmatter(&original, &Map::new(), &["missing".into()]).unwrap();
    assert_eq!(out.not_present, ["missing"]);
    assert!(out.removed.is_empty());
    assert_eq!(out.next, original);
    // Other keys keep their spelling when one changes.
    let out = update_frontmatter(&original, &obj(json!({"w": 1})), &[]).unwrap();
    assert_eq!(
        out.next,
        format!("---\nz:   'spaced'\nw: 1\n---\n{UP_BODY}")
    );
}

// --- expectedContent -----------------------------------------------------------

#[test]
fn check_expected_content_compares_the_replaced_range() {
    let original = "aaa\nbbb\nccc\n";
    let res = apply_edit(original, &window("bbb", "BBB", false, None)).unwrap();
    assert!(check_expected_content(original, &res, "bbb").is_ok());
    assert!(
        check_expected_content(original, &res, "aaa")
            .unwrap_err()
            .to_string()
            .contains("no longer matches")
    );
    assert!(check_expected_content(original, &res, "bbb  \r\n").is_ok());
}

#[test]
fn check_expected_content_refuses_insert_only_edits() {
    let res = apply_edit("a\n", &append("b")).unwrap();
    assert!(
        check_expected_content("a\n", &res, "x")
            .unwrap_err()
            .to_string()
            .starts_with("expectedContent applies only")
    );
}

#[test]
fn check_expected_content_uses_bytes_after_multibyte_text() {
    let original = "# Plan\n\nÉtape un.\nÉtape deux.\n\n# Notes\n";
    let res = apply_edit(original, &heading("Plan", "new\n", HeadingOp::Replace)).unwrap();
    assert!(check_expected_content(original, &res, "Étape un.\nÉtape deux.\n").is_ok());
}

#[test]
fn edit_note_with_expected_content_writes_nothing_on_mismatch() {
    let v = Vault::seed("# Plan\n\nStep one.\nStep two.\n\n# Notes\n\nKeep this.\n");
    let mode = heading("Plan", "Step 1.\n", HeadingOp::Replace);
    let err = edit_note(v.0.path(), REL, &mode, Some("Step one.\nStep three."))
        .unwrap_err()
        .to_string();
    assert!(err.contains("no longer matches"));
    assert_eq!(
        v.read(),
        "# Plan\n\nStep one.\nStep two.\n\n# Notes\n\nKeep this.\n"
    );
    edit_note(v.0.path(), REL, &mode, Some("Step one.\nStep two.\n")).unwrap();
    assert_eq!(v.read(), "# Plan\n\nStep 1.\n\n# Notes\n\nKeep this.\n");
}

// --- bulk ----------------------------------------------------------------------

#[test]
fn bulk_applies_edits_in_sequence() {
    let v = Vault::seed("# Title\n\nFirst paragraph.\n");
    let r = bulk_edit_note(
        v.0.path(),
        REL,
        &[
            append("\nSecond paragraph.\n"),
            window("First paragraph.", "Updated first paragraph.", false, None),
        ],
    )
    .unwrap();
    assert_eq!(r.edits_applied, 2);
    assert!(r.bytes_written > 0);
    let disk = v.read();
    assert!(disk.contains("Updated first paragraph.") && disk.contains("Second paragraph."));
}

#[test]
fn bulk_rolls_back_when_an_edit_fails() {
    let initial = "# Title\n\nSome content here.\n";
    let v = Vault::seed(initial);
    let err = bulk_edit_note(
        v.0.path(),
        REL,
        &[
            append("\nAppended.\n"),
            window("DOES_NOT_EXIST", "replacement", false, None),
        ],
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        "[bulk edit] edits[1] (replace_window) failed: [replace_window] NoMatch: search text not found. No edits were applied."
    );
    assert_eq!(v.read(), initial);
}

#[test]
fn bulk_with_no_edits_writes_nothing() {
    let v = Vault::seed("unchanged content\n");
    let r = bulk_edit_note(v.0.path(), REL, &[]).unwrap();
    assert_eq!((r.edits_applied, r.bytes_written), (0, 0));
    assert_eq!(r.before, r.after);
    assert_eq!(v.read(), "unchanged content\n");
}

#[test]
fn edit_note_errors_like_node_on_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = edit_note(dir.path(), "gone.md", &append("x"), None)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("ENOENT: no such file or directory, open '"),
        "{err}"
    );
}

// --- EditMode as JSON ---------------------------------------------------------

#[test]
fn edit_modes_deserialize_from_the_ts_shape() {
    let m: EditMode = serde_json::from_value(json!({"kind": "patch_heading", "heading": "H", "content": "x", "headingIndex": 1, "scope": "body"})).unwrap();
    assert_eq!(
        m,
        EditMode::PatchHeading {
            heading: "H".into(),
            content: "x".into(),
            op: HeadingOp::Replace,
            scope: HeadingScope::Body,
            heading_index: Some(1)
        }
    );
    let m: EditMode = serde_json::from_value(
        json!({"kind": "replace_window", "search": "a", "content": "b", "fuzzyThreshold": 0.5}),
    )
    .unwrap();
    assert_eq!(m, window("a", "b", false, Some(0.5)));
    let m: EditMode =
        serde_json::from_value(json!({"kind": "at_line", "line": 3, "content": "c"})).unwrap();
    assert_eq!(m, at_line_mode(3, "c", LineOp::After));
    let m: EditMode =
        serde_json::from_value(json!({"kind": "patch_frontmatter", "key": "k", "value": null}))
            .unwrap();
    assert_eq!(m.kind(), "patch_frontmatter");
}
