//! The watcher against a real file system, with short timings.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use obsidian_brain::context::VaultContext;
use obsidian_brain::pipeline::{WatcherOptions, start_watcher};
use obsidian_brain::store::nodes::get_node;

use crate::support::{FakeEmbedder, context, write};

async fn wait_until(ctx: &Arc<VaultContext>, id: &str, present: bool) -> bool {
    for _ in 0..200 {
        if ctx.db.with(|c| get_node(c, id)).unwrap().is_some() == present {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

fn has(ctx: &Arc<VaultContext>, id: &str) -> bool {
    ctx.db.with(|c| get_node(c, id)).unwrap().is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn indexes_added_changed_and_removed_notes() {
    let dir = tempfile::tempdir().unwrap();
    let vault: &Path = dir.path();
    std::fs::create_dir_all(vault.join("Sub")).unwrap();
    let ctx = context(vault, Arc::new(FakeEmbedder::instant()));
    let watcher = start_watcher(
        ctx.clone(),
        WatcherOptions {
            debounce_ms: Some(50),
            stability_ms: Some(50),
        },
    )
    .unwrap();
    // Let the watches settle before writing.
    tokio::time::sleep(Duration::from_millis(100)).await;

    write(vault, "Sub/note.md", "# Note\n\nSee [[Other]].\n");
    write(vault, ".obsidian/skip.md", "# Skip\n");
    write(vault, "image.txt", "not markdown");
    assert!(wait_until(&ctx, "Sub/note.md", true).await, "add");
    // The stub lands just after the note, as in the TS indexer.
    assert!(wait_until(&ctx, "_stub/Other.md", true).await, "stub");

    write(vault, "Fresh/Deep/new.md", "# New\n");
    assert!(
        wait_until(&ctx, "Fresh/Deep/new.md", true).await,
        "a note in a new directory"
    );

    std::fs::remove_file(vault.join("Sub/note.md")).unwrap();
    assert!(wait_until(&ctx, "Sub/note.md", false).await, "unlink");

    assert!(!has(&ctx, ".obsidian/skip.md"));
    assert!(!has(&ctx, "image.txt"));
    watcher.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_watcher_indexes_nothing_more() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path(), Arc::new(FakeEmbedder::instant()));
    let watcher = start_watcher(
        ctx.clone(),
        WatcherOptions {
            debounce_ms: Some(50),
            stability_ms: Some(50),
        },
    )
    .unwrap();
    watcher.close().await;
    write(dir.path(), "late.md", "# Late\n");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!has(&ctx, "late.md"));
}
