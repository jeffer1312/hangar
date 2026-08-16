<script lang="ts">
  import * as m from '../../paraglide/messages';
  import type { PathDiff, FileContent } from '../../lib/types';
  import { highlightDiff, type DiffRow } from '../../lib/highlight';
  import { dec } from '../../lib/fmt';
  import DiffView from '../git/DiffView.svelte';

  interface Props {
    path: string;
    diff: PathDiff | null;
    conteudo: FileContent | null;
    loading: boolean;
    onEscopo: (e: 'branch' | 'nao_commitado') => void;
    onFechar: () => void;
  }
  let { path, diff, conteudo, loading, onEscopo, onFechar }: Props = $props();

  // Linhas do diff já destacadas. highlightDiff é assíncrona (import dinâmico do Shiki).
  // A flag `valida` do $effect descarta resposta velha — escopo trocado, diff novo ou o
  // componente desmontado no meio da busca; sem ela a resposta anterior sobrescreve a nova.
  let rows: DiffRow[] = $state([]);

  // Qual TEXTO de diff já está em `rows`. Derivar "está destacando" disto (em vez de um $state
  // ligado dentro do $effect) tira o quadro inicial em que rows=[] e destacando=false — nele o
  // DiffView desenhava "sem diferenças" para um arquivo que tem diff.
  let destacadoDe: string | null = $state(null);

  // Payload de OUTRO arquivo nao desenha: entre `abrir(b)` e a resposta, o store ainda tem o
  // conteudo de `a` (ele so troca quando a resposta chega), e a tela mostrava o arquivo errado
  // sob o nome certo. `path` no FileContent/PathDiff existe exatamente pra isso.
  const doArquivo = $derived(conteudo !== null && conteudo.path === path ? conteudo : null);
  const diffDoArquivo = $derived(diff !== null && diff.path === path ? diff : null);

  // Diff VAZIO nao e diff: o path_diff responde "" para arquivo sem alteracao no escopo, e nesse
  // caso quem o usuario quer ver e o arquivo.
  const temDiff = $derived(diffDoArquivo !== null && diffDoArquivo.diff.trim() !== '');

  const destacando = $derived(temDiff && diffDoArquivo !== null && destacadoDe !== diffDoArquivo.diff);

  // O que está NA TELA é o escopo_usado. Quando ele diverge do pedido, o outro escopo é
  // impossível (a base não existe) — botão desabilitado, mas com o rótulo do que está sendo
  // mostrado e o motivo em TEXTO (title de botão desabilitado não é lido por ninguém).
  const caiu = $derived(diffDoArquivo !== null && diffDoArquivo.escopo_usado !== diffDoArquivo.escopo_pedido);

  $effect(() => {
    const d = diffDoArquivo;
    let valida = true;
    if (d === null || d.diff.trim() === '') {
      rows = [];
      destacadoDe = null;
      return () => { valida = false; };
    }
    rows = [];
    highlightDiff(d.diff, path).then((r) => {
      if (!valida) return;
      rows = r;
      destacadoDe = d.diff;
    }).catch(() => {
      if (!valida) return;
      rows = [];
      destacadoDe = d.diff;   // desiste desta versão: não fica preso em "carregando"
    });
    return () => { valida = false; };
  });

  // +N −M contado das linhas destacadas (a mesma conta do cabeçalho interno do DiffView).
  const estat = $derived({
    add: rows.filter((r) => r.kind === 'add').length,
    del: rows.filter((r) => r.kind === 'del').length,
  });

  // Caminho quebrado em pasta + nome: a pasta sai em --text-muted, o nome no tom normal.
  // $derived de propósito: trocar de arquivo sem remontar o componente tem que trocar o cabeçalho.
  const ultimaBarra = $derived(path.lastIndexOf('/'));
  const nomeArquivo = $derived(ultimaBarra === -1 ? path : path.slice(ultimaBarra + 1));
  const dirParte = $derived(ultimaBarra === -1 ? '' : path.slice(0, ultimaBarra + 1));

  // "desde 721d1a0" chega inteiro da tradução; a barra mostra o "desde" com peso 500.
  // Split na primeira palavra cobre pt ("desde") e en ("since").
  const partesDesde = $derived(
    diffDoArquivo !== null && diffDoArquivo.base !== null && diffDoArquivo.base.trim() !== ''
      ? m.arq_escopo_desde({ base: diffDoArquivo.base }).split(/ (.+)/)
      : null,
  );

  // Tamanho binário na vírgula do idioma do app (dec usa intlLocale): "12,4 KB" casa com a
  // barra; abaixo de 1 KB mostra os bytes crus.
  function tamLegivel(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${dec(n / 1024, 1)} KB`;
    return `${dec(n / (1024 * 1024), 1)} MB`;
  }

  function linhasDoTexto(texto: string): number {
    if (!texto) return 0;
    return texto.split('\n').length - (texto.endsWith('\n') ? 1 : 0);
  }
</script>

<div class="visor">
  <div class="cab">
    <div class="cab-l1">
      <span class="caminho">
        {#if dirParte}<span class="dir">{dirParte}</span>{/if}{nomeArquivo}
      </span>
      {#if diffDoArquivo && !loading && !destacando && (estat.add || estat.del)}
        <span class="stat"><span class="stat-add">+{estat.add}</span> <span class="stat-del">−{estat.del}</span></span>
      {/if}
      <button class="fechar" aria-label={m.arq_fechar()} onclick={onFechar}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
      </button>
    </div>
    <div class="cab-l2">
      {#if diffDoArquivo}
        {@const d = diffDoArquivo}
        <button
          class="escopo"
          disabled={caiu}
          onclick={() => onEscopo(d.escopo_usado === 'branch' ? 'nao_commitado' : 'branch')}
        >
          {d.escopo_usado === 'branch' ? m.arq_escopo_branch() : m.arq_escopo_nao_commitado()}
          <span class="seta">▾</span>
        </button>
        {#if caiu && d.motivo}<span class="motivo">{d.motivo}</span>{/if}
      {/if}
      <span class="meta">
        {#if partesDesde}
          <b>{partesDesde[0]}</b> {partesDesde[1]}
        {/if}
        {#if partesDesde && doArquivo}<span class="sep"> · </span>{/if}
        {#if doArquivo}
          {m.arq_meta_arquivo({ tam: tamLegivel(doArquivo.size), linhas: linhasDoTexto(doArquivo.text) })}
        {/if}
      </span>
      <button class="voltar" onclick={onFechar}>← {m.arq_voltar_conversa()}</button>
    </div>
  </div>

  <div class="corpo">
    {#if temDiff && diffDoArquivo}
      {@const d = diffDoArquivo}
      {#if d.truncated}
        <p class="aviso">{m.arq_diff_cortado()}</p>
      {/if}
      <DiffView path={path} rows={rows} loading={loading || destacando} />
    {:else if doArquivo}
      {#if doArquivo.truncated}
        <p class="aviso">{m.arq_arquivo_cortado()}</p>
      {/if}
      <pre class="conteudo">{doArquivo.text}</pre>
    {:else if loading}
      <!-- Busca em voo sem nada ainda: aviso de carga, nunca a afirmação "sem diferenças". -->
      <p class="aviso">{m.git_diff_carregando()}</p>
    {:else}
      <p class="aviso">{m.git_sem_diferencas()}</p>
    {/if}
  </div>
</div>

<style>
  .visor { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow: hidden; }
  .cab { padding: 11px 16px 10px; border-bottom: 1px solid var(--border-subtle); }
  .cab-l1 { display: flex; align-items: center; gap: 10px; }
  .caminho {
    font-family: var(--font-mono); font-size: 13px;
    flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .caminho .dir { color: var(--text-muted); }
  .stat { font-family: var(--font-mono); font-size: 12px; margin-left: auto; flex: none; }
  .stat .stat-add { color: var(--success); }
  .stat .stat-del { color: var(--error); }
  .fechar {
    border: 0; background: none; color: var(--text-muted); cursor: pointer;
    width: 26px; height: 26px; border-radius: 6px; display: grid; place-items: center;
    flex: none;
  }
  .fechar:hover { background: var(--bg-hover); color: var(--text-primary); }
  .fechar svg { width: 15px; height: 15px; }

  .cab-l2 { display: flex; align-items: center; gap: 10px; margin-top: 9px; }
  .escopo {
    display: inline-flex; align-items: center; gap: 5px;
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    border-radius: 8px; padding: 5px 9px; font-size: 12px; color: var(--text-primary);
    cursor: pointer; font-family: inherit; flex: none;
  }
  .escopo:disabled { cursor: default; opacity: 0.55; }
  .escopo .seta { font-size: 8px; }
  .meta { font-size: 12px; color: var(--text-muted); min-width: 0; }
  .meta b { font-weight: 500; color: var(--text-secondary); }
  /* Motivo do escopo caído: TEXTO ao lado do botão (title em botão desabilitado não é lido).
     --warning é o token que o DiffView já usa para o aviso de corte — mesma família visual. */
  .motivo { font-size: 12px; color: var(--warning); min-width: 0; }
  .voltar {
    margin-left: auto; display: inline-flex; align-items: center; gap: 5px;
    background: none; border: 0; color: var(--accent); font: inherit; font-size: 12px;
    cursor: pointer; flex: none; padding: 0;
  }

  .corpo { padding: 14px 16px 18px; overflow-y: auto; flex: 1; min-height: 0; }

  /* O cabeçalho de cima (caminho, +N −M, fechar) é DESTE componente. O interno do DiffView
     (que serve o GitChangesTab intacto) fica escondido aqui, por CSS no escopo do FileViewer —
     sem tocar no DiffView, que é arquivo compartilhado deste lote. */
  .corpo :global(.git-diff-head) {
    display: none;
  }

  .aviso {
    margin: 0 0 var(--space-2); padding: 7px 9px; border-radius: 7px;
    background: var(--fill-subtle); color: var(--text-muted);
    font-size: 11.5px; line-height: 1.4;
  }

  .conteudo {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
    max-height: 62vh; overflow: auto; white-space: pre;
  }
</style>
