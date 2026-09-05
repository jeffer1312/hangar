<script lang="ts">
  import * as m from '../paraglide/messages';
  import { intlLocale } from '../lib/locale';
  import type { RecadoOrq } from '../lib/orqRecado';

  // Cartão do recado `[painel: orquestração]`. Mesmo molde do HangarCommandCard: o desfecho no
  // cabeçalho, o dado em chips, o que fazer em três linhas — e o texto original inteiro num bloco
  // fechado, porque o cartão pode ler errado e o recado não.
  interface Props {
    recado: RecadoOrq;
    /** Texto cru do recado (sem o prefixo), pro bloco fechado. */
    cru: string;
    ts?: number | null;
    /** Abre o modal de orquestração desta sessão. */
    onAbrirPainel?: (() => void) | null;
  }
  let { recado, cru, ts = null, onAbrirPainel = null }: Props = $props();

  const hora = $derived(
    ts ? new Date(ts * 1000).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '',
  );

  let cruAberto = $state(false);
</script>

<div class="oc" class:sucessao={recado.sucessao}>
  <div class="oc-cab">
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <circle cx="12" cy="12" r="3.2" />
      <path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.2 5.2l2.1 2.1M16.7 16.7l2.1 2.1M18.8 5.2l-2.1 2.1M7.3 16.7l-2.1 2.1" />
    </svg>
    <span class="oc-titulo">
      {recado.papeis.length === 1 ? m.orq_card_titulo_um() : m.orq_card_titulo({ n: recado.papeis.length })}
    </span>
    {#if hora}<span class="oc-hora">{hora}</span>{/if}
  </div>

  <div class="oc-corpo">
    <div class="oc-papeis">
      {#each recado.papeis as p (p.papel)}
        <div class="oc-papel">
          <span class="oc-nome">{p.papel}</span>
          <span class="oc-seta" aria-hidden="true">→</span>
          <span class="oc-chip destaque">{p.provider}</span>
          <span class="oc-chip">{p.conta}</span>
          {#if p.modelo}<span class="oc-chip">{p.modelo}</span>{/if}
          {#if p.esforco}<span class="oc-chip">{p.esforco}</span>{/if}
        </div>
      {/each}
    </div>

    <ul class="oc-fazer">
      <li>{m.orq_card_trabalhando()}</li>
      <li>{m.orq_card_parada()}</li>
      <li>{m.orq_card_gravada()}</li>
    </ul>

    {#if recado.sucessao}
      <div class="oc-arb">
        <div class="oc-arb-tit">{m.orq_card_sucessao_tit()}</div>
        <ol>
          <li>{m.orq_card_sucessao_1()}</li>
          <li>{m.orq_card_sucessao_2()}</li>
          <li>{m.orq_card_sucessao_3()}</li>
          <li>{m.orq_card_sucessao_4()}</li>
          <li>{m.orq_card_sucessao_5()}</li>
        </ol>
      </div>
    {/if}

    <div class="oc-regras">{recado.regras}</div>

    {#if onAbrirPainel}
      <div class="oc-acoes">
        <button class="oc-btn" onclick={onAbrirPainel}>{m.orq_card_abrir_painel()}</button>
      </div>
    {/if}

    <details class="oc-cru" bind:open={cruAberto}>
      <summary>{m.orq_card_texto_original()}</summary>
      <pre>{cru}</pre>
    </details>
  </div>
</div>

<style>
  /* Faixa lateral neutra: é configuração, não pessoa (mesma leitura da bolha `panel`). Vira âmbar
     quando o papel mudado é o do próprio árbitro — ali há um rito a cumprir. */
  .oc {
    align-self: flex-start;
    max-width: min(80%, 46rem);
    border: 1px solid var(--border-subtle);
    border-left: 3px solid var(--text-muted);
    border-radius: var(--radius-lg);
    background: var(--surface-raised);
    overflow: hidden;
    margin-bottom: var(--space-3);
    animation: bubble-in 180ms var(--ease-out) both;
  }
  .oc.sucessao { border-left-color: var(--warning); }
  .oc-cab {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--fill-subtle);
  }
  .oc-cab svg { flex-shrink: 0; color: var(--text-muted); }
  .oc.sucessao .oc-cab svg { color: var(--warning); }
  .oc-titulo {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .oc-hora {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 10.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .oc-corpo {
    padding: var(--space-2) var(--space-3) var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .oc-papeis { display: flex; flex-direction: column; gap: 6px; }
  .oc-papel {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 8px;
    border-radius: var(--radius-md);
    background: var(--fill-subtle);
  }
  .oc-nome { font-weight: 700; font-size: 12.5px; color: var(--text-primary); }
  .oc-seta { color: var(--text-muted); font-size: 12px; }
  .oc-chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    background: var(--surface-card);
    color: var(--text-secondary);
  }
  .oc-chip.destaque { background: var(--accent-dim); color: var(--accent); font-weight: 600; }
  .oc-fazer {
    margin: 0;
    padding: 0 0 0 10px;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 5px;
    border-left: 2px solid var(--accent);
  }
  .oc-fazer li { font-size: 12.5px; line-height: 1.45; color: var(--text-secondary); }
  .oc-arb {
    border: 1px dashed var(--warning);
    border-radius: var(--radius-md);
    padding: var(--space-2);
  }
  .oc-arb-tit { font-size: 11.5px; font-weight: 700; color: var(--warning); margin-bottom: 5px; }
  .oc-arb ol { margin: 0; padding-left: 18px; display: flex; flex-direction: column; gap: 4px; }
  .oc-arb li { font-size: 12px; line-height: 1.45; color: var(--text-secondary); }
  .oc-regras {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-muted);
    word-break: break-all;
  }
  .oc-acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .oc-btn {
    font-size: 11.5px;
    padding: 5px 11px;
    border: none;
    border-radius: var(--radius-full);
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
  }
  .oc-cru { border-top: 1px dashed var(--border-subtle); padding-top: var(--space-2); }
  .oc-cru summary { font-size: 11.5px; color: var(--text-muted); cursor: pointer; list-style: none; }
  .oc-cru summary::before { content: '▸ '; }
  .oc-cru[open] summary::before { content: '▾ '; }
  .oc-cru pre {
    margin: var(--space-2) 0 0;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.55;
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
