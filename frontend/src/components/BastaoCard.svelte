<script lang="ts">
  import * as m from '../paraglide/messages';
  import { intlLocale } from '../lib/locale';
  import { renderMarkdown } from '../lib/markdown';
  import { getBastaoDossie } from '../lib/api';
  import type { RecadoBastao } from '../lib/bastaoRecado';
  import HangarMark from './icons/HangarMark.svelte';
  import BottomSheet from './BottomSheet.svelte';

  // Cartão do kick-off `[hangar: passagem de bastão]` — o recado que a sessão SUCESSORA recebe.
  // Mesmo molde do HangarCommandCard e do OrqPainelCard: o desfecho no cabeçalho, o que fazer em
  // passos numerados, o que o dossiê sozinho não resolve como aviso — e o recado original inteiro
  // num bloco fechado, porque o cartão pode ler errado e o recado não.
  interface Props {
    recado: RecadoBastao;
    /** Texto cru do kick-off, pro bloco fechado. */
    cru: string;
    ts?: number | null;
    /** Sessão DESTE chat — é dela que o dossiê gravado é lido. */
    sessionName: string;
    /** Abre o chat da sessão de origem (ela continua viva). */
    onAbrirOrigem?: (() => void) | null;
  }
  let { recado, cru, ts = null, sessionName, onAbrirOrigem = null }: Props = $props();

  const hora = $derived(
    ts ? new Date(ts * 1000).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '',
  );

  let cruAberto = $state(false);
  let dossieAberto = $state(false);
  let dossie = $state('');
  let carregando = $state(false);
  let erro = $state('');

  async function abrirDossie() {
    dossieAberto = true;
    // Já lido nesta sessão de tela: o arquivo não muda depois da passagem (é gravado uma vez,
    // antes de a sessão nascer), então reler a cada abertura seria round-trip por nada.
    if (dossie || carregando) return;
    carregando = true;
    erro = '';
    try {
      dossie = await getBastaoDossie(sessionName);
    } catch (e) {
      // `ensureOk` já traduz o código do backend na mensagem do Error (`errorDetail`).
      erro = e instanceof Error ? e.message : String(e);
    } finally {
      carregando = false;
    }
  }
</script>

