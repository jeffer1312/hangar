// Estado + acoes da aba Arquivos (FileTree/FileViewer/FileSearchBar): lista, le, busca e diff de
// arquivos do repo da sessao. .svelte.ts permite runes fora de componente. Uma instancia serve
// UMA sessao (nome fixo no construtor) — quem mantem a instancia viva e o registry abaixo, que
// vive no MODULO: o App remonta o Chat por {#key} a cada troca de sessao (App.svelte:462), e um
// store criado no mount do componente morreria junto com as pastas abertas. O precedente e o
// sessionsStore (singleton com retain/release); o Git.svelte cria o store no componente e o
// $effect o RECRIA quando a sessao muda — o padrao contrario, que faz a regua "pasta aberta
// continua aberta ao voltar" falhar sem erro nenhum.
import * as m from '../paraglide/messages';
import { listFiles, readFile, searchFiles, pathDiff, writeFile } from './api';
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

  // Geracao do DONO do campo `erro` (parecer Task 11, B1): `abrir`, `buscar` e `recarregar`
  // capturam uma geracao nova ao comecar e so escrevem/limpam `erro` se ainda forem o dono
  // quando a resposta voltar. Sem isto, a recuperacao do 404 de um arquivo antigo (que roda
  // `await recarregar()` no meio) sobrescrevia o erro de uma abertura mais nova com o aviso
  // do arquivo que ja foi abandonado.
  private gErro = 0;
  // Geracao da busca que gravou `resultados` — o 404 de abrir so remove o hit morto da lista
  // se os resultados ainda forem dessa busca (B4); busca nova em voo tem prioridade.
  private gResultados = -1;

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

  // Grava o arquivo aberto. Devolve a MENSAGEM do erro em vez de levantar: o visor a mostra e
  // mantem o texto digitado na tela — perder a edicao por causa de um conflito seria trocar um
  // problema por outro pior. Mensagem, nao codigo: o `errorDetail` do api.ts ja traduziu o
  // `{code, params}` do backend, e o codigo nao sobrevive ao Error que ele levanta.
  async salvar(path: string, texto: string): Promise<string | null> {
    const atual = this.conteudo;
    if (!atual || atual.path !== path) return 'erro_arq_inexistente';
    try {
      const r = await writeFile(this.sessao, path, texto, atual.digest);
      // So atualiza se ainda for o mesmo arquivo na tela (o usuario pode ter trocado no meio).
      if (this.conteudo?.path === path) {
        this.conteudo = { ...this.conteudo, text: texto, size: r.size, digest: r.digest };
      }
      // O diff da tela envelheceu no instante da gravacao: reler e o que impede o visor de
      // afirmar um diff que nao existe mais.
      void this.recarregarDiff(path);
      return null;
    } catch (e) {
      return (e as Error)?.message || 'erro_arq_salvar_falhou';
    }
  }

  async recarregarDiff(path: string) {
    try {
      const d = await pathDiff(this.sessao, path, this.escopo);
      if (this.selecionado === path) this.diff = d;
    } catch {
      // Diff e enfeite aqui: a gravacao ja aconteceu, e falhar em reler nao pode virar erro na
      // cara de quem acabou de salvar com sucesso.
    }
  }

  // Abre um arquivo: pinta conteudo + diff (no escopo atual) quando a resposta voltar.
  async abrir(path: string) {
    this.selecionado = path;
    this.erro = null;
    this.loading = true;
    const g = ++this.gArquivo;
    const ge = ++this.gErro;   // esta abertura e a dona do erro a partir de agora
    const gr = this.gResultados;   // geracao dos resultados que ESTA abertura pode podar (B4)
    const gb = this.gBusca;        // ... e a geracao da busca vigente no momento do clique
    // A poda so vale se a busca vigente JA pintou os resultados que esta abertura viu: com uma
    // busca nova em voo (gBusca avancou, gResultados nao), o painel ainda mostra a lista
    // antiga e a abertura nao pode poda-la — a busca em voo pode falhar e o hit era a unica
    // resposta (B4, rodada 5 — o clique no hit antigo depois da busca nova comecar).
    const podePodar = gb === gr;
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
      // Linha fantasma (Task 11): arquivo apagado entre listar e abrir. Alem do erro certo,
      // RE-LISTA — sem a recarga, a linha continua clicavel para sempre apontando pra nada
      // (o 404 do readFile e o sinal; o status vem na propriedade, como no _listar). O erro
      // so e pintado depois da recarga: a raiz limpa this.erro no sucesso, e o recarregar
      // pode falhar com algo mais grave (sessao encerrada) que nao pode ser sobrescrito.
      const status = (c.reason as Error & { status?: number })?.status;
      if (status === 404) {
        // A pasta PAI e re-listada MESMO colapsada (B3): o recarregar so cobre raiz e abertas,
        // e sem esta passada o cache velho da pasta fechada devolveria a linha apagada na
        // proxima expansao. Invalida a geracao ANTES (resposta velha da pasta nao repovoa).
        const pai = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
        // Invalida o cache do pai e descendentes ANTES de qualquer await (B3, rodada 3): se o
        // usuario expandir a pasta enquanto a recuperacao esta em voo, o alternarPasta nao
        // pode achar a listagem velha (com o arquivo apagado) e pular a rede.
        this._invalidarSubarvore(pai);
        await this.recarregar(ge);
        // Forca a listagem do pai SEM condicionar em abertos.has(pai): o recarregar so cobre
        // raiz e pastas abertas, e a expansao durante a recuperacao pode ja ter listado com
        // geracao mais nova — o _listar abaixo deixa gLista descartar a resposta mais velha.
        if (pai !== '') await this._listar(pai, ge);
        // Hit de busca morto (B4): o arquivo apagado sai dos resultados, senao o botao
        // continua clicavel para sempre. So se a busca vigente ja tinha pintado os resultados
        // que esta abertura viu (podePodar) E nada mudou desde o clique (gb/gr iguais aos
        // capturados): busca nova em voo ou concluida nunca e alterada pela resposta velha.
        if (podePodar && gb === this.gBusca && gr === this.gResultados) {
          this.resultados = this.resultados.filter((h) => h.path !== path);
        }
        // B1: so pinta o erro se ESTA abertura ainda for a dona — uma abertura/busca nova
        // venceu durante a recarga e nao pode ser sobrescrita pelo aviso do arquivo antigo.
        if (ge === this.gErro && !this.erro) this.erro = m.erro_arq_inexistente();
      } else if (ge === this.gErro) {
        this.erro = cleanErr(c.reason);
      }
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
    if (!this.porPasta.has(path)) await this._listar(path, ++this.gErro);
  }

  // Busca por nome ou conteudo; os achados vao para `resultados`.
  async buscar(q: string, mode: 'names' | 'contents') {
    this.erro = null;
    const g = ++this.gBusca;
    const ge = ++this.gErro;   // esta busca e a dona do erro
    try {
      const r = await searchFiles(this.sessao, q, mode);
      if (g !== this.gBusca) return;
      this.resultados = r.hits;
      this.gResultados = g;
      this.buscaCortada = r.truncated;
    } catch (e) {
      if (g === this.gBusca && ge === this.gErro) this.erro = cleanErr(e);
    }
  }

  // Re-lista a raiz e todas as pastas abertas, com o filtro `soModificados` de agora.
  // `ge` e a geracao do dono do erro quando chamada POR uma operacao (a recuperacao do 404
  // passa a da abertura); chamada pelo botao, captura geracao nova (e o dono).
  async recarregar(ge?: number) {
    const dona = ge ?? ++this.gErro;
    await Promise.all(['', ...this.abertos].map((p) => this._listar(p, dona)));
  }

  // Troca o escopo do diff e reabre o arquivo selecionado. Quem le o controle da tela e o
  // componente — o store nao depende de DOM (o seletor da barra e um botao, nao um <select>).
  async trocarEscopo(escopo: 'branch' | 'nao_commitado') {
    if (escopo === this.escopo) return;
    this.escopo = escopo;
    if (this.selecionado) await this.abrir(this.selecionado);
  }

  // Invalida o cache de uma subarvore (a pasta e todos os descendentes): incrementa a
  // geracao de cada pasta — resposta em voo nao pode repovoa-la — e remove o conteudo de
  // porPasta/cortePorPasta. NAO mexe em `abertos`: a pasta continua aberta, so o conteudo e
  // re-listado. E o caminho do 404 de arquivo (B3); a poda do _listar, que remove `abertos`
  // porque a PASTA sumiu do disco, e outra (e por isso nao reusa esta).
  private _invalidarSubarvore(path: string) {
    for (const p of [...this.porPasta.keys(), ...this.cortePorPasta.keys()]) {
      if (p === path || (path !== '' && p.startsWith(path + '/'))) {
        this.gLista.set(p, (this.gLista.get(p) ?? 0) + 1);
        this.cortePorPasta.delete(p);
        this.porPasta.delete(p);
      }
    }
  }

  private async _listar(path: string, ge: number) {
    const g = (this.gLista.get(path) ?? 0) + 1;
    this.gLista.set(path, g);
    // A raiz e sempre listada no recarregar: sucesso limpa o erro — mas so se quem pediu a
    // listagem ainda e o dono (B1): uma abertura/busca nova nao pode ter seu erro apagado
    // pela resposta velha de uma listagem abandonada.
    if (path === '' && ge === this.gErro) this.erro = null;
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
          if (ge === this.gErro) this.erro = m.arq_sessao_encerrada();
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
      if (ge === this.gErro) this.erro = cleanErr(e);
    }
  }
}

