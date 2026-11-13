//! Tests of the `create_note_from_template` tool. The date format, template
//! rendering and templates config cases are tested with the vault layer.

use obsidian_brain::store::edges::get_edges_by_target;
use obsidian_brain::store::nodes::get_node;
use serde_json::json;

use crate::support::Vault;

fn vault() -> Vault {
    let v = Vault::new();
    v.write(
        "Templates/Meeting.md",
        "---\ntype: meeting\ncreated: \"{{date}} {{time}}\"\n---\n# {{title}}\n\nOn {{date:dddd D MMMM}} with {{attendees}}.\n",
    );
    v.write("Templates/Plain.md", "Plain {{title}}\n");
    v
}

#[tokio::test]
async fn renders_keeps_the_frontmatter_injects_the_title_and_indexes() {
    let v = vault();
    let out = v
        .ok(
            "create_note_from_template",
            json!({
                "template": "Meeting",
                "title": "Kickoff",
                "directory": "Meetings",
                "variables": { "attendees": "Ann" },
                "date": "2026-03-05T09:07:03",
            }),
        )
        .await;
    assert_eq!(
        out,
        json!({ "path": "Meetings/Kickoff.md", "title": "Kickoff", "template": "Templates/Meeting.md" })
    );
    // The template's own quoting stays: only the `title` line is new.
    assert_eq!(
        v.read("Meetings/Kickoff.md"),
        "---\ntype: meeting\ncreated: \"2026-03-05 09:07\"\ntitle: Kickoff\n---\n# Kickoff\n\nOn Thursday 5 March with Ann.\n"
    );
    let node = v
        .db()
        .with(|c| get_node(c, "Meetings/Kickoff.md"))
        .unwrap()
        .unwrap();
    assert_eq!(node.title, "Kickoff");
    assert_eq!(node.frontmatter["type"], "meeting");
}

#[tokio::test]
async fn accepts_a_path_with_extension_and_a_date_only_date() {
    let v = vault();
    let out = v
        .ok(
            "create_note_from_template",
            json!({ "template": "Templates/Plain.md", "title": "P", "date": "2026-01-02" }),
        )
        .await;
    assert_eq!(out["path"], "P.md");
    assert_eq!(v.read("P.md"), "---\ntitle: P\n---\nPlain P\n");
}

#[tokio::test]
async fn uses_the_folder_and_formats_of_the_templates_settings() {
    let v = vault();
    v.write(
        ".obsidian/templates.json",
        r#"{"folder":"Meta","dateFormat":"DD.MM.YYYY","timeFormat":"h:mm A"}"#,
    );
    v.write("Meta/Daily.md", "{{date}} {{time}}");
    v.ok(
        "create_note_from_template",
        json!({ "template": "Daily", "title": "D", "date": "2026-03-05T15:04:00" }),
    )
    .await;
    let text = v.read("D.md");
    assert!(text.contains("05.03.2026 3:04 PM"), "{text}");
    // `matter.stringify` ended the note with a newline.
    assert!(text.ends_with('\n'));
}

#[tokio::test]
async fn uses_the_current_time_without_a_date() {
    let v = vault();
    v.write("Templates/Year.md", "{{date:YYYY}}");
    v.ok(
        "create_note_from_template",
        json!({ "template": "Year", "title": "Y" }),
    )
    .await;
    let year = chrono::Local::now().format("%Y").to_string();
    assert!(v.read("Y.md").contains(&year));
}

#[tokio::test]
async fn leaves_templater_syntax_and_says_so() {
    let v = vault();
    v.write(
        "Templates/Tp.md",
        "Made <% tp.date.now() %> for {{title}}\n",
    );
    let out = v
        .ok(
            "create_note_from_template",
            json!({ "template": "Tp", "title": "T" }),
        )
        .await;
    assert!(
        out["templater"]
            .as_str()
            .unwrap()
            .contains("does not run Templater")
    );
    assert!(v.read("T.md").contains("Made <% tp.date.now() %> for T"));
}

