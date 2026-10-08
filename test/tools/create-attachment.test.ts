import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdir, mkdtemp, readFile, readdir, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { registerCreateAttachmentTool } from '../../src/tools/create-attachment.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

const BYTES = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x00, 0xff, 0x10]);
const B64 = BYTES.toString('base64');

describe('tools/create_attachment', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    tool = loadTool(registerCreateAttachmentTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('writes the decoded bytes and creates missing parent folders', async () => {
    const data = ok(await tool.call({ path: 'assets/img/pic.png', content: B64 }));
    expect(data).toEqual({ path: 'assets/img/pic.png', bytesWritten: BYTES.length, overwritten: false });
    expect(await readFile(join(h.vault, 'assets/img/pic.png'))).toEqual(BYTES);
    expect(await readdir(join(h.vault, 'assets/img'))).toEqual(['pic.png']);
  });

  it('accepts base64 with line breaks and without padding', async () => {
    const wrapped = `${B64.slice(0, 4)}\n${B64.slice(4)}`.replace(/=+$/, '');
    ok(await tool.call({ path: 'a.bin', content: wrapped }));
    expect(await readFile(join(h.vault, 'a.bin'))).toEqual(BYTES);
  });

  it('refuses to replace an existing file unless overwrite is true', async () => {
    await h.write('pic.png', 'old');
    expect(err(await tool.call({ path: 'pic.png', content: B64 }))).toMatch(/already exists.*overwrite: true/);
    expect(err(await tool.call({ path: 'pic.png', content: B64, overwrite: false }))).toMatch(/already exists/);
    expect(await readFile(join(h.vault, 'pic.png'), 'utf-8')).toBe('old');

    const data = ok(await tool.call({ path: 'pic.png', content: B64, overwrite: true }));
    expect(data.overwritten).toBe(true);
    expect(await readFile(join(h.vault, 'pic.png'))).toEqual(BYTES);
  });

  it('refuses markdown, a folder, the root and invalid base64', async () => {
    await mkdir(join(h.vault, 'dir'));
    expect(err(await tool.call({ path: 'Note.MD', content: B64 }))).toMatch(/create_note/);
    expect(err(await tool.call({ path: 'dir', content: B64, overwrite: true }))).toMatch(/is a folder/);
    expect(err(await tool.call({ path: '', content: B64 }))).toMatch(/vault root/);
    expect(err(await tool.call({ path: 'x.bin', content: 'not base64!' }))).toMatch(/not valid base64/);
    expect(err(await tool.call({ path: 'x.bin', content: 'abcde' }))).toMatch(/not valid base64/);
    expect(err(await tool.call({ path: 'x.bin', content: 'ab=c' }))).toMatch(/not valid base64/);
    expect(await readdir(h.vault)).toEqual(['dir']);
  });

  it('caps the decoded size at 10 MB', async () => {
    const limit = 10 * 1024 * 1024;
    const tooBig = Buffer.alloc(limit + 1).toString('base64');
    expect(err(await tool.call({ path: 'big.bin', content: tooBig }))).toMatch(/limit is 10485760 bytes/);
    const atLimit = Buffer.alloc(limit).toString('base64');
    expect(ok(await tool.call({ path: 'max.bin', content: atLimit })).bytesWritten).toBe(limit);
  });

  it('refuses paths outside the vault', async () => {
    expect(err(await tool.call({ path: '../x.png', content: B64 }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ path: '/tmp/x.png', content: B64 }))).toMatch(/outside the vault/);
    const outside = await mkdtemp(join(tmpdir(), 'kg-outside-'));
    try {
      await symlink(outside, join(h.vault, 'out'));
      expect(err(await tool.call({ path: 'out/x.png', content: B64 }))).toMatch(/outside the vault/);
      expect(await readdir(outside)).toEqual([]);
    } finally {
      await rm(outside, { recursive: true, force: true });
    }
  });
});
