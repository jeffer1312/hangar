import { beforeEach, expect, it, vi } from 'vitest';

// Disco de mentira: nome do arquivo → conteúdo. O tamanho é o do conteúdo.
const fs = vi.hoisted(() => ({ files: new Map<string, string>(), onDownload: null as null | (() => void) }));
vi.mock('expo-file-system', () => {
  class File {
    name: string;
    constructor(_dir: unknown, name: string) { this.name = name; }
    get exists() { return fs.files.has(this.name); }
    get size() { return fs.files.get(this.name)?.length ?? 0; }
    get parentDirectory() { return {}; }
    textSync() { return fs.files.get(this.name) ?? ''; }
    write(text: string) { fs.files.set(this.name, text); }
    delete() { fs.files.delete(this.name); }
    moveSync(to: File) { fs.files.set(to.name, fs.files.get(this.name)!); fs.files.delete(this.name); }
    static createDownloadTask(_uri: string, tmp: File) {
      return { downloadAsync: async () => { fs.onDownload?.(); fs.files.set(tmp.name, 'x'.repeat(10)); return {}; } };
    }
  }
  return { Paths: { cache: 'cache' }, Directory: class { create() {} }, File };
});
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: vi.fn() }));
vi.mock('react-native', () => ({ Platform: { OS: 'ios' }, Share: { share: vi.fn() } }));
vi.mock('../../paraglide/messages', () => ({}));

import { downloadKept, keptCopy, keyHash } from './mediaCache';
import { keptIsCurrent, keptKey, type KeptVersion } from './attachmentUrls';

const item = { source: 'cited' as const, ref: '/tmp/out/report.pdf' };
const key = keptKey('s1', 'sess', item);
const download = (version: () => KeptVersion) =>
  downloadKept(key, 'https://a.test/f', undefined, 'report.pdf', version, () => {}, new AbortController().signal);

beforeEach(() => { fs.files.clear(); fs.onDownload = null; });

it('the key is the path only: sizes arriving later do not move the copy', () => {
  expect(key).toBe('s1::sess::cited:/tmp/out/report.pdf');
  expect(keptKey('s2', 'sess', item)).not.toBe(key);
  expect(keptKey('s1', 'other', item)).not.toBe(key);
});

it('size and date measured during the download are stored with the copy and find it afterwards', async () => {
  let now: KeptVersion = { size: null, mtime: null };
  fs.onDownload = () => { now = { size: 10, mtime: 5 }; };
  const file = await download(() => now);
  expect(file.name).toBe(`${keyHash(key)}-report.pdf`);
  expect(keptCopy(key, 'report.pdf', { size: null, mtime: null })?.name).toBe(file.name);
  expect(keptCopy(key, 'report.pdf', { size: 10, mtime: 5 })?.name).toBe(file.name);
});

it('a file rewritten on the server with another date or size does not open the stale copy', async () => {
  await download(() => ({ size: 10, mtime: 5 }));
  expect(keptCopy(key, 'report.pdf', { size: 10, mtime: 6 })).toBeNull();
  expect(keptCopy(key, 'report.pdf', { size: 99, mtime: 5 })).toBeNull();
});

it('a copy downloaded before any measure still checks the size against its bytes', async () => {
  await download(() => ({ size: null, mtime: null }));
  expect(keptCopy(key, 'report.pdf', { size: 10, mtime: 5 })).not.toBeNull();
  expect(keptCopy(key, 'report.pdf', { size: 11, mtime: 5 })).toBeNull();
});

it('only what both sides know and differs discards the copy', () => {
  expect(keptIsCurrent(null, null, { size: 1, mtime: 1 })).toBe(true);
  expect(keptIsCurrent({ size: 1, mtime: 1 }, 1, { size: null, mtime: null })).toBe(true);
  expect(keptIsCurrent({ size: 1, mtime: 1 }, 1, { size: 1, mtime: 2 })).toBe(false);
  expect(keptIsCurrent({ size: null, mtime: null }, 1, { size: 2, mtime: 2 })).toBe(false);
});

it('the name hash is stable and wider than 32 bits', () => {
  expect(keyHash(key)).toBe(keyHash(key));
  // Duas metades de 53 bits: bem além dos ~7 caracteres base36 de um hash de 32 bits.
  expect(keyHash(key).length).toBeGreaterThan(14);
  const many = new Set(Array.from({ length: 5000 }, (_, i) => keyHash(`s1::sess::upload:f${i}.png`)));
  expect(many.size).toBe(5000);
});
