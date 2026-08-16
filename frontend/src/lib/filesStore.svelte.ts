// Estado + acoes da aba Arquivos (FileTree/FileViewer/FileSearchBar): lista, le, busca e diff de
// arquivos do repo da sessao. .svelte.ts permite runes fora de componente. Uma instancia serve
// UMA sessao (nome fixo no construtor) — quem mantem a instancia viva e o registry abaixo, que
// vive no MODULO: o App remonta o Chat por {#key} a cada troca de sessao (App.svelte:462), e um
// store criado no mount do componente morreria junto com as pastas abertas. O precedente e o
// sessionsStore (singleton com retain/release); o Git.svelte cria o store no componente e o
// $effect o RECRIA quando a sessao muda — o padrao contrario, que faz a regua "pasta aberta
// continua aberta ao voltar" falhar sem erro nenhum.
import * as m from '../paraglide/messages';
import { listFiles, readFile, searchFiles, pathDiff } from './api';
import { cleanErr } from './gitStore.svelte';
import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import type { FileContent, PathDiff, SearchHit, TreeEntry } from './types';

export class FilesStore {
  // Pastas expandidas na arvore (caminho absoluto dentro do repo). SvelteSet, nao Set: `$state`
  // so faz proxy de objeto simples e array (svelte/internal/client/proxy.js), entao `.add`/
  // `.delete` num Set cru nao repintariam a arvore.
  abertos = new SvelteSet<string>();
  // Arquivo selecionado na arvore (caminho absoluto).
  selecionado = $state<string | null>(null);
  // Conteudo do arquivo selecionado (nulo antes da primeira abertura).
  conteudo = $state<FileContent | null>(null);
  // Diff do arquivo selecionado no escopo atual (nulo quando o diff falha — fora de repo git).
  diff = $state<PathDiff | null>(null);
  // Escopo do diff: desde a base da branch (soma dos turnos) ou so o nao-commitado.
  escopo = $state<'branch' | 'nao_commitado'>('branch');
  // Achados da ultima busca; no modo `names`, line e text vem null.
  resultados = $state<SearchHit[]>([]);
  // Erro legivel da ultima operacao (nulo quando a operacao foi limpa).
  erro = $state<string | null>(null);
  // Listar so arquivos modificados (a arvore inteira quando false).
  soModificados = $state(true);
  // O backend cortou os achados em 200 (filesearch.MAX_HITS). Uma resposta so, sem paralelismo.
  buscaCortada = $state(false);

  // Abrir em voo (verdadeiro entre o clique e a resposta). O FileViewer precisa saber que NAO
  // pode afirmar "sem diferencas" enquanto o diff nao chegou — sem o sinal, a janela cai no
  // {:else} e a tela mente sobre arquivo que tem diff.
  loading = $state(false);

  // Um contador POR ALVO: `abrir`, `buscar` e `_listar` pintam campos diferentes e nao podem
  // cancelar uns aos outros. Contador unico descartava o arquivo que estava abrindo assim que
  // qualquer outra operacao comecasse. O de lista e POR PASTA: `recarregar()` re-lista varias
  // pastas em paralelo e uma nao pode cancelar a outra.
  private gArquivo = 0;
  private gBusca = 0;
  private gLista = new Map<string, number>();

  // Conteudo de cada pasta ja listada ('' = raiz). A arvore mostra varias pastas abertas ao
  // mesmo tempo (docs/mocks/2026-08-15-arvore/arvore.js), entao um diretorio de cada vez nao
  // serve.
  private porPasta = new SvelteMap<string, TreeEntry[]>();

  // O corte e POR PASTA: `recarregar()` lista a raiz e as abertas em paralelo, e um campo unico
  // ficava com o valor de quem respondeu por ultimo — a raiz cortada sumia assim que uma
  // subpasta inteira respondesse depois dela.
  private cortePorPasta = new SvelteMap<string, boolean>();

  private readonly sessao: string;

  constructor(sessao: string) {
    this.sessao = sessao;
  }

  // A arvore achatada: a raiz e, logo depois de cada pasta aberta, os filhos dela.
  get entries(): TreeEntry[] {
    const saida: TreeEntry[] = [];
    const empilha = (dir: string) => {
      for (const e of this.porPasta.get(dir) ?? []) {
        saida.push(e);
        if (e.is_dir && this.abertos.has(e.path)) empilha(e.path);
      }
    };
    empilha('');
    return saida;
  }

  // Cortou em alguma pasta que o usuario esta vendo? Pasta colapsada nao conta: os filhos dela
  // nao estao na arvore.
  get listaCortada(): boolean {
    for (const [dir, cortou] of this.cortePorPasta) {
      if (cortou && (dir === '' || this.abertos.has(dir))) return true;
    }
    return false;
  }

  // Abre um arquivo: pinta conteudo + diff (no escopo atual) quando a resposta voltar.
  async abrir(path: string) {
    this.selecionado = path;
    this.erro = null;
    this.loading = true;
    const g = ++this.gArquivo;
    // allSettled, nao all: o conteudo MANDA. Fora de repositorio git o path_diff sempre responde
    // 409 (git_ops.py) e a arvore tem que continuar lendo arquivo (regra do usuario, 15/08).
    const [c, d] = await Promise.allSettled([
      readFile(this.sessao, path),
      pathDiff(this.sessao, path, this.escopo),
    ]);
    if (g !== this.gArquivo) return; // uma abertura mais nova ja tomou o lugar
    this.loading = false;
    if (c.status === 'rejected') {
      // Falha ao abrir nao pode deixar o conteudo do arquivo anterior na tela sob o nome novo.
      this.conteudo = null;
      this.diff = null;
      // Nem o nome do arquivo que falhou: com selecionado marcado e conteudo/diff nulos, o
      // FileViewer cairia no "sem diferencas" — mentira sobre um arquivo que nem abriu (medido
      // ao vivo com um binario: o erro sumia no recarregar e a tela afirmava que o png nao tem
      // diff). Sem selecao, o painel mostra o aviso de erro e a arvore continua navegavel.
      this.selecionado = null;
      this.erro = cleanErr(c.reason);
      return;
    }
    this.conteudo = c.value;
    // Diff que falha NAO derruba a leitura: fora de repositorio git o path_diff responde 409 e
    // a arvore tem que continuar lendo arquivo. `diff = null` e o estado de "sem alteracao".
    this.diff = d.status === 'fulfilled' ? d.value : null;
  }

