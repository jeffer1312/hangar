<script lang="ts">
  import type { ChatEvent } from '../lib/types';

  interface Props {
    event: ChatEvent;
    result?: ChatEvent | null;
  }
  let { event, result = null }: Props = $props();

  let expanded = $state(false);

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
</script>

<div
  class="tool-row"
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
      {verb} <span class="row-tool">{event.tool_name ?? 'Tool'}</span>{#if summary} · {summary}{/if}
    </span>
    <span class="row-chevron" class:open={expanded} aria-hidden="true">›</span>
  </div>

  {#if expanded && result?.result}
    <div class="row-result">
      <pre>{result.result}</pre>
    </div>
  {/if}
</div>

<style>
  /* Linha muda colapsada (estilo Claude iOS): "Executou <tool> · <summary> ›". Tap expande. */
  .tool-row {
    padding: var(--space-1) 0;
    margin-bottom: var(--space-1);
    cursor: pointer;
    min-height: 32px;
    animation: bubble-in 180ms ease-out both;
  }

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
