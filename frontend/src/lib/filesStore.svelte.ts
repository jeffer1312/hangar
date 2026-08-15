// Estado + acoes da aba Arquivos (FileTree/FileViewer/FileSearchBar): lista, le, busca e diff de
// arquivos do repo da sessao. .svelte.ts permite runes fora de componente. Uma instancia serve
// UMA sessao (nome fixo no construtor) — quem mantem a instancia viva (o painel de contexto da
// sessao) e quem preserva as pastas abertas entre trocas de tela.
import { listFiles, readFile, searchFiles, pathDiff } from './api';
import { formataErro } from './errosApi';
import type { FileContent, PathDiff, SearchHit, TreeEntry } from './types';

export class FilesStore {
  /** Pastas expandidas na arvore (caminho absoluto dentro do repo). */
  abertos = $state<Set<string>>(new Set());
  /** Arquivo selecionado na arvore (caminho absoluto). */
  selecionado = $state<string | null>(null);
  /** Conteudo do diretorio sendo exibido ('' = raiz). */
  entries = $state<TreeEntry[]>([]);
  /** Conteudo do arquivo selecionado (nulo antes da primeira abertura). */
  conteudo = $state<FileContent | null>(null);
  /** Diff do arquivo selecionado no escopo atual. */
  diff = $state<PathDiff | null>(null);
  /** Escopo do diff: desde a base da branch (soma dos turnos) ou so o nao-commitado. */
  escopo = $state<'branch' | 'nao_commitado'>('branch');
  /** Achados da ultima busca; no modo `names`, line e text vem null. */
  resultados = $state<SearchHit[]>([]);
  /** Erro legivel da ultima operacao (nulo quando a operacao foi limpa). */
  erro = $state<string | null>(null);
  /** Listar so arquivos modificados (a arvore inteira quando false). */
  soModificados = $state(true);

  // Contador de pedido: cada chamada guarda o numero dela e ao voltar so pinta se ainda for a
  // mais recente. Sem isto, uma resposta atrasada de um alvo abandonado pintaria por cima.
  private geracao = 0;
  /** Diretorio listado em `entries` ('' = raiz) — o que `recarregar()` re-lista. */
  private dirAtual = '';
  private readonly sessao: string;

  constructor(sessao: string) {
    this.sessao = sessao;
  }

  /** Abre um arquivo: pinta conteudo + diff (no escopo atual) quando a resposta voltar. */
  async abrir(path: string) {
    this.selecionado = path;
    this.erro = null;
    const g = ++this.geracao;
    try {
      const [c, d] = await Promise.all([
        readFile(this.sessao, path),
        pathDiff(this.sessao, path, this.escopo),
      ]);
      if (g !== this.geracao) return; // um pedido mais novo ja tomou o lugar
      this.conteudo = c;
      this.diff = d;
    } catch (e) {
      if (g === this.geracao) this.erro = formataErro(e) ?? String(e);
    }
  }

  /** Expande/colapsa uma pasta; ao expandir, lista o conteudo dela no `entries`. */
  async alternarPasta(path: string) {
    if (this.abertos.has(path)) {
      this.abertos.delete(path);
      this.dirAtual = '';
      await this._listar(''); // colapsou: a arvore volta a mostrar a raiz
      return;
    }
    this.abertos.add(path);
    this.dirAtual = path;
    await this._listar(path);
  }

  /** Busca por nome ou conteudo; os achados vao para `resultados`. */
  async buscar(q: string, mode: 'names' | 'contents') {
    this.erro = null;
    const g = ++this.geracao;
    try {
      const r = await searchFiles(this.sessao, q, mode);
      if (g !== this.geracao) return;
      this.resultados = r.hits;
    } catch (e) {
      if (g === this.geracao) this.erro = formataErro(e) ?? String(e);
    }
  }

  /** Re-lista o diretorio atual com o filtro `soModificados` de agora. */
  async recarregar() {
    await this._listar(this.dirAtual);
  }

  /** Troca o escopo do diff (select de value 'branch' | 'nao_commitado') e reabre o selecionado. */
  async trocarEscopo(e: Event) {
    const v = (e.target as HTMLSelectElement).value;
    if (v !== 'branch' && v !== 'nao_commitado') return;
    this.escopo = v;
    if (this.selecionado) await this.abrir(this.selecionado);
  }

  private async _listar(path: string) {
    const g = ++this.geracao;
    try {
      const r = await listFiles(this.sessao, path || undefined, this.soModificados);
      if (g !== this.geracao) return;
      this.entries = r.entries;
    } catch (e) {
      if (g === this.geracao) this.erro = formataErro(e) ?? String(e);
    }
  }
}
