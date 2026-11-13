//! `oracle.json` holds what gray-matter 4.0.3 and js-yaml 3.15.2 return for
//! each case, generated with Node from the TypeScript server's
//! dependencies. The edit tests check that untouched lines stay
//! byte-identical.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;

fn oracle() -> Value {
    serde_json::from_str(include_str!("oracle.json")).expect("valid oracle")
}

/// What the TS saw after spreading gray-matter's data into an object.
fn as_frontmatter(v: &Value) -> Frontmatter {
    to_frontmatter(v.clone())
}

#[test]
fn parse_matches_gray_matter() {
    for case in oracle()["parse"].as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        let got = parse(raw);
        if let Some(err) = case.get("error") {
            assert!(got.is_err(), "{raw:?} should fail like gray-matter: {err}");
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("{raw:?}: {e}"));
        assert_eq!(
            Value::Object(got.data),
            Value::Object(as_frontmatter(&case["data"])),
            "data of {raw:?}"
        );
        assert_eq!(
            got.content,
            case["content"].as_str().unwrap(),
            "content of {raw:?}"
        );
    }
}

#[test]
fn dump_matches_js_yaml() {
    for case in oracle()["dump"].as_array().unwrap() {
        assert_eq!(
            dump_yaml(&case["value"]),
            case["yaml"].as_str().unwrap(),
            "dump of {}",
            case["value"]
        );
    }
}

#[test]
fn stringify_matches_gray_matter() {
    for case in oracle()["stringify"].as_array().unwrap() {
        let data = case["data"].as_object().unwrap();
        assert_eq!(
            stringify(case["content"].as_str().unwrap(), data),
            case["out"].as_str().unwrap()
        );
    }
}

#[test]
fn dump_round_trips_through_load() {
    for case in oracle()["dump"].as_array().unwrap() {
        let v = &case["value"];
        if v.is_object() {
            let back = load::load(&dump_yaml(v)).unwrap().value;
            assert_eq!(back, *v);
        }
    }
}

#[test]
fn split_reports_the_block() {
    let s = split("\u{feff}---yaml\na: 1\n---\nbody");
    assert_eq!(s.bom, 3);
    let b = s.block.unwrap();
    assert_eq!(b.language, "yaml");
    assert_eq!(b.close, Some(15));
    assert_eq!(s.content, "body");
    assert_eq!(split("plain").block, None);
    assert_eq!(split("---\na: 1\n").block.unwrap().close, None);
}

fn set(k: &str, v: Value) -> FrontmatterOp {
    FrontmatterOp::Set(k.into(), v)
}

fn remove(k: &str) -> FrontmatterOp {
    FrontmatterOp::Remove(k.into())
}

fn edit(raw: &str, ops: &[FrontmatterOp]) -> Edited {
    let e = edit_frontmatter(raw, ops).unwrap();
    // Whatever path it took, the result reads back as the expected data.
    assert_eq!(parse(&e.text).unwrap().data, e.data, "{:?}", e.text);
    e
}

fn edit_in_place(raw: &str, ops: &[FrontmatterOp]) -> String {
    let e = edit(raw, ops);
    assert!(
        e.in_place,
        "expected an in-place edit of {raw:?}, got {:?}",
        e.text
    );
    e.text
}

const RICH: &str = "---
# Header comment
title: Old   # trailing
'quoted key': x
\"dq: key\": 1
tags: [a, b]   # flow list
list:
  - one
  - two
nested:
  a: 1
  # inner comment
  b: 2

# about the date
date: 2024-01-01
body: |
  line one
  # not a comment
  line three
last: true
---
Body text
";

#[test]
fn set_changes_only_that_line() {
    let out = edit_in_place(RICH, &[set("title", json!("New"))]);
    assert_eq!(
        out,
        RICH.replace("title: Old   # trailing\n", "title: New\n")
    );
}

