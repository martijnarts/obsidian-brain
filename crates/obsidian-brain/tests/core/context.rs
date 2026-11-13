//! Tests of the vault context, the shared embedder, closing a context and
//! the startup index. The `OBSIDIAN_BRAIN_NO_CATCHUP` case runs in the
//! end-to-end test, which can set the environment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::{Embedder, share_embedder};
use obsidian_brain::store::nodes::{get_node, upsert_node};
use obsidian_brain::types::ParsedNode;
use obsidian_brain::vaults::run_startup_index;

use crate::support::{FakeEmbedder, context, memory_db, multi_section_note, write};

fn instant_ctx() -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path(), Arc::new(FakeEmbedder::instant()));
    (dir, ctx)
}

#[tokio::test]
async fn reindex_in_progress_is_true_only_while_work_runs() {
    let (_dir, ctx) = instant_ctx();
    assert!(!ctx.reindex_in_progress());
    let seen = Arc::new(AtomicBool::new(false));
    let (c, s) = (ctx.clone(), seen.clone());
    ctx.enqueue_background_reindex(async move {
        s.store(c.reindex_in_progress(), Ordering::SeqCst);
        Ok(())
    });
    ctx.wait_for_background_reindex().await;
    assert!(seen.load(Ordering::SeqCst));
    assert!(!ctx.reindex_in_progress());
    assert_eq!(ctx.background_reindex_count(), 1);
}

