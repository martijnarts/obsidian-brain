import type { ServerContext } from '../../src/context.js';
import { registerTools } from '../../src/server.js';
import { VaultTools } from '../../src/vaults.js';

/**
 * Tools a server exposes: every per-vault tool from `registerTools` plus
 * `list_vaults`. Derived, so adding a tool does not mean editing counts in
 * every test. Handlers never run, so an empty context is enough.
 */
export const EXPOSED_TOOL_COUNT = new VaultTools(
  [{ name: 'notes', ctx: {} as ServerContext, watcher: null }],
  registerTools,
).names().length;
