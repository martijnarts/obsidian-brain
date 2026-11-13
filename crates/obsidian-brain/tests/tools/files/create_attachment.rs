//! Ported from `test/tools/create-attachment.test.ts`.

use serde_json::json;

use super::harness::{Harness, assert_contains, outside, symlink};

const BYTES: [u8; 7] = [0x89, 0x50, 0x4e, 0x47, 0x00, 0xff, 0x10];
const B64: &str = "iVBORwD/EA==";

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn writes_the_decoded_bytes_and_creates_missing_parent_folders() {
    let h = Harness::new();
    let data = h
        .ok(
            "create_attachment",
            json!({ "path": "assets/img/pic.png", "content": B64 }),
        )
        .await;
    assert_eq!(
        data,
        json!({ "path": "assets/img/pic.png", "bytesWritten": BYTES.len(), "overwritten": false })
    );
    assert_eq!(std::fs::read(h.abs("assets/img/pic.png")).unwrap(), BYTES);
    assert_eq!(names(&h.abs("assets/img")), ["pic.png"]);
}

#[tokio::test]
async fn accepts_base64_with_line_breaks_and_without_padding() {
    let h = Harness::new();
    let wrapped = format!("{}\n{}", &B64[..4], &B64[4..]);
    let wrapped = wrapped.trim_end_matches('=');
    h.ok(
        "create_attachment",
        json!({ "path": "a.bin", "content": wrapped }),
    )
    .await;
    assert_eq!(std::fs::read(h.abs("a.bin")).unwrap(), BYTES);
}

#[tokio::test]
async fn refuses_to_replace_an_existing_file_unless_overwrite_is_true() {
    let h = Harness::new();
    h.write("pic.png", "old");
    assert_eq!(
        h.err(
            "create_attachment",
            json!({ "path": "pic.png", "content": B64 })
        )
        .await,
        "File already exists: pic.png. Pass overwrite: true to replace it."
    );
    assert_contains(
        &h.err(
            "create_attachment",
            json!({ "path": "pic.png", "content": B64, "overwrite": false }),
        )
        .await,
        &["already exists"],
    );
    assert_eq!(h.read("pic.png"), "old");

    let data = h
        .ok(
            "create_attachment",
            json!({ "path": "pic.png", "content": B64, "overwrite": true }),
        )
        .await;
    assert_eq!(data["overwritten"], true);
    assert_eq!(std::fs::read(h.abs("pic.png")).unwrap(), BYTES);
}

#[tokio::test]
async fn refuses_markdown_a_folder_the_root_and_invalid_base64() {
    let h = Harness::new();
    std::fs::create_dir(h.abs("dir")).unwrap();
    let cases = [
        (json!({ "path": "Note.MD", "content": B64 }), "create_note"),
        (
            json!({ "path": "dir", "content": B64, "overwrite": true }),
            "dir is a folder",
        ),
        (json!({ "path": "", "content": B64 }), "vault root"),
        (
            json!({ "path": "x.bin", "content": "not base64!" }),
            "not valid base64",
        ),
        (
            json!({ "path": "x.bin", "content": "abcde" }),
            "not valid base64",
        ),
        (
            json!({ "path": "x.bin", "content": "ab=c" }),
            "not valid base64",
        ),
    ];
    for (args, want) in cases {
        assert_contains(&h.err("create_attachment", args).await, &[want]);
    }
    assert_eq!(names(&h.vault), ["dir"]);
}

#[tokio::test]
async fn caps_the_decoded_size_at_10_mb() {
    let h = Harness::new();
    let limit = 10 * 1024 * 1024;
    // The limit is 3 * 3495253 + 1 bytes: whole groups, then one or two
    // padded bytes.
    let groups = "A".repeat(limit / 3 * 4);
    let too_big = format!("{groups}AAA=");
    let at_limit = format!("{groups}AA==");
    assert_contains(
        &h.err(
            "create_attachment",
            json!({ "path": "big.bin", "content": too_big }),
        )
        .await,
        &["limit is 10485760 bytes"],
    );
    let data = h
        .ok(
            "create_attachment",
            json!({ "path": "max.bin", "content": at_limit }),
        )
        .await;
    assert_eq!(data["bytesWritten"], limit);
}

#[tokio::test]
async fn refuses_paths_outside_the_vault() {
    let h = Harness::new();
    for path in ["../x.png", "/tmp/x.png"] {
        assert_contains(
            &h.err("create_attachment", json!({ "path": path, "content": B64 }))
                .await,
            &["outside the vault"],
        );
    }
    let out = outside();
    symlink(out.path(), &h.abs("out"));
    assert_contains(
        &h.err(
            "create_attachment",
            json!({ "path": "out/x.png", "content": B64 }),
        )
        .await,
        &["outside the vault"],
    );
    assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
}