#[test]
fn set_keeps_the_key_spelling() {
    let out = edit_in_place(
        RICH,
        &[set("quoted key", json!("why")), set("dq: key", json!(2))],
    );
    assert_eq!(
        out,
        RICH.replace("'quoted key': x\n", "'quoted key': why\n")
            .replace("\"dq: key\": 1\n", "\"dq: key\": 2\n")
    );
}

#[test]
fn remove_drops_a_multi_line_value_and_keeps_comments() {
    let out = edit_in_place(RICH, &[remove("list"), remove("nested"), remove("body")]);
    let want = RICH
        .replace("list:\n  - one\n  - two\n", "")
        .replace("nested:\n  a: 1\n  # inner comment\n  b: 2\n", "")
        .replace("body: |\n  line one\n  # not a comment\n  line three\n", "");
    assert_eq!(out, want);
    assert!(out.contains("\n# about the date\ndate: 2024-01-01\n"));
}

#[test]
fn set_renders_new_values_in_js_yaml_style() {
    let out = edit_in_place(
        RICH,
        &[
            set("nested", json!({"a": 3, "c": ["x", "y", "z"]})),
            set("status", json!("draft")),
            set("when", json!("2024-02-02")),
        ],
    );
    let want = RICH
        .replace(
            "nested:\n  a: 1\n  # inner comment\n  b: 2\n",
            "nested:\n  a: 3\n  c:\n    - x\n    - 'y'\n    - z\n",
        )
        .replace(
            "last: true\n",
            "last: true\nstatus: draft\nwhen: '2024-02-02'\n",
        );
    assert_eq!(out, want);
    // The date nobody touched keeps its YAML form.
    assert!(out.contains("date: 2024-01-01\n"));
}

#[test]
fn setting_the_same_value_changes_nothing() {
    let e = edit(
        RICH,
        &[
            set("title", json!("Old")),
            set("tags", json!(["a", "b"])),
            remove("missing"),
        ],
    );
    assert_eq!(e.text, RICH);
}

#[test]
fn crlf_notes_get_crlf_lines() {
    let raw = "---\r\ntitle: Old\r\ntags:\r\n  - a\r\n---\r\nBody\r\n";
    let out = edit_in_place(raw, &[set("tags", json!(["b", "c"])), set("new", json!(1))]);
    assert_eq!(
        out,
        "---\r\ntitle: Old\r\ntags:\r\n  - b\r\n  - c\r\nnew: 1\r\n---\r\nBody\r\n"
    );
    assert_eq!(
        edit_in_place("Body\r\nmore\r\n", &[set("a", json!("x"))]),
        "---\r\na: x\r\n---\r\nBody\r\nmore\r\n"
    );
}

#[test]
fn creates_a_block_and_keeps_the_body() {
    assert_eq!(
        edit_in_place(
            "Just body, no frontmatter.",
            &[set("status", json!("draft"))]
        ),
        "---\nstatus: draft\n---\nJust body, no frontmatter."
    );
    assert_eq!(
        edit_in_place("", &[set("a", json!([1]))]),
        "---\na:\n  - 1\n---\n"
    );
    assert_eq!(
        edit_in_place("\u{feff}body", &[set("a", json!(1))]),
        "\u{feff}---\na: 1\n---\nbody"
    );
    // Removing from a note without a block changes nothing.
    assert_eq!(edit("body", &[remove("a")]).text, "body");
}

#[test]
fn removing_the_last_key_drops_the_block() {
    assert_eq!(
        edit_in_place("---\na: 1\n---\n\nBody", &[remove("a")]),
        "\nBody"
    );
    assert_eq!(
        edit_in_place("\u{feff}---\r\na: 1\r\n---\r\nBody", &[remove("a")]),
        "\u{feff}Body"
    );
}

