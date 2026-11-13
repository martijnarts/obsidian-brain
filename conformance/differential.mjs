// Runs the same tool calls against the TypeScript and the Rust server, each
// on its own copy of one vault, and prints every output that differs. At
// the end it compares the two vaults file by file.
//
//   node conformance/differential.mjs [--only name,name] [--show-same]
//
// Run it from the repository root after `npm run build` and `cargo build`.
// Both servers use Ollama at a dead port, so no model loads and semantic
// search answers with the same "not ready" envelope on both sides.

import { spawn } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { createTwoFilesPatch } from 'diff';

const ROOT = new URL('..', import.meta.url).pathname;
const args = process.argv.slice(2);
const only = args.includes('--only') ? args[args.indexOf('--only') + 1].split(',') : null;
const showSame = args.includes('--show-same');

const SERVERS = {
  ts: ['node', [join(ROOT, 'dist/cli/index.js')]],
  rs: [join(ROOT, 'target/debug/obsidian-brain'), []],
};

function seedVault(dir) {
  cpSync(join(ROOT, 'test/fixtures/vault'), dir, { recursive: true });
  const w = (rel, text) => {
    mkdirSync(join(dir, rel, '..'), { recursive: true });
    writeFileSync(join(dir, rel), text);
  };
  w('Welcome.md', '# Welcome\n\nWelcome to the smoke-test vault. See [[Concepts/Widgets]].\n\n- [ ] Explore the smoke-test vault\n- [x] Done already\n\nCafé ünïcode 🎉 widgets here.\n');
  w('Concepts/Widgets.md', '---\ntype: Concept\n# a comment that must survive\ntags: [widgets, demo]\ncreated: 2024-01-05\n---\n\n# Widgets\n\nWidgets connect back to [[Welcome]] and forward to [[Concepts/Gadgets]] and [[Missing Note]].\n\n## Details #smoke\n\nSee [[Welcome#Welcome]].\n');
  w('Concepts/Gadgets.md', '---\ntype: Concept\ntags:\n  - gadgets\n  - demo\n---\n\n# Gadgets\n\nStub note, no backlinks. #smoke/nested\n\n```\n#notatag [[NotALink]]\n```\n');
  w('Board.canvas', JSON.stringify({ nodes: [{ id: '0123456789abcdef', type: 'text', text: 'Start', x: 0, y: 0, width: 250, height: 60 }], edges: [] }));
  w('Templates/Daily.md', '---\nkind: daily\n---\n# {{title}}\n\nCreated {{date}}.\n');
  w('.obsidian/bookmarks.json', JSON.stringify({ items: [{ type: 'file', path: 'Welcome.md' }] }));
}

