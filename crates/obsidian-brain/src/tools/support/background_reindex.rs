//! The reindex every write tool queues after its write has landed on
//! disk, to bring the index into eventual consistency.

use std::sync::Arc;

use crate::context::VaultContext;

/// Queue a full incremental index of the vault behind any queued before.
/// Returns at once; a failure is logged by the queue and never reaches the
/// caller. Tests await `ctx.wait_for_background_reindex()`.
pub fn run_background_reindex(ctx: &Arc<VaultContext>) {
    let job = ctx.clone();
    ctx.enqueue_background_reindex(async move {
        job.ensure_embedder_ready().await?;
        job.pipeline.index(&job.config.vault_path).await?;
        Ok(())
    });
}