#[test]
fn edits_empty_and_comment_only_blocks() {
    assert_eq!(
        edit_in_place("---\n---\nbody", &[set("a", json!(1))]),
        "---\na: 1\n---\nbody"
    );
    assert_eq!(
        edit_in_place("---\n# keep me\n---\nbody", &[set("a", json!(1))]),
        "---\n# keep me\na: 1\n---\nbody"
    );
    // A null document cannot take a key next to it.
    let e = edit("---\n~\n---\nbody", &[set("a", json!(1))]);
    assert!(!e.in_place);
    assert_eq!(e.text, "---\na: 1\n---\nbody\n");
}

#[test]
fn edits_indented_and_compact_mappings() {
    assert_eq!(
        edit_in_place("---\n  a: 1\n  b: 2\n---\n", &[set("b", json!({"x": 1}))]),
        "---\n  a: 1\n  b:\n    x: 1\n---\n"
    );
    let raw = "---\ntags:\n- a\n- b\nnext: 1\n---\n";
    assert_eq!(edit_in_place(raw, &[remove("tags")]), "---\nnext: 1\n---\n");
    assert_eq!(
        edit_in_place(raw, &[set("next", json!(2))]),
        "---\ntags:\n- a\n- b\nnext: 2\n---\n"
    );
}

#[test]
fn keeps_blank_lines_and_comments_between_keys() {
    let raw = "---\na: 1\n\n# about b\nb: 2\n\n---\n";
    assert_eq!(
        edit_in_place(raw, &[remove("a")]),
        "---\n\n# about b\nb: 2\n\n---\n"
    );
    assert_eq!(
        edit_in_place(raw, &[set("c", json!(3))]),
        "---\na: 1\n\n# about b\nb: 2\nc: 3\n\n---\n"
    );
}

#[test]
fn multi_line_plain_and_quoted_values() {
    let raw =
        "---\nmulti: this is\n  a plain\n  multi-line scalar\nq: \"line\n  two\"\nz: 1\n---\n";
    assert_eq!(
        edit_in_place(raw, &[set("multi", json!("short"))]),
        "---\nmulti: short\nq: \"line\n  two\"\nz: 1\n---\n"
    );
    assert_eq!(
        edit_in_place(raw, &[remove("q")]),
        "---\nmulti: this is\n  a plain\n  multi-line scalar\nz: 1\n---\n"
    );
    assert_eq!(
        edit_in_place(raw, &[set("z", json!("two\nlines"))]),
        "---\nmulti: this is\n  a plain\n  multi-line scalar\nq: \"line\n  two\"\nz: |-\n  two\n  lines\n---\n"
    );
}

#[test]
fn index_like_keys_keep_js_order_in_the_data() {
    let e = edit(RICH, &[set("1", json!("one"))]);
    assert!(e.in_place);
    assert_eq!(e.data.keys().next().map(String::as_str), Some("1"));
    assert!(e.text.contains("last: true\n'1': one\n"));
}

#[test]
fn falls_back_to_re_serialising_when_lines_cannot_carry_the_edit() {
    // A flow-style block.
    let e = edit("---\n{a: 1, b: 2}\n---\nbody", &[set("b", json!(3))]);
    assert!(!e.in_place);
    assert_eq!(e.text, "---\na: 1\nb: 3\n---\nbody\n");
    // Removing a key that only a merge provides.
    let e = edit("---\nbase: &b {x: 1}\n<<: *b\ny: 2\n---\n", &[remove("x")]);
    assert!(!e.in_place);
    assert_eq!(
        e.data,
        json!({"base": {"x": 1}, "y": 2})
            .as_object()
            .unwrap()
            .clone()
    );
    // Changing a key whose anchor another key uses.
    let e = edit("---\na: &x 1\nb: *x\n---\n", &[set("a", json!(2))]);
    assert!(!e.in_place);
    // matter.stringify gives an empty body its newline.
    assert_eq!(e.text, "---\na: 2\nb: 1\n---\n\n");
    // Tab indentation, which only js-yaml's reading accepts.
    assert!(
        !edit(
            "---\ntitle: Test\nnested:\n\tvalue: ok\n---\n",
            &[set("title", json!("x"))]
        )
        .in_place
    );
    // A block that never closes.
    assert!(!edit("---\na: 1\n", &[set("a", json!(2))]).in_place);
}

