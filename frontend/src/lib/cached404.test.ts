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
  path, name: path.split('/').pop()!, is_dir, changed: null, added: null, deleted: null,
  size: 0, add: 0, del: 0,
});
const erro404 = () => Object.assign(new Error('Nao deu pra acessar esse arquivo ou pasta.'), { status: 404 });

describe('404 do pai com descendente colapsado', () => {
  it('descarta cache de descendente colapsado antes de reabrir a árvore', async () => {
    let fase: 'normal' | 'sumiu' | 'voltou' = 'normal';
    vi.mocked(listFiles).mockImplementation(async (_s: string, path?: string) => {
      if (fase === 'sumiu' && path === 'src') throw erro404();
      if (path === undefined) return { entries: [ent('src', true)], truncated: false };
      if (path === 'src') return { entries: [ent('src/lib', true)], truncated: false };
      return { entries: [ent(`src/lib/${fase === 'voltou' ? 'novo.ts' : 'velho.ts'}`, false)], truncated: true };
    });
    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('src');
    await s.alternarPasta('src/lib');
    await s.alternarPasta('src/lib'); // colapsa, mas mantém o cache
    fase = 'sumiu';
    await s.recarregar();
    fase = 'voltou';
    await s.alternarPasta('src');
    await s.alternarPasta('src/lib');
    expect(s.entries.some((e) => e.path === 'src/lib/velho.ts')).toBe(false);
    expect(s.entries.some((e) => e.path === 'src/lib/novo.ts')).toBe(true);
  });
});
