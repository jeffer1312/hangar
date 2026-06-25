<script lang="ts">
  import IconTool from './icons/IconTool.svelte';
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

  const resultPreview = $derived(
    phase === 'error' && result?.result
      ? result.result.slice(0, 80)
      : null
  );
</script>

<div
  class="tool-card"
  class:tool-card--error={phase === 'error'}
  role="button"
  tabindex="0"
  aria-expanded={expanded}
  onclick={() => (expanded = !expanded)}
  onkeydown={(e) => e.key === 'Enter' && (expanded = !expanded)}
>
  <div class="card-header">
    <span class="card-icon">
      <IconTool />
    </span>
    <span class="tool-name">{event.tool_name ?? 'Tool'}</span>
    <span class="card-status" aria-label={phase}>
      {#if phase === 'pending'}
        <span class="spinner" aria-label="Executando…">⟳</span>
      {:else if phase === 'done'}
        <span class="check">✓</span>
      {:else}
        <span class="cross">✗</span>
      {/if}
    </span>
  </div>

  {#if summary}
    <div class="card-summary">{summary}</div>
  {/if}

  {#if resultPreview}
    <div class="card-preview">▸ {resultPreview}</div>
  {/if}

  {#if expanded && result?.result}
    <div class="card-result">
      <pre>{result.result}</pre>
    </div>
  {/if}
</div>

<style>
  .tool-card {
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    padding: var(--space-3) var(--space-4);
    margin-bottom: var(--space-2);
    cursor: pointer;
    transition: border-color 180ms ease-out, background 180ms ease-out;
    animation: bubble-in 180ms ease-out both;
    min-height: 44px;
  }

  .tool-card:active {
    background: var(--bg-hover);
  }

  .tool-card--error {
    border-color: var(--error);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .card-icon {
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }

  .tool-name {
    font-size: var(--text-sm);
    font-family: var(--font-mono);
    font-weight: 500;
    color: var(--text-primary);
    flex: 1;
  }

  .card-status {
    font-size: var(--text-sm);
    flex-shrink: 0;
  }

  .spinner {
    color: var(--text-muted);
    display: inline-block;
    animation: spin 0.8s linear infinite;
  }

  .check {
    color: var(--success);
    font-weight: 600;
  }

  .cross {
    color: var(--error);
    font-weight: 600;
  }

  .card-summary {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    margin-top: var(--space-1);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-preview {
    font-size: var(--text-xs);
    color: var(--error);
    margin-top: var(--space-1);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-result {
    margin-top: var(--space-2);
    max-height: 200px;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .card-result pre {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    line-height: 1.3;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
