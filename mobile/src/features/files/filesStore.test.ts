import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@hangar/core', async () => {
  const actual = await vi.importActual<typeof import('@hangar/core')>('@hangar/core');
  return {
    ...actual,
    listFiles: vi.fn(),
    readFile: vi.fn(),
    searchFiles: vi.fn(),
    pathDiff: vi.fn(),
    writeFile: vi.fn(),
    discardFile: vi.fn(),
  };
});

import { filesStore, _resetFilesForTests } from './filesStore';
import * as core from '@hangar/core';

const sessao = 'sess-test';
const serverId = 'srv1';

function apis() {
  return filesStore(serverId, sessao);
}

beforeEach(() => {
  _resetFilesForTests();
  vi.clearAllMocks();
  // default mocks: list empty, diff empty
  vi.mocked(core.listFiles).mockResolvedValue({ entries: [], truncated: false });
  vi.mocked(core.pathDiff).mockResolvedValue({
    path: 'a.txt',
    diff: '',
    truncated: false,
    escopo_pedido: 'branch',
    escopo_usado: 'branch',
    base: null,
    motivo: null,
    original: null,
  });
});

describe('filesStore', () => {
  it('descarta resposta atrasada de abrir quando outra abertura vence', async () => {
    const readFile = vi.mocked(core.readFile);
    let resolveA!: (v: { path: string; text: string; size: number; truncated: boolean; digest: string | null }) => void;
    let resolveB!: (v: typeof resolveA extends (v: infer T) => void ? T : never) => void;
    readFile
      .mockReturnValueOnce(new Promise((r) => (resolveA = r)))
      .mockReturnValueOnce(new Promise((r) => (resolveB = r)));

    const api = apis();
    const pA = api.abrir('a.txt');
    const pB = api.abrir('b.txt');

    // resolve B primeiro (mais nova)
    resolveB({ path: 'b.txt', text: 'conteudo b', size: 10, truncated: false, digest: 'd2' });
    await pB;
    // resolve A atrasada (deve ser descartada)
    resolveA({ path: 'a.txt', text: 'conteudo a', size: 10, truncated: false, digest: 'd1' });
    await pA;

    // deve permanecer com b
    expect(api.use.getState().selecionado).toBe('b.txt');
    expect(api.use.getState().conteudo?.text).toBe('conteudo b');
  });

  it('salvar atualiza conteudo com novo digest e chama recarregarDiff', async () => {
    const readFile = vi.mocked(core.readFile);
    const pathDiffMock = vi.mocked(core.pathDiff);
    const writeFile = vi.mocked(core.writeFile);

    readFile.mockResolvedValue({ path: 'a.txt', text: 'old', size: 3, truncated: false, digest: 'd-old' } as never);
    pathDiffMock.mockResolvedValue({
      path: 'a.txt',
      diff: 'diffold',
      truncated: false,
      escopo_pedido: 'branch',
      escopo_usado: 'branch',
      base: null,
      motivo: null,
      original: null,
    } as never);

    const api = apis();
    await api.abrir('a.txt');
    expect(api.use.getState().conteudo?.digest).toBe('d-old');

    writeFile.mockResolvedValue({ path: 'a.txt', size: 3, digest: 'd-new' } as never);
    // pathDiff vai ser chamado por recarregarDiff após salvar
    pathDiffMock.mockResolvedValue({
      path: 'a.txt',
      diff: 'diffnew',
      truncated: false,
      escopo_pedido: 'branch',
      escopo_usado: 'branch',
      base: null,
      motivo: null,
      original: null,
    } as never);

    const err = await api.salvar('a.txt', 'new');
    expect(err).toBeNull();
    expect(api.use.getState().conteudo?.text).toBe('new');
    expect(api.use.getState().conteudo?.digest).toBe('d-new');
    expect(writeFile).toHaveBeenCalledWith(sessao, 'a.txt', 'new', 'd-old');
    // espera o void recarregarDiff(path) — é fire-and-forget, precisa tick
    await new Promise((r) => setTimeout(r, 20));
    expect(pathDiffMock).toHaveBeenLastCalledWith(sessao, 'a.txt', 'branch');
    expect(api.use.getState().diff?.diff).toBe('diffnew');
  });

  it('salvar retorna erro quando digest incompatível e não altera conteudo', async () => {
    const readFile = vi.mocked(core.readFile);
    readFile.mockResolvedValue({ path: 'a.txt', text: 'old', size: 3, truncated: false, digest: 'd-old' } as never);
    const api = apis();
    await api.abrir('a.txt');

    vi.mocked(core.writeFile).mockRejectedValue(new Error('409: erro_arq_mudou_no_disco'));

    const msg = await api.salvar('a.txt', 'new');
    expect(msg).toContain('erro_arq_mudou_no_disco');
    // mantém old
    expect(api.use.getState().conteudo?.text).toBe('old');
  });

  it('404 ao abrir remove hit dos resultados quando busca gravada ainda é a mesma', async () => {
    const api = apis();
    // simula busca prévia
    vi.mocked(core.searchFiles).mockResolvedValue({
      hits: [{ path: 'a.txt', line: 1, text: 'hit' }],
      truncated: false,
      mode: 'names',
    } as never);
    await api.buscar('a', 'names');
    expect(api.use.getState().resultados).toHaveLength(1);

    // abrir a.txt com 404
    const err404 = Object.assign(new Error('404: erro_arq_inexistente'), { status: 404 });
    vi.mocked(core.readFile).mockRejectedValue(err404);
    // pathDiff neve? abrir faz Promise.allSettled de readFile e pathDiff
    // pathDiff pode resolver mas será descartado por conteúdo falhar
    vi.mocked(core.pathDiff).mockResolvedValue({
      path: 'a.txt',
      diff: '',
      truncated: false,
      escopo_pedido: 'branch',
      escopo_usado: 'branch',
      base: null,
      motivo: null,
      original: null,
    } as never);
    // listFiles para recarregar (precisa resolver)
    vi.mocked(core.listFiles).mockResolvedValue({ entries: [], truncated: false });

    await api.abrir('a.txt');

    // deve remover hit
    expect(api.use.getState().resultados).toHaveLength(0);
    // erro deve ser erro_arq_inexistente traduzido? Na implementação mobile usamos m.erro_arq_inexistente() que retorna string traduzida, mas não podemos prever valor exato. Verifica que erro não é null.
    expect(api.use.getState().erro).toBeTruthy();
  });
});