// `$prev:<field>` in an argument takes that field from the previous call's
// JSON output on the same server, for ids that differ per run.
const CALLS = [
  ['list_vaults', {}],
  ['search', { query: 'widgets', mode: 'fulltext', limit: 5 }],
  ['search', { query: 'widgets', limit: 5 }],
  ['list_notes', { limit: 20 }],
  ['list_notes', { tag: 'demo', includeStubs: false }],
  ['vault_overview', {}],
  ['list_tags', {}],
  ['list_bookmarks', {}],
  ['list_property_values', { key: 'tags' }],
  ['read_note', { name: 'Widgets' }],
  ['read_note', { name: 'Welcome', mode: 'full' }],
  ['read_note', { name: 'Nope' }],
  ['find_notes_by_name', { query: 'gadg' }],
  ['grep_vault', { query: 'widgets', folder: 'Concepts' }],
  ['grep_vault', { pattern: 'ünï\\w+', regex: true }],
  ['grep_vault', { pattern: '(?<=Caf)é', regex: true }],
  ['query_notes', { filter: { has_tag: [{ var: 'tags' }, 'demo'] }, sort: { field: 'title' } }],
  ['query_notes', { filter: { '==': [{ var: 'type' }, 'Concept'] }, fields: ['type'] }],
  ['read_notes', { names: ['Welcome', 'Widgets'] }],
  ['read_note_part', { name: 'Widgets', mode: 'outline' }],
  ['read_note_part', { name: 'Widgets', mode: 'heading', heading: 'Details' }],
  ['file_info', { path: 'Concepts/Widgets.md' }],
  ['find_connections', { name: 'Welcome', depth: 2 }],
  ['find_path_between', { from: 'Welcome', to: 'Gadgets', maxDepth: 3, includeCommon: true }],
  ['rank_notes', { metric: 'both', limit: 5 }],
  ['rank_notes', { metric: 'influence', limit: 5 }],
  ['list_tasks', { status: 'all' }],
  ['ensure_block_id', { name: 'Welcome', line: 5, dryRun: true }],
  ['find_broken_links', { limit: 10 }],
  ['find_orphaned_notes', { limit: 10 }],
  ['search_and_replace', { pattern: 'widget', replacement: 'gizmo' }],
  ['search_and_replace', { pattern: '(W)idget(s?)', regex: true, replacement: '[$1/$2/$&/$$]', caseSensitive: true }],
  ['rename_tag', { from: 'smoke', to: 'smoked' }],
  ['rename_heading', { name: 'Widgets', from: 'Widgets', to: 'Widget Notes', dryRun: true }],
  ['index_status', {}],
  // Writes.
  ['create_note', { title: 'Smoke Test', content: 'Created by the smoke-test script.\n', frontmatter: { tags: ['smoke'], when: '2026-01-02' } }],
  ['edit_note', { name: 'Smoke Test', mode: 'append', content: ' more text' }],
  ['edit_note', { name: 'Widgets', mode: 'replace_window', search: 'forward to', content: 'onward to', dryRun: true }],
  ['apply_edit_preview', { previewId: '$prev:previewId' }],
  ['edit_note', { name: 'Widgets', mode: 'patch_heading', heading: 'Details', content: 'New details.\n' }],
  ['edit_note', { name: 'Widgets', mode: 'patch_frontmatter', key: 'status', value: 'draft' }],
  ['update_properties', { name: 'Smoke Test', set: { status: 'smoke' }, remove: ['tags'] }],
  ['update_properties', { name: 'Widgets', set: { rating: 4 } }],
  ['set_task_status', { name: 'Welcome', line: 5, status: 'x' }],
  ['link_notes', { source: 'Smoke Test', target: 'Widgets', context: 'Smoke test cross-link' }],
  ['move_note', { source: 'Smoke Test', destination: 'Archive/Smoke Moved.md' }],
  ['delete_note', { name: 'Smoke Moved', confirm: true }],
  ['create_note_from_template', { template: 'Daily', title: 'Smoke Daily', date: '2026-01-02' }],
  ['read_canvas', { path: 'Board.canvas' }],
  ['edit_canvas', { path: 'Board.canvas', operation: 'add_node', type: 'file', file: 'Welcome.md' }],
  ['create_folder', { path: 'Smoke Assets' }],
  ['create_attachment', { path: 'Smoke Assets/pixel.png', content: Buffer.from('smoke').toString('base64') }],
  ['list_attachments', { folder: 'Smoke Assets' }],
  ['delete_folder', { path: 'Smoke Assets', recursive: true, confirm: true }],
  ['rename_heading', { name: 'Widgets', from: 'Widgets', to: 'Widget Notes' }],
  ['rename_tag', { from: 'demo', to: 'example' }],
  ['search_and_replace', { pattern: 'Stub note', replacement: 'Short note', dryRun: false }],
  ['find_broken_links', { limit: 10 }],
];

class Client {
  constructor(cmd, argv, env) {
    this.proc = spawn(cmd, argv, { env, stdio: ['pipe', 'pipe', 'pipe'] });
    this.buf = '';
    this.pending = new Map();
    this.nextId = 1;
    this.stderr = '';
    this.proc.stderr.on('data', (d) => { this.stderr += d; });
    this.proc.stdout.on('data', (d) => {
      this.buf += d;
      let i;
      while ((i = this.buf.indexOf('\n')) >= 0) {
        const line = this.buf.slice(0, i);
        this.buf = this.buf.slice(i + 1);
        if (!line.trim()) continue;
        const msg = JSON.parse(line);
        if (msg.id !== undefined && this.pending.has(msg.id)) {
          this.pending.get(msg.id)(msg);
          this.pending.delete(msg.id);
        }
      }
    });
  }
  request(method, params, timeoutMs = 60000) {
    const id = this.nextId++;
    this.proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(`${method} timed out`)), timeoutMs);
      this.pending.set(id, (m) => { clearTimeout(t); resolve(m); });
    });
  }
  notify(method) {
    this.proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', method }) + '\n');
  }
  async call(name, args) {
    const m = await this.request('tools/call', { name, arguments: { vault: 'v', ...args } });
    if (m.error) return { isError: true, text: `JSON-RPC ${m.error.code}: ${m.error.message}` };
    return { isError: m.result.isError === true, text: m.result.content.map((c) => c.text).join('\n') };
  }
  close() {
    this.proc.stdin.end();
    setTimeout(() => this.proc.kill('SIGKILL'), 3000).unref();
  }
}

