<script lang="ts">
  // Visão "Citados" da aba Arquivos: os arquivos que apareceram na conversa (tool_use do agente,
  // prosa dele, prompt seu), por recência. A lista já chega agrupada e ordenada pelo pai; aqui
  // só filtro local, desenho e clique.
  import * as m from '../../paraglide/messages';
  import { intlLocale } from '../../lib/locale';
  import type { Citado, Origem } from '../../lib/arquivosCitados';
  import FileIcon from './FileIcon.svelte';

  interface Props {
    citados: Citado[];
    carregando: boolean;      // ainda sem evento nenhum: skeleton
    parcial: boolean;         // histórico ainda não fechou (histGap != ''): aviso
    onAbrir: (c: Citado) => void;
    selecionado?: string | null;   // path aberto no visor (relativo, ou o cru de um externo)
  }
  let { citados, carregando, parcial, onAbrir, selecionado = null }: Props = $props();

  let filtro = $state('');
  const visiveis = $derived.by(() => {
    const f = filtro.trim().toLowerCase();
    return f ? citados.filter((c) => c.cru.toLowerCase().includes(f)) : citados;
  });

  const ORDEM: Origem[] = ['Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'Read', 'Bash', 'tool', 'voce', 'citado'];
  function rotulo(o: Origem): string {
    return o === 'voce' ? m.arq_origem_voce() : o === 'citado' ? m.arq_origem_citado() : o === 'tool' ? m.arq_origem_tool() : o;
  }
  function chips(c: Citado): { o: Origem; n: number }[] {
    return ORDEM.filter((o) => c.origens[o]).map((o) => ({ o, n: c.origens[o]! }));
  }
  function hora(ts: number): string {
    return ts ? new Date(ts * 1000).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '';
  }
</script>

<div class="busca">
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
  <input type="text" bind:value={filtro} placeholder={m.arq_filtrar_citados()} aria-label={m.arq_filtrar_citados()} />
</div>

{#if parcial}<p class="aviso">{m.arq_historico_parcial()}</p>{/if}

{#if carregando}
  <div class="skel" role="status" aria-busy="true" aria-label={m.arq_carregando()}>
    {#each [58, 34, 46, 72, 40, 62] as w, k (k)}
      <div class="skel-linha"><span class="skel-ico"></span><span class="skel-bar" style="width: {w}%"></span></div>
    {/each}
  </div>
{:else if visiveis.length === 0}
  <p class="aviso">{citados.length === 0 ? m.arq_citados_vazio() : m.arq_sem_nome()}</p>
{:else}
  <div class="lista">
    {#each visiveis as c (c.cru)}
      <button type="button" class="no" class:sel={selecionado !== null && (selecionado === c.relativo || selecionado === c.cru)}
              aria-current={selecionado !== null && (selecionado === c.relativo || selecionado === c.cru) ? 'true' : undefined}
              onclick={() => onAbrir(c)}
              title={c.relativo === null ? m.arq_abrir_fora() : c.relativo}>
        <span class="l1">
          <span class="ico" aria-hidden="true"><FileIcon nome={c.nome} /></span>
          <span class="nome">{c.nome}</span>
          <span class="hora">{hora(c.ultimoTs)}</span>
        </span>
        <span class="l2">
          <span class="pasta">{c.pasta}</span>
          <span class="chips">
            {#each chips(c) as ch (ch.o)}
              <span class="chip {ch.o === 'voce' ? 'chip-voce' : ch.o === 'citado' || ch.o === 'tool' ? '' : ch.o === 'Read' || ch.o === 'Bash' ? 'chip-leu' : 'chip-escreveu'}">{rotulo(ch.o)}{ch.n > 1 ? ` ×${ch.n}` : ''}</span>
            {/each}
          </span>
        </span>
      </button>
    {/each}
  </div>
{/if}

<style>
  /* Mesmo campo da busca da árvore (FileSearchBar), token por token. */
  .busca {
    margin: 0 10px 6px; display: flex; align-items: center; gap: 6px;
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 6px 8px;
  }
  .busca:focus-within { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .busca svg { width: 13px; height: 13px; color: var(--text-muted); flex: none; }
  .busca input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--text-primary); font: inherit; font-size: 12.5px; }
  .busca input::placeholder { color: var(--text-muted); }

  .aviso { margin: 0 10px 6px; padding: 7px 9px; border-radius: 7px; background: var(--fill-subtle); color: var(--text-muted); font-size: 11.5px; line-height: 1.4; }

  .lista { padding: 2px 0 10px; overflow-y: auto; overflow-x: hidden; flex: 1; min-height: 0; }
  .l1, .l2 { max-width: 100%; box-sizing: border-box; }
  .no {
    /* align-items explícito: o `button` global do app.css centraliza o conteúdo, e em coluna isso
       centralizava nome e pasta (medido: linhas "dançando" no dock). */
    display: flex; flex-direction: column; align-items: stretch; justify-content: flex-start;
    gap: 2px; width: 100%; padding: 5px 10px;
    border: 0; background: none; text-align: left; font-family: inherit; color: var(--text-secondary);
    cursor: pointer; min-height: 0; min-width: 0; overflow: hidden;
  }
  .no:hover { background: var(--bg-hover); }
  .no.sel { background: var(--accent-dim); }
  .no.sel .nome { color: var(--text-primary); }
  .no:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
  .l1 { display: flex; align-items: center; gap: 5px; min-width: 0; font-size: 13px; line-height: 1.35; }
  .ico { width: 16px; flex: none; display: grid; place-items: center; }
  .nome { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-primary); }
  .hora { flex: none; font-size: 10.5px; color: var(--text-muted); font-family: var(--font-mono); }
  /* Pasta e chips quebram linha quando não cabem: no dock de 460px, "Edit ×26 · Write ×16 · Bash ×35"
     junto do caminho estourava a largura e empurrava o texto pra fora do painel (medido). */
  .l2 { display: flex; align-items: center; flex-wrap: wrap; gap: 2px 6px; padding-left: 21px; min-width: 0; }
  .pasta { flex: 1 1 100%; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: 10.5px; color: var(--text-muted); }
  .chips { display: flex; flex-wrap: wrap; gap: 4px; }
  .chip { font-size: 10px; padding: 1px 6px; border-radius: 5px; background: var(--fill-subtle); color: var(--text-secondary); white-space: nowrap; }
  .chip-escreveu { background: var(--accent-dim); color: var(--accent); }
  .chip-leu { background: color-mix(in srgb, var(--success) 16%, transparent); color: var(--success); }
  .chip-voce { background: color-mix(in srgb, var(--warning, #ff9f0a) 16%, transparent); color: var(--warning, #ff9f0a); }

  .skel { padding: 2px 0 10px; }
  .skel-linha { display: flex; align-items: center; gap: 5px; padding: 8px 10px; min-height: 40px; }
  .skel-ico, .skel-bar {
    display: block; height: 12px; border-radius: 6px; flex: none;
    background: linear-gradient(90deg, var(--fill-subtle) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, var(--fill-subtle) 75%);
    background-size: 200% 100%; animation: shimmer 1.4s ease-in-out infinite;
  }
  .skel-ico { width: 16px; height: 16px; border-radius: 4px; }
</style>
