//! Ported from the lifecycle cases of `test/vaults.test.ts`. The tool
//! router cases belong to `mcp` and its own tests. `OBSIDIAN_BRAIN_NO_WATCH`
//! is covered end to end, where the environment can be set.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use obsidian_brain::embeddings::share_embedder;
use obsidian_brain::vaults::{VaultSpec, Vaults, index_vaults_in_turn};
use serde_json::json;

use crate::support::FakeEmbedder;

fn specs(vault: &std::path::Path, names: &[&str]) -> Vec<VaultSpec> {
    names
        .iter()
        .map(|n| VaultSpec {
            name: n.to_string(),
            path: vault.to_path_buf(),
        })
        .collect()
}

#[tokio::test]
async fn opens_a_context_per_vault_under_its_own_data_dir_with_one_shared_embedder() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let embedder = Arc::new(FakeEmbedder::instant());
    let vaults = Vaults::open_with(
        &specs(dir.path(), &["one", "two"]),
        &data,
        share_embedder(embedder.clone()),
    )
    .await
    .unwrap();
    let ctxs: Vec<_> = vaults.iter().cloned().collect();
    assert_eq!(ctxs[0].config.data_dir, data.join("one"));
    assert_eq!(ctxs[1].config.data_dir, data.join("two"));
    assert!(ctxs[0].config.db_path.exists());
    assert!(Arc::ptr_eq(&ctxs[0].embedder, &ctxs[1].embedder));
    assert!(ctxs.iter().all(|c| !c.owns_embedder()));

    vaults.close().await;
    assert_eq!(embedder.dispose_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_vault_that_cannot_open_fails_the_open() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    // A regular file where the second vault's data dir should be.
    std::fs::write(data.join("two"), "").unwrap();
    let err = Vaults::open_with(
        &specs(dir.path(), &["one", "two"]),
        &data,
        share_embedder(Arc::new(FakeEmbedder::instant())),
    )
    .await
    .err()
    .unwrap();
    assert!(format!("{err:#}").contains("two"));
}

#[tokio::test]
async fn describe_reports_each_vault() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "# A\n").unwrap();
    let vaults = Vaults::open_with(
        &specs(dir.path(), &["alpha", "beta"]),
        &dir.path().join("data"),
        share_embedder(Arc::new(FakeEmbedder::instant())),
    )
    .await
    .unwrap();
    let alpha = vaults.get("alpha").unwrap();
    alpha.ensure_embedder_ready().await.unwrap();
    alpha.pipeline.index(dir.path()).await.unwrap();
    vaults
        .get("beta")
        .unwrap()
        .set_init_error(&anyhow::anyhow!("embedder unavailable"));
    let path = dir.path().to_str().unwrap();
    assert_eq!(
        vaults.describe(),
        json!([
            {"name": "alpha", "path": path, "notes": 1, "embedderReady": true, "reindexInProgress": false},
            {"name": "beta", "path": path, "notes": 0, "embedderReady": false, "reindexInProgress": false,
             "initError": "Error: embedder unavailable"},
        ])
    );
    vaults.close().await;
}

#[tokio::test]
async fn indexes_vaults_one_at_a_time_and_stops_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let vaults = Vaults::open_with(
        &specs(dir.path(), &["alpha", "beta"]),
        &dir.path().join("data"),
        share_embedder(Arc::new(FakeEmbedder::instant())),
    )
    .await
    .unwrap();
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let run = |ctx: Arc<obsidian_brain::context::VaultContext>, empty: bool| {
        let order = order.clone();
        async move {
            let name = ctx.config.name.clone();
            order
                .lock()
                .unwrap()
                .push(format!("start {name} empty={empty}"));
            let o = order.clone();
            ctx.enqueue_background_reindex(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                o.lock().unwrap().push(format!("done {name}"));
                Ok(())
            });
        }
    };
    index_vaults_in_turn(vaults.iter(), run, || false).await;
    assert_eq!(
        *order.lock().unwrap(),
        [
            "start alpha empty=true",
            "done alpha",
            "start beta empty=true",
            "done beta"
        ]
    );

    let calls = Arc::new(Mutex::new(0));
    let stopped = Arc::new(Mutex::new(false));
    let (c, s) = (calls.clone(), stopped.clone());
    index_vaults_in_turn(
        vaults.iter(),
        move |_ctx, _empty| {
            let (c, s) = (c.clone(), s.clone());
            async move {
                *c.lock().unwrap() += 1;
                *s.lock().unwrap() = true;
            }
        },
        || *stopped.lock().unwrap(),
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), 1);
    vaults.close().await;
}
