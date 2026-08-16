import { describe, it, expect, vi } from 'vitest';
import { FilesStore } from './filesStore.svelte';
import { listFiles } from './api';

vi.mock('./api', () => ({
  listFiles: vi.fn(),
  readFile: vi.fn(),
  searchFiles: vi.fn(),
  pathDiff: vi.fn(),
}));

const ent = (path: string, is_dir: boolean) => ({
  path,
  name: path.split('/').pop()!,
  is_dir,
  changed: null,
  added: null,
  deleted: null,
  size: 0,
  add: 0,
  del: 0,
});
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((r, j) => { resolve = r; reject = j; });
  return { promise, resolve, reject };
};
const erro404 = () => Object.assign(new Error('Nao deu pra acessar esse arquivo ou pasta.'), { status: 404 });

describe('404 de pasta aberta aninhada', () => {
  it('poda a pasta e todos os descendentes mesmo com resposta atrasada', async () => {
    const src404 = deferred<never>();
    const childOk = deferred<{ entries: ReturnType<typeof ent>[]; truncated: boolean }>();
    let fase = 0;
    vi.mocked(listFiles).mockImplementation(async (_s: string, path?: string) => {
      if (fase === 0) {
        if (path === undefined) return { entries: [ent('src', true)], truncated: false };
        if (path === 'src') return { entries: [ent('src/lib', true)], truncated: false };
        return { entries: [ent('src/lib/a.ts', false)], truncated: true };
      }
      if (path === undefined) return { entries: [ent('src', true)], truncated: false };
      if (path === 'src') throw erro404();
      return childOk.promise;
    });

    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('src');
    await s.alternarPasta('src/lib');
    expect(s.abertos.has('src/lib')).toBe(true);
    expect(s.listaCortada).toBe(true);

    fase = 1;
    const recarga = s.recarregar();
    childOk.resolve({ entries: [ent('src/lib/a.ts', false)], truncated: true });
    await recarga;

    expect(s.abertos.has('src')).toBe(false);
    expect(s.abertos.has('src/lib')).toBe(false);
    expect(s.listaCortada).toBe(false);
  });
});
