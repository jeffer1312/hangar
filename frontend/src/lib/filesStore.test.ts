// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SvelteSet } from 'svelte/reactivity';
import * as m from '../paraglide/messages';
import { listFiles, readFile, searchFiles, pathDiff } from './api';
import { FilesStore, filesStores } from './filesStore.svelte';
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

  // Prova do bloqueador 7: o aviso de corte do backend nao pode morrer no store — e o corte e
  // POR PASTA: `recarregar()` lista a raiz e as abertas em paralelo, entao o aviso da raiz tem
  // que sobreviver a uma subpasta inteira que responda depois dela.
  it('o aviso de corte da raiz sobrevive a uma subpasta inteira', async () => {
    vi.mocked(listFiles).mockImplementation(async (_s, path) =>
      path === undefined
        ? { entries: [ent('src', true)], truncated: true } // a RAIZ foi cortada
        : { entries: [], truncated: false }); // a subpasta, nao
    vi.mocked(searchFiles).mockResolvedValue({ hits: [], truncated: true, mode: 'names' });
    const s = new FilesStore('sessao');
    await s.recarregar();
    expect(s.listaCortada).toBe(true);
    await s.alternarPasta('src');
    expect(s.listaCortada).toBe(true); // hoje vira false
    await s.recarregar();
    expect(s.listaCortada).toBe(true); // hoje vira false
    await s.buscar('x', 'names');
    expect(s.buscaCortada).toBe(true);
  });

  // Task 10, poda (medido no parecer): pasta que sumiu do disco deixa o aviso de corte ligado
  // com a arvore vazia. O 404 do filetree e o sinal — poda abertos/cortePorPasta/porPasta e
  // nao mostra erro: a pasta sumiu, a arvore correta e a que fica.
  it('pasta que sumiu do disco e podada da arvore, sem aviso de corte nem erro', async () => {
    // 1) a pasta abre e o conteudo dela corta: o aviso de corte dela acende
    vi.mocked(listFiles).mockImplementation(async (_s, path) => {
      if (path === undefined) return { entries: [ent('src', true)], truncated: false };
      if (path === 'src') return { entries: [ent('src/x.ts', false)], truncated: true };
      return { entries: [], truncated: false };
    });
    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(true);
    expect(s.listaCortada).toBe(true);
    // 2) a pasta some do disco: o recarregar re-lista e recebe 404 — poda. Shape do erro
    // REAL da API: mensagem limpa (o texto traduzido do envelope) + `.status` na propriedade
    // (ensureOk, api.ts:111) — nunca "404:" no texto.
    vi.mocked(listFiles).mockImplementation(async (_s, path) => {
      if (path === undefined) return { entries: [ent('src', true)], truncated: false };
      if (path === 'src') throw Object.assign(new Error('Nao deu pra acessar esse arquivo ou pasta.'), { status: 404 });
      return { entries: [], truncated: false };
    });
    await s.recarregar();
    expect(s.abertos.has('src')).toBe(false);
    expect(s.listaCortada).toBe(false);     // o aviso de corte morreu junto com a pasta
    expect(s.erro).toBeNull();
  });

  // 404 na RAIZ nao e pasta sumida — e o cwd da sessao que morreu (ou a sessao encerrou):
  // vira aviso visivel, nao poda silenciosa.
  it('404 na raiz vira o aviso de sessao encerrada', async () => {
    vi.mocked(listFiles).mockRejectedValueOnce(
      Object.assign(new Error('sessao nao encontrada'), { status: 404 }));
    const s = new FilesStore('sessao');
    await s.recarregar();
    expect(s.erro).toBe(m.arq_sessao_encerrada());
  });

  // Task 10 (medido ao vivo com um binario): abrir que FALHA desmarca o arquivo — com
  // selecionado marcado e conteudo/diff nulos, o FileViewer cairia no "sem diferencas" sobre
  // um arquivo que nem abriu (o erro sumia no recarregar e a tela mentia).
  it('abrir que falha desmarca o arquivo e limpa o loading', async () => {
    vi.mocked(readFile).mockRejectedValue(
      Object.assign(new Error('Arquivo binário — não dá pra mostrar aqui.'), { status: 415 }));
    const s = new FilesStore('sessao');
    await s.abrir('foto.png');
    expect(s.selecionado).toBeNull();
    expect(s.loading).toBe(false);
    expect(s.erro).toContain('binário');
  });

  // Task 11, Step 3 (linha fantasma): arquivo apagado entre listar e abrir devolve 404 — o
  // store mostra o erro certo E re-lista, senao a linha continua clicavel para sempre
  // apontando pra nada. A recarga e a prova: listFiles roda de novo e a arvore volta sem o
  // arquivo.
  it('404 ao abrir mostra erro_arq_inexistente e recarrega a arvore', async () => {
    vi.mocked(readFile).mockRejectedValue(
      Object.assign(new Error('Esse arquivo não existe mais.'), { status: 404 }));
    // Primeira listagem tem o arquivo; depois que ele "sumiu do disco", a recarga nao o acha.
    let lista = [ent('a.txt', false)];
    vi.mocked(listFiles).mockImplementation(async (_s, path) => ({
      truncated: false,
      entries: path === undefined ? lista : [],
    }));
    const s = new FilesStore('sessao');
    await s.recarregar();
    expect(s.entries.length).toBe(1);
    lista = [];   // o arquivo apagado entre listar e abrir
    await s.abrir('a.txt');
    expect(s.selecionado).toBeNull();
    expect(s.erro).toBe(m.erro_arq_inexistente());
    expect(s.entries.length).toBe(0);   // a linha fantasma saiu da arvore
    expect(listFiles).toHaveBeenCalledTimes(2);
  });

  // Parecer Task 11, B1: o 404 ANTIGO nao pode sobrescrever uma abertura nova. A recuperacao
  // do 404 roda `await recarregar()` no meio; se uma abertura nova comeca antes da recarga
  // terminar, a continuacao antiga nao pode pintar o erro do arquivo abandonado.
  it('404 antigo nao sobrescreve abertura nova (corrida na recuperacao)', async () => {
    // a.txt falha com 404; a LISTAGEM dele fica pendurada (libera manualmente)
    let liberaLista: (v: unknown) => void = () => {};
    vi.mocked(readFile).mockRejectedValueOnce(
      Object.assign(new Error('gone'), { status: 404 }));
    vi.mocked(listFiles).mockImplementationOnce(
      () => new Promise((r) => (liberaLista = r)) as never);
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: false });
    vi.mocked(pathDiff).mockResolvedValue({ path: 'new.ts', diff: '', truncated: false } as never);
    const s = new FilesStore('sessao');
    const antiga = s.abrir('a.txt');       // 404 -> recarregar pendurada
    await vi.waitFor(() => expect(listFiles).toHaveBeenCalled());
    // abertura NOVA no meio da recarga antiga
    vi.mocked(readFile).mockResolvedValueOnce({ path: 'new.ts', text: 'N', size: 1, truncated: false });
    const nova = s.abrir('new.ts');
    await nova;
    expect(s.selecionado).toBe('new.ts');
    expect(s.conteudo?.path).toBe('new.ts');
    liberaLista({ truncated: false, entries: [] });
    await antiga;
    expect(s.erro).toBeNull();            // o 404 do a.txt NAO sobrescreve
    expect(s.selecionado).toBe('new.ts');
  });

  // Parecer Task 11, B3: arquivo apagado nao volta ao expandir pasta que estava FECHADA.
  // O recarregar so cobre raiz e abertas; a pasta colapsada guarda cache velho em porPasta —
  // o 404 precisa re-listar a pasta PAI mesmo colapsada, senao a linha fantasma reaparece.
  it('404 re-lista a pasta pai MESMO colapsada (linha apagada nao volta)', async () => {
    vi.mocked(listFiles).mockImplementation(async (_s, path) => {
      if (path === undefined) return { truncated: false, entries: [ent('src', true)] };
      if (path === 'src') return { truncated: false, entries: [ent('src/a.txt', false)] };
      return { truncated: false, entries: [] };
    });
    const s = new FilesStore('sessao');
    await s.recarregar();
    await s.alternarPasta('src');       // expande (lista src)
    expect(s.entries.some((e) => e.path === 'src/a.txt')).toBe(true);
    await s.alternarPasta('src');       // colapsa (cache de src fica guardado)
    // arquivo apagado: abrir devolve 404
    vi.mocked(readFile).mockRejectedValue(
      Object.assign(new Error('gone'), { status: 404 }));
    vi.mocked(listFiles).mockImplementation(async (_s, path) => {
      if (path === undefined) return { truncated: false, entries: [ent('src', true)] };
      if (path === 'src') return { truncated: false, entries: [] };  // sem a.txt agora
      return { truncated: false, entries: [] };
    });
    await s.abrir('src/a.txt');
    // a pasta pai colapsada foi re-listada: expandir de novo NAO ressuscita a linha
    await s.alternarPasta('src');
    expect(s.entries.some((e) => e.path === 'src/a.txt')).toBe(false);
  });

  // Parecer Task 11, B4: hit de busca morto sai dos resultados no 404 — senao o botao
  // continua clicavel para sempre sob a busca ativa.
  it('404 remove o hit morto dos resultados da busca vigente', async () => {
    vi.mocked(searchFiles).mockResolvedValue({
      hits: [{ path: 'a.txt', line: 1, text: 'x' }], truncated: false, mode: 'names',
    } as never);
    vi.mocked(readFile).mockRejectedValue(
      Object.assign(new Error('gone'), { status: 404 }));
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: false });
    vi.mocked(pathDiff).mockResolvedValue({ path: 'a.txt', diff: '', truncated: false } as never);
    const s = new FilesStore('sessao');
    await s.buscar('a', 'names');
    expect(s.resultados.length).toBe(1);
    await s.abrir('a.txt');   // 404: o hit do arquivo apagado sai
    expect(s.resultados.length).toBe(0);
    expect(s.erro).toBe(m.erro_arq_inexistente());
  });

  // Task 10, contrato 2: `loading` fica ligado entre o clique e a resposta — sem o sinal, o
  // FileViewer nao tem como saber que nao pode afirmar "sem diferencas".
  it('loading fica ligado durante a abertura e desliga no fim', async () => {
    let libera: (v: unknown) => void = () => {};
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((r) => (libera = r)) as never);
    vi.mocked(pathDiff).mockResolvedValue({ path: 'a.txt', diff: '', truncated: false } as never);
    const s = new FilesStore('sessao');
    const p = s.abrir('a.txt');
    expect(s.loading).toBe(true);
    libera({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    await p;
    expect(s.loading).toBe(false);
  });

  // Parecer Task 11, B2: o registry chaveia por IDENTIDADE (serverId::sessionName), nao por
  // nome — dois servidores podem ter sessoes homonimas e nao podem compartilhar estado.
  it('sessoes homonimas de servidores diferentes tem stores distintos', async () => {
    const a = filesStores.retain('srv-a::api', 'api');
    const b = filesStores.retain('srv-b::api', 'api');
    expect(a).not.toBe(b);   // mesma sessao 'api', servidores diferentes -> stores diferentes
    a.selecionado = 'x.ts';
    expect(b.selecionado).toBeNull();   // o que A abriu nao vaza pra B
    // voltar ao A restaura o estado do A (mesma instancia, nao uma copia)
    const a2 = filesStores.retain('srv-a::api', 'api');
    expect(a2).toBe(a);
    expect(a2.selecionado).toBe('x.ts');
    filesStores.release('srv-a::api');
    filesStores.release('srv-b::api');
  });

  // Parecer Task 11, B2: resposta atrasada do servidor A nao pinta no store do B.
  it('resposta atrasada de uma sessao nao vaza pra homonima de outro servidor', async () => {
    let libera: (v: unknown) => void = () => {};
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((r) => (libera = r)) as never);
    vi.mocked(pathDiff).mockResolvedValue({ path: 'a.ts', diff: '', truncated: false } as never);
    const a = filesStores.retain('srv-a::api', 'api');
    const b = filesStores.retain('srv-b::api', 'api');
    const pendente = a.abrir('a.ts');          // A abre, resposta pendurada
    vi.mocked(readFile).mockResolvedValue({ path: 'b.ts', text: 'B', size: 1, truncated: false });
    await b.abrir('b.ts');                      // B abre e completa no meio
    libera({ path: 'a.ts', text: 'A', size: 1, truncated: false });
    await pendente;                             // resposta do A chega atrasada
    expect(b.conteudo?.path).toBe('b.ts');      // o B nao foi tocado pelo A
    expect(a.conteudo?.path).toBe('a.ts');
    filesStores.release('srv-a::api');
    filesStores.release('srv-b::api');
  });
});