  // Expande/colapsa uma pasta; ao expandir, lista o conteudo dela (uma vez so).
  async alternarPasta(path: string) {
    if (this.abertos.has(path)) {
      this.abertos.delete(path); // colapsar nao re-lista nem volta pra raiz
      return;
    }
    this.abertos.add(path);
    if (!this.porPasta.has(path)) await this._listar(path);
  }

  // Busca por nome ou conteudo; os achados vao para `resultados`.
  async buscar(q: string, mode: 'names' | 'contents') {
    this.erro = null;
    const g = ++this.gBusca;
    try {
      const r = await searchFiles(this.sessao, q, mode);
      if (g !== this.gBusca) return;
      this.resultados = r.hits;
      this.buscaCortada = r.truncated;
    } catch (e) {
      if (g === this.gBusca) this.erro = cleanErr(e);
    }
  }

  // Re-lista a raiz e todas as pastas abertas, com o filtro `soModificados` de agora.
  async recarregar() {
    await Promise.all(['', ...this.abertos].map((p) => this._listar(p)));
  }

  // Troca o escopo do diff e reabre o arquivo selecionado. Quem le o controle da tela e o
  // componente — o store nao depende de DOM (o seletor da barra e um botao, nao um <select>).
  async trocarEscopo(escopo: 'branch' | 'nao_commitado') {
    if (escopo === this.escopo) return;
    this.escopo = escopo;
    if (this.selecionado) await this.abrir(this.selecionado);
  }

  private async _listar(path: string) {
    const g = (this.gLista.get(path) ?? 0) + 1;
    this.gLista.set(path, g);
    if (path === '') this.erro = null;   // a raiz e sempre listada no recarregar: sucesso limpa
    try {
      const r = await listFiles(this.sessao, path || undefined, this.soModificados);
      if (g !== this.gLista.get(path)) return;
      this.porPasta.set(path, r.entries);
      this.cortePorPasta.set(path, r.truncated);
    } catch (e) {
      if (g !== this.gLista.get(path)) return;
      // O status vem na propriedade, nao no texto: `ensureOk` (api.ts:111) deixa a MENSAGEM
      // limpa e anexa `.status` — ler o texto quebra calado (api.ts:533 avisa com todas as
      // letras; medido pelo revisor: o 404 real chega como "Nao deu pra acessar..." + status).
      const status = (e as Error & { status?: number }).status;
      if (status === 404) {
        // Pasta que sumiu do disco (filetree.py responde 404 erro_arq_inexistente): poda a
        // arvore — a pasta aberta aponta pra lugar que nao existe, e o aviso de corte ficaria
        // ligado com a arvore vazia. 404 na RAIZ nao e pasta sumida: e o cwd da sessao que
        // morreu (ou a sessao encerrou) — vira aviso visivel.
        if (path === '') {
          this.erro = m.arq_sessao_encerrada();
        } else {
          // A pasta e TODOS os descendentes — abertos OU colapsados: um filho colapsado
          // mantem o cache em porPasta/cortePorPasta, e sem apagar esse cache o arquivo
          // velho reapareceria quando a arvore voltasse a existir (medido pelo revisor).
          // Antes de apagar cada estado, incrementa o contador dele em gLista — invalida
          // resposta em voo de uma subpasta (a resposta atrasada nao pode repor estado
          // obsoleto na arvore).
          const chaves = new Set([...this.abertos, ...this.porPasta.keys(), ...this.cortePorPasta.keys()]);
          for (const p of chaves) {
            if (p === path || p.startsWith(path + '/')) {
              this.gLista.set(p, (this.gLista.get(p) ?? 0) + 1);
              this.abertos.delete(p);
              this.cortePorPasta.delete(p);
              this.porPasta.delete(p);
            }
          }
        }
        return;
      }
      this.erro = cleanErr(e);
    }
  }
}

// Registry do FilesStore: uma instancia por sessao, vivendo no MODULO. O App remonta o Chat por
// {#key} a cada troca de sessao, entao um store criado no onMount morreria — e a regua "pasta
// aberta continua aberta ao voltar" exige que o estado sobreviva ao remount.
const stores = new Map<string, FilesStore>();
const refs = new Map<string, number>();

export const filesStores = {
  retain(sessao: string): FilesStore {
    let s = stores.get(sessao);
    if (!s) {
      s = new FilesStore(sessao);
      stores.set(sessao, s);
    }
    refs.set(sessao, (refs.get(sessao) ?? 0) + 1);
    return s;
  },
  release(sessao: string): void {
    const n = (refs.get(sessao) ?? 1) - 1;
    if (n <= 0) refs.delete(sessao);
    else refs.set(sessao, n);
    // ponytail: com refs 0 o store continua no map — o estado (pastas abertas) e a regua de
    // "pasta aberta continua aberta ao voltar", e a memoria por nome de sessao e barata (o app
    // reusa nomes). Se um dia virar vazamento real, um LRU por antiguedade.
  },
};