#[tokio::test]
async fn reports_placeholders_it_could_not_fill() {
    let v = vault();
    let out = v
        .ok(
            "create_note_from_template",
            json!({ "template": "Meeting", "title": "M" }),
        )
        .await;
    assert_eq!(out["unresolved"], json!(["{{attendees}}"]));
}

#[tokio::test]
async fn migrates_a_forward_reference_stub() {
    let v = vault();
    v.row("Other.md", "Other");
    v.row("_stub/Linked.md", "Linked");
    v.edge("Other.md", "_stub/Linked.md", "[[Linked]]");
    v.ok(
        "create_note_from_template",
        json!({ "template": "Plain", "title": "Linked" }),
    )
    .await;
    assert!(
        v.db()
            .with(|c| get_node(c, "_stub/Linked.md"))
            .unwrap()
            .is_none()
    );
    let sources: Vec<String> = v
        .db()
        .with(|c| get_edges_by_target(c, "Linked.md"))
        .unwrap()
        .into_iter()
        .map(|e| e.source_id)
        .collect();
    assert_eq!(sources, ["Other.md"]);
}

#[tokio::test]
async fn refuses_to_overwrite_a_note() {
    let v = vault();
    v.write("Taken.md", "keep");
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "Taken" }),
        )
        .await;
    assert!(err.contains("File already exists: Taken.md"), "{err}");
    assert_eq!(v.read("Taken.md"), "keep");
}

#[tokio::test]
async fn errors_on_a_missing_template() {
    let v = vault();
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Nope", "title": "X" }),
        )
        .await;
    assert!(err.contains("Template not found: \"Nope\""), "{err}");
    assert!(!v.exists("X.md"));
}

#[tokio::test]
async fn refuses_paths_that_leave_the_vault() {
    let v = vault();
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "../../etc/passwd", "title": "X" }),
        )
        .await;
    assert!(err.contains("outside the vault"), "{err}");
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "X", "directory": "../out" }),
        )
        .await;
    assert!(err.contains("outside the vault"), "{err}");
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "a/b" }),
        )
        .await;
    assert!(err.contains("cannot contain"), "{err}");
}

#[tokio::test]
async fn refuses_a_symlinked_directory() {
    let v = vault();
    let outside = v.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, v.path().join("linked")).unwrap();
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "X", "directory": "linked" }),
        )
        .await;
    assert!(err.contains("outside the vault"), "{err}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[tokio::test]
async fn refuses_variables_that_shadow_built_ins() {
    let v = vault();
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "X", "variables": { "Title": "y" } }),
        )
        .await;
    assert!(err.contains("cannot set Title"), "{err}");
}

#[tokio::test]
async fn rejects_an_invalid_date() {
    let v = vault();
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "Plain", "title": "X", "date": "soon" }),
        )
        .await;
    assert!(err.contains("Invalid date: soon"), "{err}");
}

#[tokio::test]
async fn errors_clearly_when_substitution_leaves_invalid_yaml() {
    let v = vault();
    v.write("Templates/BadFm.md", "---\nkey: [unclosed\n---\nbody");
    let err = v
        .err(
            "create_note_from_template",
            json!({ "template": "BadFm", "title": "X" }),
        )
        .await;
    assert!(err.contains("invalid YAML frontmatter"), "{err}");
    assert!(!v.exists("X.md"));
}

#[tokio::test]
async fn a_template_title_wins_and_a_null_title_is_dropped() {
    let v = vault();
    v.write(
        "Templates/Own.md",
        "---\n# kept comment\ntitle: 'Custom'\n---\nbody\n",
    );
    v.write(
        "Templates/None.md",
        "---\ntitle: null\nkind: x\n---\nbody\n",
    );
    v.ok(
        "create_note_from_template",
        json!({ "template": "Own", "title": "O" }),
    )
    .await;
    assert_eq!(
        v.read("O.md"),
        "---\n# kept comment\ntitle: 'Custom'\n---\nbody\n"
    );
    v.ok(
        "create_note_from_template",
        json!({ "template": "None", "title": "N" }),
    )
    .await;
    assert_eq!(v.read("N.md"), "---\nkind: x\n---\nbody\n");
}
