import { homedir } from 'os';
import { join } from 'path';
import { debugLog } from './util/debug-log.js';

debugLog('module-load: src/config.ts');

export interface Config {
  vaultPath: string;
  dataDir: string;
  dbPath: string;
}

export interface ConfigOverrides {
  vaultPath: string;
  dataDir?: string;
}

/**
 * Resolve the data dir: the override, then DATA_DIR (legacy alias
 * KG_DATA_DIR), then `$XDG_DATA_HOME/obsidian-brain`. Each vault keeps its
 * index in `<dataDir>/<name>`.
 */
export function resolveDataDir(override?: string): string {
  const xdgData = process.env.XDG_DATA_HOME
    ?? join(homedir(), '.local', 'share');
  return override
    ?? process.env.DATA_DIR
    ?? process.env.KG_DATA_DIR
    ?? join(xdgData, 'obsidian-brain');
}

/**
 * The config of one vault. The vault path comes from a --vault flag; the
 * caller passes the vault's own data dir.
 */
export function resolveConfig(overrides: ConfigOverrides): Config {
  const dataDir = resolveDataDir(overrides.dataDir);
  return {
    vaultPath: overrides.vaultPath,
    dataDir,
    dbPath: join(dataDir, 'kg.db'),
  };
}
