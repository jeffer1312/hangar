<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    open: boolean;
    status: StatusFields | null;
    onClose: () => void;
  }
  let { open, status, onClose }: Props = $props();

  const rows = $derived.by(() => {
    const s = status;
    if (!s) return [] as { label: string; value: string }[];
    const out: { label: string; value: string }[] = [];
    if (typeof s.fiveHourPct === 'number')
      out.push({ label: 'Janela 5h', value: `${s.fiveHourPct}%` + (s.fiveHourReset ? ` · reset ${s.fiveHourReset}` : '') });
    if (typeof s.weeklyPct === 'number')
      out.push({ label: 'Janela 7d', value: `${s.weeklyPct}%` + (s.weeklyReset ? ` · reset ${s.weeklyReset}` : '') });
    if (typeof s.ctxUsed === 'number')
      out.push({ label: 'Contexto', value: `${s.ctxUsed.toLocaleString('pt-BR')}${s.ctxTotal ? ' / ' + s.ctxTotal.toLocaleString('pt-BR') : ''}${typeof s.ctxPct === 'number' ? ` (${Math.round(s.ctxPct)}%)` : ''}` });
    if (typeof s.costUsd === 'number')
      out.push({ label: 'Custo', value: `$${s.costUsd.toFixed(2)}` });
    if (s.sessionTime)
      out.push({ label: 'Tempo de sessão', value: s.sessionTime });
    if (s.model)
      out.push({ label: 'Modelo', value: s.model + (s.effort ? ` · ${s.effort}` : '') });
    return out;
  });
</script>

<BottomSheet {open} {onClose} ariaLabel="Uso e limites">
  <div class="usage">
    <h2 class="usage-title">Uso & limites</h2>
    {#each rows as r}
      <div class="usage-row">
        <span class="usage-label">{r.label}</span>
        <span class="usage-value">{r.value}</span>
      </div>
    {/each}
    {#if status?.raw}
      <div class="usage-raw">
        <span class="usage-label">Statusline crua</span>
        <code class="usage-raw-line">{status.raw}</code>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  .usage { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .usage-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-1); }
  .usage-row { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-4); }
  .usage-label { font-size: var(--text-sm); color: var(--text-secondary); }
  .usage-value { font-family: var(--font-mono); font-size: var(--text-sm); font-variant-numeric: tabular-nums; color: var(--text-primary); text-align: right; }
  .usage-raw { display: flex; flex-direction: column; gap: var(--space-1); margin-top: var(--space-2); padding-top: var(--space-3); border-top: 1px solid var(--border-subtle); }
  .usage-raw-line { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); word-break: break-all; white-space: pre-wrap; }
</style>
