// Reproducao do revisor (arv-review9), Task 10: o 404 do listFiles NAO chega com "404:" no texto.
// O erro real vem de api.ts:ensureOk -> new Error(await errorDetail(res)) com .status = 404, e a
// MENSAGEM e limpa (o comentario de api.ts:533 diz isso com todas as letras). O teste do commit
// fabrica `new Error('404: ...')`, um shape que o app nunca produz.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { FilesStore } from './filesStore.svelte';
import { listFiles, readFile, searchFiles, pathDiff } from './api';
import { overwriteGetLocale } from '../paraglide/runtime';

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
});

// O 404 REAL: mensagem limpa (o texto traduzido do envelope) + .status.
const erro404 = () =>
  Object.assign(new Error('Nao deu pra acessar esse arquivo ou pasta.'), { status: 404 });

describe('404 real (mensagem limpa + .status) — o commit le o TEXTO', () => {
  // Padrao dos testes de texto deste projeto (recorte da Task 10): sem isto o paraglide
  // responde no idioma base (en) e a assercao literal em pt falha.
  beforeEach(() => {
    overwriteGetLocale(() => 'pt');
    vi.clearAllMocks();
  });

  it('pasta que sumiu do disco deveria ser podada', async () => {
    vi.mocked(listFiles).mockImplementation(async (_s: string, path?: string) => {
      if (path === undefined) return { entries: [ent('src', true)], truncated: false } as never;
      if (path === 'src') return { entries: [ent('src/x.ts', false)], truncated: true } as never;
      return { entries: [], truncated: false } as never;
    });
    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(true);

    // a pasta some do disco
    vi.mocked(listFiles).mockImplementation(async (_s: string, path?: string) => {
      if (path === undefined) return { entries: [ent('src', true)], truncated: false } as never;
      if (path === 'src') throw erro404();
      return { entries: [], truncated: false } as never;
    });
    await s.recarregar();

    expect(s.abertos.has('src')).toBe(false);   // poda
    expect(s.listaCortada).toBe(false);
    expect(s.erro).toBeNull();
  });

  it('404 na raiz deveria virar o aviso de sessao encerrada', async () => {
    vi.mocked(listFiles).mockRejectedValueOnce(erro404());
    const s = new FilesStore('sessao');
    await s.recarregar();
    expect(s.erro).toBe('Esta sessão foi encerrada.');
  });
});
