import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { pruneAllOrphanStubs } from '../store/nodes.js';

export function registerReindexTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'reindex',
    'Re-index the vault: re-embeds notes whose mtime changed and prunes orphan stubs.',
    {},
    async () => {
      // reindex BLOCKS on the embedder being ready. An explicit user call
      // to `reindex` is explicit opt-in to wait — non-blocking polling
      // semantics belong on `search` (which always supported them), not
      // here. Returning `preparing` from reindex broke callers (including
      // the smoke harness) that relied on the synchronous "do the work
      // and return stats" contract.
      await ctx.ensureEmbedderReady();
      // v1.7.20 C8: record a reason so index_status.lastReindexReasons isn't
      // empty after an explicit user-triggered reindex. Distinct from the
      // bootstrap-migration reasons that fire on model/schema change.
      ctx.lastManualReindexReason = 'user-triggered reindex';
      const stats = await ctx.pipeline.index(ctx.config.vaultPath);
      const stubsPruned = pruneAllOrphanStubs(ctx.db);
      return { ...stats, stubsPruned };
    },
  );
}