#[tokio::test]
async fn a_failed_job_resets_the_flag_and_does_not_block_the_next() {
    let (_dir, ctx) = instant_ctx();
    ctx.enqueue_background_reindex(async { anyhow::bail!("simulated reindex failure") });
    let ran = Arc::new(AtomicBool::new(false));
    let r = ran.clone();
    ctx.enqueue_background_reindex(async move {
        r.store(true, Ordering::SeqCst);
        Ok(())
    });
    ctx.wait_for_background_reindex().await;
    assert!(!ctx.reindex_in_progress());
    assert!(ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn jobs_run_in_fifo_order() {
    let (_dir, ctx) = instant_ctx();
    let order = Arc::new(Mutex::new(Vec::new()));
    for i in 1..=3 {
        let o = order.clone();
        ctx.enqueue_background_reindex(async move {
            // The first job is the slowest: order must still hold.
            tokio::time::sleep(Duration::from_millis(30 / i)).await;
            o.lock().unwrap().push(i);
            Ok(())
        });
    }
    ctx.wait_for_background_reindex().await;
    assert_eq!(*order.lock().unwrap(), [1, 2, 3]);
}

#[tokio::test]
async fn close_drains_in_flight_work_first() {
    let (_dir, ctx) = instant_ctx();
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    ctx.enqueue_background_reindex(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        d.store(true, Ordering::SeqCst);
        Ok(())
    });
    ctx.close().await;
    assert!(done.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn close_stops_waiting_for_a_stuck_job_after_3_seconds() {
    let (_dir, ctx) = instant_ctx();
    ctx.enqueue_background_reindex(std::future::pending::<anyhow::Result<()>>());
    let start = tokio::time::Instant::now();
    ctx.close().await;
    let waited = start.elapsed();
    assert!(
        waited >= Duration::from_secs(3) && waited < Duration::from_secs(4),
        "{waited:?}"
    );
}

#[tokio::test]
async fn an_init_failure_is_recorded_without_any_awaiter() {
    let dir = tempfile::tempdir().unwrap();
    let embedder = Arc::new(FakeEmbedder::instant().gated());
    let ctx = context(dir.path(), embedder.clone());
    // Start the load, then give up on it at once.
    tokio::select! {
        _ = ctx.ensure_embedder_ready() => panic!("the load is gated"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    assert!(ctx.init_error().is_none());
    embedder.release_init(Err("embedder init failed (simulated)"));
    for _ in 0..100 {
        if ctx.init_error().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        ctx.init_error().as_deref(),
        Some("Error: embedder init failed (simulated)")
    );
    // Every later caller gets the same failure; the first error stays.
    let err = ctx.ensure_embedder_ready().await.unwrap_err();
    assert!(err.to_string().contains("embedder init failed"));
    assert!(!ctx.embedder_ready());
}

#[tokio::test]
async fn concurrent_callers_share_one_load() {
    let dir = tempfile::tempdir().unwrap();
    let embedder = Arc::new(FakeEmbedder::instant());
    let ctx = context(dir.path(), embedder.clone());
    let (a, b, c) = tokio::join!(
        ctx.ensure_embedder_ready(),
        ctx.ensure_embedder_ready(),
        ctx.ensure_embedder_ready()
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    ctx.ensure_embedder_ready().await.unwrap();
    assert_eq!(embedder.init_calls.load(Ordering::SeqCst), 1);
    assert!(ctx.embedder_ready());
    assert!(ctx.bootstrap_result().is_some());
}

#[tokio::test]
async fn a_shared_embedder_loads_once_for_every_vault_and_fails_for_all() {
    let embedder = Arc::new(FakeEmbedder::instant());
    let shared = share_embedder(embedder.clone());
    let dir = tempfile::tempdir().unwrap();
    let ctxs: Vec<_> = ["one", "two"]
        .iter()
        .map(|name| {
            let config = VaultConfig::new(name, dir.path(), &dir.path().join(".data"));
            VaultContext::from_parts(config, memory_db(), shared.clone(), false)
        })
        .collect();
    let (a, b) = tokio::join!(
        ctxs[0].ensure_embedder_ready(),
        ctxs[1].ensure_embedder_ready()
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(embedder.init_calls.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(&ctxs[0].embedder, &ctxs[1].embedder));

    let failing = Arc::new(FakeEmbedder::instant().gated());
    failing.release_init(Err("model unavailable"));
    let shared = share_embedder(failing.clone());
    for name in ["one", "two"] {
        let config = VaultConfig::new(name, dir.path(), &dir.path().join(".data"));
        let ctx = VaultContext::from_parts(config, memory_db(), shared.clone(), false);
        let err = ctx.ensure_embedder_ready().await.unwrap_err();
        assert!(err.to_string().contains("model unavailable"));
    }
    assert_eq!(failing.init_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn close_disposes_only_an_owned_embedder_that_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let owned = Arc::new(FakeEmbedder::instant());
    let ctx = context(dir.path(), owned.clone());
    ctx.close().await;
    assert_eq!(
        owned.dispose_calls.load(Ordering::SeqCst),
        0,
        "never loaded"
    );

    let ctx = context(dir.path(), owned.clone());
    ctx.ensure_embedder_ready().await.unwrap();
    ctx.close().await;
    assert_eq!(owned.dispose_calls.load(Ordering::SeqCst), 1);

    let shared = Arc::new(FakeEmbedder::instant());
    let config = VaultConfig::new("s", dir.path(), &dir.path().join(".data"));
    let ctx = VaultContext::from_parts(config, memory_db(), share_embedder(shared.clone()), false);
    ctx.ensure_embedder_ready().await.unwrap();
    ctx.close().await;
    assert_eq!(shared.dispose_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_startup_index_builds_the_first_index_of_an_empty_vault() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", &multi_section_note(1));
    let ctx = context(dir.path(), Arc::new(FakeEmbedder::instant()));
    run_startup_index(ctx.clone(), true).await;
    ctx.wait_for_background_reindex().await;
    assert!(ctx.db.with(|c| get_node(c, "a.md")).unwrap().is_some());
    assert!(ctx.init_error().is_none());
}

#[tokio::test]
async fn the_startup_index_catches_up_and_rebuilds_when_bootstrap_asks() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", &multi_section_note(1));
    let ctx = context(dir.path(), Arc::new(FakeEmbedder::instant()));
    // A node and a sync row but no chunks: bootstrap asks for a rebuild,
    // so the sync row must go or the mtime guard would skip the note.
    ctx.db
        .with(|c| -> anyhow::Result<()> {
            upsert_node(
                c,
                &ParsedNode {
                    id: "a.md".into(),
                    title: "a".into(),
                    content: "old".into(),
                    frontmatter: Default::default(),
                },
            )?;
            c.execute(
                "INSERT INTO sync (path, mtime, indexed_at) VALUES ('a.md', 1e15, 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    run_startup_index(ctx.clone(), false).await;
    ctx.wait_for_background_reindex().await;
    assert!(ctx.bootstrap_result().unwrap().needs_reindex);
    let chunks: i64 = ctx
        .db
        .with(|c| c.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)))
        .unwrap();
    assert!(chunks > 0);
}

#[tokio::test]
async fn the_startup_index_records_an_embedder_failure() {
    let dir = tempfile::tempdir().unwrap();
    let embedder = Arc::new(FakeEmbedder::instant().gated());
    embedder.release_init(Err("embedder unavailable"));
    let e: Arc<dyn Embedder> = embedder;
    let ctx = context(dir.path(), e);
    run_startup_index(ctx.clone(), true).await;
    assert_eq!(
        ctx.init_error().as_deref(),
        Some("Error: embedder unavailable")
    );
    assert_eq!(ctx.background_reindex_count(), 0);
}