<div class="bc">
  <div class="bc-cab">
    <span class="bc-selo">
      <span class="bc-arco"><HangarMark size={21} /></span>
      <!-- Selo de SETA, não a marca do Claude: a passagem é do hangar, e o cartão do `SendMessage`
           (a via nativa) é que leva a marca dele. Os dois ficam irmãos sem se confundir. -->
      <svg class="bc-marca" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.4"
           stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 12h13M12 5l7 7-7 7" />
      </svg>
    </span>
    <span class="bc-titulo">{m.bastao_card_titulo({ nome: recado.origem })}</span>
    {#if hora}<span class="bc-hora">{hora}</span>{/if}
  </div>

  <div class="bc-corpo">
    <p class="bc-msg">{m.bastao_card_resumo()}</p>

    <ol class="bc-passos">
      <li><span class="bc-num">1</span><span>{m.bastao_card_passo_dossie()}</span></li>
      <li><span class="bc-num">2</span><span>{m.bastao_card_passo_plano()}</span></li>
    </ol>
    <div class="bc-arq">{recado.dossie}</div>

    <ul class="bc-avisos">
      <li><span class="bc-pin" aria-hidden="true">▲</span><span>{m.bastao_card_aviso_viva({ nome: recado.origem })}</span></li>
      <li><span class="bc-pin" aria-hidden="true">▲</span><span>{m.bastao_card_aviso_par()}</span></li>
    </ul>

    {#if recado.conta || recado.modelo}
      <div class="bc-chips">
        {#if recado.conta}<span class="bc-chip">{m.bastao_card_de_conta({ conta: recado.conta })}</span>{/if}
        {#if recado.modelo}<span class="bc-chip">{recado.modelo}</span>{/if}
      </div>
    {/if}

    <div class="bc-acoes">
      <button class="bc-btn primaria" onclick={abrirDossie}>{m.bastao_card_abrir_dossie()}</button>
      {#if onAbrirOrigem}
        <button class="bc-btn" onclick={onAbrirOrigem}>{m.hangar_cmd_abrir({ nome: recado.origem })}</button>
      {/if}
    </div>

    <details class="bc-cru" bind:open={cruAberto}>
      <summary>{m.bastao_card_recado_original({ n: cru.trim().split('\n').length })}</summary>
      <pre>{cru.trim()}</pre>
    </details>
  </div>
</div>

<BottomSheet open={dossieAberto} onClose={() => (dossieAberto = false)}
             wide centered ariaLabel={m.bastao_card_abrir_dossie()}>
  <div class="bc-dossie">
    {#if carregando}
      <p class="bc-estado">{m.comum_carregando()}</p>
    {:else if erro}
      <p class="bc-estado bc-erro">{erro}</p>
    {:else}
      <!-- Sem superfície de XSS: `renderMarkdown` escapa tudo antes de montar o HTML. E markdown
           nunca aparece cru no app — um `<pre>` com `## Decisões` à mostra seria bug, não estilo. -->
      <div class="md">{@html renderMarkdown(dossie)}</div>
    {/if}
  </div>
</BottomSheet>

<style>
  .bc {
    border: 1px solid var(--border-subtle);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-lg);
    background: var(--surface-raised);
    overflow: hidden;
    margin: var(--space-1) 0;
    animation: bubble-in 180ms ease-out both;
  }
  .bc-cab {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--fill-subtle);
  }
  /* Mesmo desenho do selo do HangarCommandCard: o arco cede o canto de baixo à direita e o anel
     separa o selo dele — a 13px, encostados, os dois viram uma mancha só. */
  .bc-selo { position: relative; display: block; flex-shrink: 0; width: 21px; height: 21px; color: var(--accent); }
  .bc-arco { position: absolute; inset: 0; display: block; transform: translate(-1px, -1.5px) scale(0.92); }
  .bc-arco :global(svg) { width: 100%; height: 100%; }
  .bc-marca {
    position: absolute;
    right: -3px;
    bottom: -2px;
    width: 13px;
    height: 13px;
    color: var(--accent);
    background: var(--bg-elevated);
    border-radius: var(--radius-full);
    padding: 1.5px;
    box-shadow: 0 0 0 1.5px var(--surface-raised);
  }
  .bc-titulo {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bc-hora {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 10.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .bc-corpo {
    padding: var(--space-2) var(--space-3) var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .bc-msg {
    margin: 0;
    font-size: var(--text-sm);
    line-height: 1.5;
    padding: var(--space-2);
    border-radius: var(--radius-md);
    background: var(--fill-subtle);
    border-left: 2px solid var(--accent);
  }
  .bc-passos { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .bc-passos li { display: flex; gap: 7px; font-size: 12.5px; line-height: 1.45; color: var(--text-secondary); }
  .bc-num {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
    border-radius: var(--radius-full);
    background: var(--accent-dim);
    color: var(--accent);
    font-size: 10px;
    font-weight: 700;
    display: grid;
    place-items: center;
    margin-top: 1px;
  }
  .bc-arq { font-family: var(--font-mono); font-size: 11.5px; color: var(--text-muted); word-break: break-all; }
  .bc-avisos { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 5px; }
  .bc-avisos li { display: flex; gap: 6px; font-size: 12px; line-height: 1.4; color: var(--text-secondary); }
  .bc-pin { color: var(--warning); flex-shrink: 0; }
  .bc-chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .bc-chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    background: var(--fill-subtle);
    color: var(--text-secondary);
  }
  .bc-acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .bc-btn {
    font-size: 11.5px;
    padding: 5px 11px;
    border: none;
    border-radius: var(--radius-full);
    background: var(--fill-subtle);
    color: var(--text-primary);
    cursor: pointer;
  }
  .bc-btn.primaria { background: var(--accent-dim); color: var(--accent); font-weight: 600; }
  .bc-cru { border-top: 1px dashed var(--border-subtle); padding-top: var(--space-2); }
  .bc-cru summary { font-size: 11.5px; color: var(--text-muted); cursor: pointer; list-style: none; }
  .bc-cru summary::before { content: '▸ '; }
  .bc-cru[open] summary::before { content: '▾ '; }
  .bc-cru pre {
    margin: var(--space-2) 0 0;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.55;
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .bc-dossie { padding: var(--space-3); }
  .bc-estado { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .bc-erro { color: var(--error); }
</style>
