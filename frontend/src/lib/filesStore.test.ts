// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { listFiles, readFile, searchFiles } from './api';
import { FilesStore } from './filesStore.svelte';

// Mock de módulo é `vi.mock`, NUNCA `vi.spyOn` num export: o namespace de um módulo ES é
// somente leitura, e o spy estoura "Cannot redefine property".
vi.mock('./api', () => ({
  listFiles: vi.fn(),
  readFile: vi.fn(),
  searchFiles: vi.fn(),
  pathDiff: vi.fn(),
}));

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

  it('erro do backend vira mensagem, nao excecao solta', async () => {
    vi.mocked(searchFiles).mockRejectedValue({
      detail: { code: 'erro_arq_nao_e_repo_git', params: {}, msg: 'x' },
    });
    const s = new FilesStore('sessao');
    await s.buscar('x', 'contents');
    expect(s.erro).toBeTruthy();
  });
});
