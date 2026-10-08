import { describe, it, expect, afterEach } from 'vitest';
import { resolveConfig, resolveDataDir } from '../src/config.js';

describe('resolveConfig', () => {
  const originalEnv = { ...process.env };

  afterEach(() => {
    process.env = { ...originalEnv };
  });

  it('takes the vault path from its argument', () => {
    const config = resolveConfig({ vaultPath: '/tmp/cli-vault' });
    expect(config.vaultPath).toBe('/tmp/cli-vault');
  });

  it('ignores the VAULT_PATH env var, which nothing reads any more', () => {
    process.env.VAULT_PATH = '/tmp/env-vault';
    process.env.KG_VAULT_PATH = '/tmp/legacy-vault';
    const config = resolveConfig({ vaultPath: '/tmp/cli-vault' });
    expect(config.vaultPath).toBe('/tmp/cli-vault');
  });

  it('uses the data dir override before DATA_DIR', () => {
    process.env.DATA_DIR = '/tmp/env-data';
    const config = resolveConfig({ vaultPath: '/tmp/vault', dataDir: '/tmp/data/notes' });
    expect(config.dataDir).toBe('/tmp/data/notes');
  });

  it('dbPath is under dataDir', () => {
    const config = resolveConfig({ vaultPath: '/tmp/vault', dataDir: '/tmp/data/notes' });
    expect(config.dbPath).toBe('/tmp/data/notes/kg.db');
  });
});

describe('resolveDataDir', () => {
  const originalEnv = { ...process.env };

  afterEach(() => {
    process.env = { ...originalEnv };
  });

  it('prefers the override', () => {
    process.env.DATA_DIR = '/tmp/env-data';
    expect(resolveDataDir('/tmp/override')).toBe('/tmp/override');
  });

  it('defaults to XDG_DATA_HOME/obsidian-brain', () => {
    process.env.XDG_DATA_HOME = '/tmp/xdg';
    delete process.env.DATA_DIR;
    delete process.env.KG_DATA_DIR;
    expect(resolveDataDir()).toBe('/tmp/xdg/obsidian-brain');
  });

  it('reads DATA_DIR', () => {
    process.env.DATA_DIR = '/tmp/custom-data';
    delete process.env.KG_DATA_DIR;
    expect(resolveDataDir()).toBe('/tmp/custom-data');
  });

  it('reads the legacy KG_DATA_DIR', () => {
    delete process.env.DATA_DIR;
    process.env.KG_DATA_DIR = '/tmp/legacy-data';
    expect(resolveDataDir()).toBe('/tmp/legacy-data');
  });

  it('falls back to ~/.local/share/obsidian-brain when XDG is not set', () => {
    delete process.env.XDG_DATA_HOME;
    delete process.env.DATA_DIR;
    delete process.env.KG_DATA_DIR;
    expect(resolveDataDir()).toContain('.local/share/obsidian-brain');
  });
});
