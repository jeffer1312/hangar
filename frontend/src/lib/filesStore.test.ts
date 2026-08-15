// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SvelteSet } from 'svelte/reactivity';
import { listFiles, readFile, searchFiles, pathDiff } from './api';
import { FilesStore } from './filesStore.svelte';
import type { TreeEntry } from './types';

// Mock de módulo é `vi.mock`, NUNCA `vi.spyOn` num export: o namespace de um módulo ES é
// somente leitura, e o spy estoura "Cannot redefine property".
vi.mock('./api', () => ({
  listFiles: vi.fn(),
  readFile: vi.fn(),
  searchFiles: vi.fn(),
  pathDiff: vi.fn(),
}));

// Helper das provas: entrada de arvore minima, so com o que o teste afirma.
const ent = (path: string, is_dir: boolean): TreeEntry =>
  ({ name: path.split('/').pop() ?? path, path, is_dir, size: 0, changed: null, add: 0, del: 0 });

describe('FilesStore', () => {
  beforeEach(() => vi.clearAllMocks());

  it('resposta atrasada de um alvo abandonado e descartada', async () => {
    let libera: (v: unknown) => void = () => {};
    vi.mocked(readFile)
      .mockImplementationOnce(() => new Promise((r) => (libera = r)) as never)
      .mockResolvedValueOnce({ path: 'b.txt', text: 'B', size: 1, truncated: false });
    const s = new FilesStore('sessao');
    const primeiro = s.abrir('a.txt');
    await s.abrir('b.txt');
    libera({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    await primeiro;
    expect(s.conteudo?.text).toBe('B'); // o primeiro nao pinta por cima
  });

  it('guarda pasta aberta por sessao', async () => {
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: false });
    const s = new FilesStore('sessao');
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(true);
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(false);
  });

  // Prova do bloqueador 3: o apiFetch lanca Error com a mensagem JA traduzida (errorDetail em
  // api.ts), entao o texto do erro nao pode ganhar prefixo "Error: ".
  it('erro do backend vira a mensagem traduzida, sem "Error:" na frente', async () => {
    vi.mocked(searchFiles).mockRejectedValue(
      Object.assign(new Error('a busca precisa de um repositorio git'), { status: 409 }));
    const s = new FilesStore('sessao');
    await s.buscar('x', 'contents');
    expect(s.erro).toBe('a busca precisa de um repositorio git');
  });

  // Prova do bloqueador 1: operacoes diferentes tem contadores diferentes — expandir uma pasta
  // no meio nao pode cancelar, calado, o arquivo que esta abrindo.
  it('expandir uma pasta nao descarta o arquivo que esta abrindo', async () => {
    let libera: (v: unknown) => void = () => {};
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((r) => (libera = r)) as never);
    vi.mocked(pathDiff).mockResolvedValue({ path: 'a.txt', diff: 'd' } as never);
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: false });
    const s = new FilesStore('sessao');
    const clique = s.abrir('a.txt');
    await s.alternarPasta('src'); // outra operacao no meio
    libera({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    await clique;
    expect(s.conteudo?.text).toBe('A');
  });

  // Prova do bloqueador 2: o conteudo MANDA. Fora de repositorio git o path_diff responde 409 e
  // a arvore tem que continuar lendo arquivo (regra do usuario, 15/08).
  it('fora de repositorio git, o arquivo abre mesmo sem diff', async () => {
    vi.mocked(readFile).mockResolvedValue({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    vi.mocked(pathDiff).mockRejectedValue(new Error('nao e um repositorio git'));
    const s = new FilesStore('sessao');
    await s.abrir('a.txt');
    expect(s.conteudo?.text).toBe('A');
    expect(s.diff).toBeNull();
    expect(s.erro).toBeNull();
  });

  // Bloqueador 4: `abertos` e um SvelteSet de verdade. A reatividade dele vem do tipo (o runtime
  // do svelte nao faz proxy de Set cru dentro de $state — svelte/internal/client/proxy.js), e o
  // teste de repintura montado so tem como existir na Task 10, sobre a arvore.
  it('abertos e um SvelteSet, nao um Set cru', () => {
    const s = new FilesStore('sessao');
    expect(s.abertos).toBeInstanceOf(SvelteSet);
  });

  // Prova do bloqueador 5: a arvore mostra varias pastas abertas ao mesmo tempo, na ordem da
  // barra — o store entrega a lista achatada (raiz + filhos de cada aberta), nao um diretorio so.
  it('mantem varias pastas abertas ao mesmo tempo', async () => {
    vi.mocked(listFiles).mockImplementation(async (_s, path) => ({
      truncated: false,
      entries: path === undefined
        ? [ent('backend', true), ent('docs', true)]
        : path === 'backend' ? [ent('backend/app', true)] : [ent('docs/x.md', false)],
    }));
    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('backend');
    await s.alternarPasta('docs');
    expect(s.entries.map((e) => e.path))
      .toEqual(['backend', 'backend/app', 'docs', 'docs/x.md']);
  });

  // Prova do bloqueador 6: o store recebe o VALOR, nao o evento — quem le o controle da tela e o
  // componente (o seletor da barra e um botao, nao um <select>).
  it('trocar o escopo reabre o arquivo no escopo novo', async () => {
    vi.mocked(readFile).mockResolvedValue({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    vi.mocked(pathDiff).mockImplementation(async (_s, _p, escopo) =>
      ({ path: 'a.txt', diff: escopo, truncated: false }) as never);
    const s = new FilesStore('sessao');
    await s.abrir('a.txt');
    await s.trocarEscopo('nao_commitado');
    expect(s.escopo).toBe('nao_commitado');
    expect(s.diff?.diff).toBe('nao_commitado');
  });

  // Prova do bloqueador 7: o aviso de corte do backend nao pode morrer no store — uma arvore
  // cortada em 1000 entradas nao pode parecer completa.
  it('guarda o aviso de corte que o backend manda', async () => {
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: true });
    vi.mocked(searchFiles).mockResolvedValue({ hits: [], truncated: true, mode: 'names' });
    const s = new FilesStore('sessao');
    await s.recarregar();
    expect(s.listaCortada).toBe(true);
    await s.buscar('x', 'names');
    expect(s.buscaCortada).toBe(true);
  });
});
