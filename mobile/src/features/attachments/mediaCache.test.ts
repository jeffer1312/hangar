import { expect, it, vi } from 'vitest';

const fs = vi.hoisted(() => ({ existing: new Set<string>() }));
vi.mock('expo-file-system', () => ({
  Paths: { cache: 'cache' },
  Directory: class { create() {} },
  File: class {
    name: string;
    constructor(_dir: unknown, name: string) { this.name = name; }
    get exists() { return fs.existing.has(this.name); }
  },
}));
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: vi.fn() }));
vi.mock('react-native', () => ({ Platform: { OS: 'ios' }, Share: { share: vi.fn() } }));
vi.mock('../../paraglide/messages', () => ({}));

import { keptCopy, keyHash } from './mediaCache';
import { keptKey } from './attachmentUrls';

const item = { source: 'cited' as const, ref: '/tmp/out/report.pdf' };

it('a file regenerated with another date or size is not found as the old kept copy', () => {
  const before = keptKey('s1', 'sess', { ...item, mtime: 100, size: 2048 });
  const after = keptKey('s1', 'sess', { ...item, mtime: 200, size: 2048 });
  const resized = keptKey('s1', 'sess', { ...item, mtime: 100, size: 4096 });
  expect(new Set([before, after, resized]).size).toBe(3);
  fs.existing.clear();
  fs.existing.add(`${keyHash(before)}-report.pdf`);
  expect(keptCopy(before, 'report.pdf')?.name).toBe(`${keyHash(before)}-report.pdf`);
  expect(keptCopy(after, 'report.pdf')).toBeNull();
  expect(keptCopy(resized, 'report.pdf')).toBeNull();
});

it('the name hash is stable and wider than 32 bits', () => {
  const key = keptKey('s1', 'sess', { ...item, mtime: 1, size: 2 });
  expect(keyHash(key)).toBe(keyHash(key));
  // Duas metades de 53 bits: bem além dos ~7 caracteres base36 de um hash de 32 bits.
  expect(keyHash(key).length).toBeGreaterThan(14);
  const many = new Set(Array.from({ length: 5000 }, (_, i) => keyHash(`s1::sess::upload:f${i}.png::${i}:10`)));
  expect(many.size).toBe(5000);
});