async function start(kind, base) {
  const vault = join(base, 'vault');
  seedVault(vault);
  const env = {
    ...process.env,
    DATA_DIR: join(base, 'data'),
    TRANSFORMERS_CACHE: join(base, 'models'),
    EMBEDDING_PROVIDER: 'ollama',
    OLLAMA_BASE_URL: 'http://127.0.0.1:9',
    OLLAMA_EMBEDDING_DIM: '384',
    OBSIDIAN_BRAIN_NO_WATCH: '1',
    TZ: 'UTC',
  };
  const [cmd, argv] = SERVERS[kind];
  const c = new Client(cmd, [...argv, 'server', '--vault', `v=${vault}`], env);
  await c.request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'diff', version: '0' } });
  c.notify('notifications/initialized');
  // Wait for the startup index.
  for (let i = 0; i < 120; i++) {
    const r = await c.call('list_vaults', {});
    try {
      const v = JSON.parse(r.text)[0];
      if (v.notes > 0 && !v.reindexInProgress) break;
    } catch { /* not JSON yet */ }
    await new Promise((res) => setTimeout(res, 250));
  }
  await new Promise((res) => setTimeout(res, 1000));
  return { c, vault, base };
}

function normalize(text, side) {
  return text
    .split(side.vault).join('<VAULT>')
    .split(side.base).join('<BASE>')
    .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z/g, '<ISO>')
    .replace(/\b1[6-9]\d{11}(\.\d+)?\b/g, '<EPOCH_MS>')
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, '<UUID>')
    .replace(/"(previewId|id)": "[0-9a-z-]{8,}"/g, '"$1": "<ID>"')
    .replace(/"(durationMs|elapsedMs|ms)": \d+(\.\d+)?/g, '"$1": <MS>');
}

function resolveArgs(argsIn, prev) {
  return JSON.parse(JSON.stringify(argsIn), (_k, v) => {
    if (typeof v === 'string' && v.startsWith('$prev:')) {
      try { return JSON.parse(prev)[v.slice(6)]; } catch { return v; }
    }
    return v;
  });
}

function walk(dir, out = {}, root = dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out, root);
    else out[relative(root, p)] = readFileSync(p, 'latin1');
  }
  return out;
}

const tmp = mkdtempSync(join(tmpdir(), 'ob-diff-'));
const ts = await start('ts', join(tmp, 'ts'));
const rs = await start('rs', join(tmp, 'rs'));
let same = 0;
let differ = 0;
const prev = { ts: '', rs: '' };
try {
  for (const [i, [name, a]] of CALLS.entries()) {
    const label = `#${i} ${name} ${JSON.stringify(a)}`;
    const out = {};
    for (const [kind, side] of [['ts', ts], ['rs', rs]]) {
      const r = await side.c.call(name, resolveArgs(a, prev[kind]));
      prev[kind] = r.text;
      out[kind] = (r.isError ? '[isError]\n' : '') + normalize(r.text, side);
    }
    if (only && !only.includes(name)) continue;
    if (out.ts === out.rs) {
      same++;
      if (showSame) console.log(`SAME ${label}`);
    } else {
      differ++;
      console.log(`DIFF ${label}`);
      console.log(createTwoFilesPatch('ts', 'rs', out.ts + '\n', out.rs + '\n', '', '', { context: 2 }).split('\n').slice(4).join('\n'));
    }
  }
  const a = walk(ts.vault);
  const b = walk(rs.vault);
  for (const f of [...new Set([...Object.keys(a), ...Object.keys(b)])].sort()) {
    if (a[f] === b[f]) continue;
    differ++;
    console.log(`FILE ${f}`);
    console.log(createTwoFilesPatch(`ts/${f}`, `rs/${f}`, a[f] ?? '<missing>\n', b[f] ?? '<missing>\n', '', '', { context: 2 }).split('\n').slice(4).join('\n'));
  }
} finally {
  ts.c.close();
  rs.c.close();
  setTimeout(() => rmSync(tmp, { recursive: true, force: true }), 500);
}
console.log(`\n${same} same, ${differ} different`);