#[test]
fn overriding_a_merged_key_adds_it_explicitly() {
    let raw = "---\nbase: &b {x: 1}\n<<: *b\ny: 2\n---\n";
    assert_eq!(
        edit_in_place(raw, &[set("x", json!(5))]),
        "---\nbase: &b {x: 1}\n<<: *b\ny: 2\nx: 5\n---\n"
    );
}

#[test]
fn explicit_keys_are_not_edited_by_line() {
    let e = edit(
        "---\n? complex\n: value\nb: 1\n---\n",
        &[set("b", json!(2))],
    );
    assert!(!e.in_place);
    assert_eq!(
        e.data,
        json!({"complex": "value", "b": 2})
            .as_object()
            .unwrap()
            .clone()
    );
}

#[test]
fn edit_errors_on_broken_yaml() {
    assert!(edit_frontmatter("---\na: 1\na: 2\n---\n", &[set("b", json!(1))]).is_err());
}

#[test]
fn language_blocks() {
    assert!(edit_in_place("---yaml\na: 1\n---\n", &[set("a", json!(2))]) == "---yaml\na: 2\n---\n");
    // JSON frontmatter is rewritten as YAML, as matter.stringify did.
    let e = edit("---json\n{\"a\": 1}\n---\nbody", &[set("a", json!(2))]);
    assert!(!e.in_place);
    assert_eq!(e.text, "---\na: 2\n---\nbody\n");
}

#[test]
fn dotted_document_end_inside_the_block() {
    let raw = "---\na: 1\n...\n---\nafter";
    assert_eq!(
        edit_in_place(raw, &[set("b", json!(2))]),
        "---\na: 1\nb: 2\n...\n---\nafter"
    );
}

/// The number of separate runs of changed lines between `a` and `b`.
fn changed_hunks(a: &str, b: &str) -> usize {
    let diff = similar::TextDiff::from_lines(a, b);
    let mut hunks = 0;
    let mut in_change = false;
    for op in diff.ops() {
        let changed = !matches!(op, similar::DiffOp::Equal { .. });
        if changed && !in_change {
            hunks += 1;
        }
        in_change = changed;
    }
    hunks
}

#[test]
fn every_single_key_edit_touches_one_run_of_lines() {
    let samples = [
        RICH.to_string(),
        RICH.replace('\n', "\r\n"),
        "---\n  a: 1\n  # note\n  b:\n    - x\n  c: 'q'\n---\nbody".to_string(),
        "---\ntags:\n- a\n- b\n\n# trailing comment\nnext: >\n  folded\n  text\n\nempty:\nk: \"v # not comment\"\n---\n".to_string(),
        "\u{feff}---yaml\nx: 2024-01-01\ny: 0123\nz: [1, {a: b}]\n...\n---\nbody\n".to_string(),
    ];
    for raw in &samples {
        let keys: Vec<String> = parse(raw).unwrap().data.keys().cloned().collect();
        assert!(!keys.is_empty());
        for key in &keys {
            for op in [
                remove(key),
                set(key, json!("zz new")),
                set(key, json!({"deep": [1, "two"]})),
            ] {
                let e = edit(raw, std::slice::from_ref(&op));
                assert!(e.in_place, "{op:?} on {raw:?} fell back: {:?}", e.text);
                assert_eq!(
                    changed_hunks(raw, &e.text),
                    1,
                    "{op:?} on {raw:?} gave {:?}",
                    e.text
                );
            }
        }
        // A new key is one inserted run as well.
        let e = edit(raw, &[set("brand new", json!(["a"]))]);
        assert!(e.in_place);
        assert_eq!(changed_hunks(raw, &e.text), 1, "{:?}", e.text);
    }
}
