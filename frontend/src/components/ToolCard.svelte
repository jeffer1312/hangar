<script lang="ts">
  import type { ChatEvent } from '../lib/types';
  import { parseFilePaths } from '../lib/format';
  import FileAttachment from './FileAttachment.svelte';

  interface Props {
    event: ChatEvent;
    result?: ChatEvent | null;
    sessionName: string;
    animate?: boolean;   // false = card de HISTORICO remontado (paginacao/janela): sem fade
  }
  let { event, result = null, sessionName, animate = true }: Props = $props();

  let expanded = $state(false);

  // Imagem (ou midia/doc) que o Claude LEU: o transcript dropa o bloco image do tool_result, mas o
  // path do Read esta citado na conversa -> serve pelo /file (parseFilePaths filtra por extensao
  // conhecida, entao codigo/.md nao vira anexo). Reusa a mesma trava + componente do chat.
  const fileRefs = $derived(
    event.tool_name === 'Read'
      ? parseFilePaths(String((event.tool_input as Record<string, unknown> | null)?.['file_path'] ?? ''))
      : []
  );

  const phase = $derived<'pending' | 'done' | 'error'>(
    result === null
      ? 'pending'
      : result.is_error
      ? 'error'
      : 'done'
  );

  const verb = $derived(
    phase === 'pending' ? 'Executando' : phase === 'error' ? 'Falhou' : 'Executou'
  );

  function summarizeInput(toolName: string | null | undefined, input: Record<string, unknown> | null | undefined): string {
    if (!input) return '';
    const name = toolName ?? '';
    if (['Write', 'Read', 'Edit'].includes(name)) {
      const p = (input['file_path'] ?? input['path']) as string | undefined;
      return p ? `path: ${p}` : '';
    }
    if (name === 'Bash') {
      const cmd = (input['command'] as string | undefined) ?? '';
      return `cmd: ${cmd.slice(0, 60)}${cmd.length > 60 ? '…' : ''}`;
    }
    if (name === 'WebSearch') {
      return `query: ${input['query'] ?? ''}`;
    }
    if (name === 'WebFetch') {
      return `url: ${input['url'] ?? ''}`;
    }
    const keys = Object.keys(input);
    if (!keys.length) return '';
    return `${keys[0]}: ${String(input[keys[0]]).slice(0, 60)}`;
  }

  const summary = $derived(summarizeInput(event.tool_name, event.tool_input));

  // Bash com run_in_background retorna NA HORA (vira shell destacado) -> "Executou" engana. Marca
  // como background; a saida viva chega depois pelos cards de BashOutput (o agente puxa via bash_id).
  const isBackground = $derived(
    event.tool_name === 'Bash' && (event.tool_input as Record<string, unknown> | null)?.['run_in_background'] === true
  );
  const label = $derived(isBackground && phase !== 'error' ? 'Rodando em background' : verb);
</script>

<div
  class="tool-row"
  class:noanim={!animate}
  class:tool-row--error={phase === 'error'}
  role="button"
  tabindex="0"
  aria-expanded={expanded}
  onclick={() => (expanded = !expanded)}
  onkeydown={(e) => e.key === 'Enter' && (expanded = !expanded)}
>
  <div class="row-line">
    {#if phase === 'pending'}
      <span class="row-spin" aria-label="Executando…">⟳</span>
    {/if}
    <span class="row-label">
      {label} <span class="row-tool">{event.tool_name ?? 'Tool'}</span>{#if isBackground}<span class="row-badge">background</span>{/if}{#if summary} · {summary}{/if}
    </span>
    <span class="row-chevron" class:open={expanded} aria-hidden="true">›</span>
  </div>

  {#if expanded && result?.result}
    <div class="row-result">
      <pre>{result.result}</pre>
    </div>
  {/if}
</div>

{#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}

<style>
  /* Linha muda colapsada (estilo Claude iOS): "Executou <tool> · <summary> ›". Tap expande. */
  .tool-row {
    padding: var(--space-1) 0;
    margin-bottom: var(--space-1);
    cursor: pointer;
    min-height: 32px;
    animation: bubble-in 180ms ease-out both;
  }

  /* Historico remontado (paginacao/janela): entra parado. */
  .tool-row.noanim { animation: none; }

  .row-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .row-spin {
    flex-shrink: 0;
    color: var(--text-muted);
    display: inline-block;
    animation: spin 0.8s linear infinite;
    font-size: var(--text-xs);
  }

  .row-label {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .row-tool {
    font-family: var(--font-mono);
    color: var(--text-secondary);
  }

  .row-badge {
    flex-shrink: 0;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
    padding: 1px 6px;
    margin-left: 4px;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--accent-dim);
  }

  .tool-row--error .row-label {
    color: var(--error);
  }

  .row-chevron {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    transition: transform 180ms var(--ease-out);
  }

  .row-chevron.open {
    transform: rotate(90deg);
  }

  .row-result {
    margin-top: var(--space-2);
    max-height: 240px;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    border-left: 2px solid var(--border-default);
    padding-left: var(--space-3);
  }

  .row-result pre {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    line-height: 1.35;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
