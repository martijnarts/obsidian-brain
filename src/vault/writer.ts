import { existsSync, mkdirSync, readFileSync, writeFileSync, appendFileSync } from 'fs';
import { basename, dirname } from 'path';
import matter from 'gray-matter';
import type { DatabaseHandle } from '../store/db.js';
import { upsertNode } from '../store/nodes.js';
import { insertEdge } from '../store/edges.js';
import { resolveVaultPath } from './vault-path.js';
import { debugLog } from '../util/debug-log.js';

debugLog('module-load: src/vault/writer.ts');

export interface CreateNodeOptions {
  title: string;
  directory?: string;
  frontmatter: Record<string, unknown>;
  content: string;
}

export class VaultWriter {
  constructor(
    private vaultPath: string,
    private db: DatabaseHandle,
  ) {}

  createNode(opts: CreateNodeOptions): string {
    // The directory and the title both come from the client, and the title
    // becomes the file name, so the guard checks the whole path.
    const filename = `${opts.title}.md`;
    const { rel: relPath, abs: absPath } = resolveVaultPath(
      this.vaultPath,
      opts.directory ? `${opts.directory}/${filename}` : filename,
    );
    mkdirSync(dirname(absPath), { recursive: true });

    if (existsSync(absPath)) {
      throw new Error(`File already exists: ${relPath}`);
    }

    // Default: auto-inject `title` into frontmatter matching the note's title.
    // Opt-out: caller passes `frontmatter: { title: null }` to suppress
    // injection (the null marker is dropped, no key written).
    // Override: caller passes `frontmatter: { title: 'Custom' }` to set their own.
    const fm: Record<string, unknown> = { ...opts.frontmatter };
    if (!('title' in fm)) {
      fm.title = opts.title;
    } else if (fm.title === null) {
      delete fm.title;
    }
    const fileContent = matter.stringify(opts.content, fm);
    writeFileSync(absPath, fileContent, 'utf-8');

    // Index in store
    this.indexFile(relPath);

    return relPath;
  }

  annotateNode(nodeId: string, content: string): void {
    const absPath = resolveVaultPath(this.vaultPath, nodeId).abs;
    if (!existsSync(absPath)) {
      throw new Error(`Node not found: ${nodeId}`);
    }

    appendFileSync(absPath, content, 'utf-8');

    // Re-index
    this.indexFile(nodeId);
  }

  addLink(sourceId: string, targetRef: string, context: string): void {
    const absPath = resolveVaultPath(this.vaultPath, sourceId).abs;
    if (!existsSync(absPath)) {
      throw new Error(`Source node not found: ${sourceId}`);
    }

    const line = `\n${context} [[${targetRef}]]`;
    appendFileSync(absPath, line, 'utf-8');

    // Re-index source node
    this.indexFile(sourceId);

    // Add edge to store
    const targetId = targetRef.endsWith('.md') ? targetRef : targetRef + '.md';
    insertEdge(this.db, {
      sourceId,
      targetId,
      context,
    });
  }

  private indexFile(relPath: string): void {
    const absPath = resolveVaultPath(this.vaultPath, relPath).abs;
    const raw = readFileSync(absPath, 'utf-8');

    let fm: Record<string, unknown>;
    let content: string;
    try {
      const parsed = matter(raw);
      fm = parsed.data;
      content = parsed.content;
    } catch {
      fm = {};
      content = raw;
    }

    const title = (fm.title as string) ?? basename(relPath, '.md');

    upsertNode(this.db, {
      id: relPath,
      title,
      content,
      frontmatter: fm,
    });
  }
}
