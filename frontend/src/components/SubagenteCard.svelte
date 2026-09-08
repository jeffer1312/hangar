<script lang="ts">
  import * as m from '../paraglide/messages';
  import { intlLocale } from '../lib/locale';
  import { renderMarkdown } from '../lib/markdown';
  import { rotuloSubagente, subagenteFalhou, type SubagenteCodex } from '../lib/subagenteCodex';

  // Cartão da notificação de subagente do Codex. Mesmo molde do OrqPainelCard: desfecho no
  // cabeçalho, corpo renderizado, e a notificação crua num bloco fechado — o cartão pode ler
  // errado, o texto original não.
  interface Props {
    sub: SubagenteCodex;
    /** Texto cru da mensagem (envelope incluído), pro bloco fechado. */
    cru: string;
    ts?: number | null;
  }
  let { sub, cru, ts = null }: Props = $props();

  const erro = $derived(subagenteFalhou(sub.status));
  const hora = $derived(
    ts ? new Date(ts * 1000).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '',
  );
  const titulo = $derived(rotuloSubagente(sub.status));
  // Só os 8 primeiros: o agent_path é um uuid inteiro e come a linha do cabeçalho no celular.
  const curto = $derived(sub.agentPath.slice(0, 8));

  let cruAberto = $state(false);
</script>

<div class="sc" class:erro>
  <div class="sc-cab">
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <rect x="3" y="4" width="18" height="12" rx="2.5" />
      <path d="M8 20h8M12 16v4M9 9.5h.01M15 9.5h.01" />
    </svg>
    <span class="sc-titulo">{titulo}</span>
    {#if curto}<span class="sc-id">{curto}</span>{/if}
    {#if hora}<span class="sc-hora">{hora}</span>{/if}
  </div>

  <div class="sc-corpo">
    <!-- Sem superfície de XSS: `renderMarkdown` escapa tudo antes de montar o HTML. E markdown
         nunca aparece cru no app — o relatório do subagente vem com `**` e listas. -->
    <div class="md">{@html renderMarkdown(sub.texto)}</div>

    <details class="sc-cru" bind:open={cruAberto}>
      <summary>{m.subagente_card_original()}</summary>
      <pre>{cru.trim()}</pre>
    </details>
  </div>
</div>

<style>
  .sc {
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
  .sc.erro { border-left-color: var(--error); }
  .sc-cab {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--fill-subtle);
  }
  .sc-cab svg { flex-shrink: 0; color: var(--text-muted); }
  .sc.erro .sc-cab svg { color: var(--error); }
  .sc-titulo { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .sc-id {
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-muted);
    padding: 1px 6px;
    border-radius: var(--radius-full);
    background: var(--surface-card);
  }
  .sc-hora {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 10.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .sc-corpo {
    padding: var(--space-2) var(--space-3) var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .md {
    font-size: var(--text-xs);
    line-height: 1.55;
    color: var(--text-secondary);
    word-break: break-word;
  }
  /* Mesma tipografia do PlanPanel.md — é o mesmo caso: markdown de outra ferramenta num bloco
     estreito. */
  .md :global(h1) { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-primary); }
  .md :global(h2) { margin: var(--space-3) 0 var(--space-1); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); }
  .md :global(h3) { margin: var(--space-2) 0 var(--space-1); font-size: var(--text-xs); color: var(--text-secondary); }
  .md :global(p) { margin: 0 0 var(--space-2); }
  .md :global(p:last-child) { margin-bottom: 0; }
  .md :global(strong) { color: var(--text-primary); font-weight: 650; }
  .md :global(ul), .md :global(ol) { margin: 0 0 var(--space-2); padding-left: 1.2em; }
  .md :global(li) { margin: 2px 0; }
  .md :global(code) { padding: 0 4px; border-radius: 3px; background: var(--surface-inset); font-family: var(--font-mono); font-size: 11px; color: var(--text-primary); }
  .md :global(pre) { margin: 0 0 var(--space-2); padding: var(--space-2); overflow-x: auto; border-radius: var(--radius-sm); background: var(--surface-inset); }
  .md :global(.code-block pre) { background: none; border: none; border-radius: 0; margin: 0; }
  .md :global(a) { color: var(--accent); }
  .sc-cru { border-top: 1px dashed var(--border-subtle); padding-top: var(--space-2); }
  .sc-cru summary { font-size: 11.5px; color: var(--text-muted); cursor: pointer; list-style: none; }
  .sc-cru summary::before { content: '▸ '; }
  .sc-cru[open] summary::before { content: '▾ '; }
  .sc-cru pre {
    margin: var(--space-2) 0 0;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.55;
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