// Registry do FilesStore: uma instancia por IDENTIDADE composta (serverId::sessionName), vivendo
// no MODULO. O App remonta o Chat por {#key} a cada troca de sessao, entao um store criado no
// onMount morreria — e a regua "pasta aberta continua aberta ao voltar" exige que o estado
// sobreviva ao remount. A chave leva o serverId porque dois servidores podem ter sessoes com o
// MESMO nome (parecer Task 11, B2): sem o servidor na chave, abrir um arquivo no servidor A
// deixava selecionado/conteudo/cache no store que o servidor B recebia ao abrir a homonima.
const stores = new Map<string, FilesStore>();
const refs = new Map<string, number>();

export const filesStores = {
  // `chave` e a identidade composta (serverId::sessionName); `sessao` e o NOME, que e o que as
  // chamadas de API usam (filesStore.sessao). A identidade vem do mesmo lugar que remonta o
  // Chat (DesktopShell.workspaceSessionKey), nunca calculada diferente por caller.
  retain(chave: string, sessao: string): FilesStore {
    let s = stores.get(chave);
    if (!s) {
      s = new FilesStore(sessao);
      stores.set(chave, s);
    }
    refs.set(chave, (refs.get(chave) ?? 0) + 1);
    return s;
  },
  release(chave: string): void {
    const n = (refs.get(chave) ?? 1) - 1;
    if (n <= 0) refs.delete(chave);
    else refs.set(chave, n);
    // ponytail: com refs 0 o store continua no map — o estado (pastas abertas) e a regua de
    // "pasta aberta continua aberta ao voltar", e a memoria por identidade e barata (o app
    // reusa nomes). Se um dia virar vazamento real, um LRU por antiguedade.
  },
};
