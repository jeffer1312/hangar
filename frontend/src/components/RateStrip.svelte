<script lang="ts">
  import { parseStatusLine, type StatusFields } from '../lib/statusline';
  import type { ServerBucket } from '../lib/sessions';

  // Faixa de rate-limit no topo do Board/Canvas: as janelas ⚡5h e 📅7d são POR CONTA (servidor),
  // não por sessão — qualquer statusline de qualquer sessão do servidor carrega o mesmo valor,
  // então basta a primeira que tiver os campos parseáveis. Sem statusline nenhuma (tema custom
  // sem os emojis, sessões todas sem captura ainda) o chip do servidor simplesmente não aparece.
  interface Props { buckets: ServerBucket[] }
  let { buckets }: Props = $props();

  interface Chip { id: string; label: string; color: string; st: StatusFields }
  const chips = $derived(buckets.flatMap((b): Chip[] => {
    for (const s of b.sessions) {
      const st = parseStatusLine(s.status_line);
      if (st && (st.fiveHourPct != null || st.weeklyPct != null)) {
        return [{ id: b.server.id, label: b.server.label, color: s.serverColor, st }];
      }
    }
    return [];
  }));
  const pctClass = (p?: number) => (p == null ? '' : p >= 90 ? 'crit' : p >= 70 ? 'warn' : '');
</script>

{#if chips.length}
  <div class="rate-strip">
    {#each chips as c (c.id)}
      <span class="rs-chip">
        <span class="rs-dot" style="background: {c.color}" aria-hidden="true"></span>
        <span class="rs-srv">{c.label}</span>
        {#if c.st.fiveHourPct != null}
          <span class="rs-win {pctClass(c.st.fiveHourPct)}"
                title={c.st.fiveHourReset ? `janela de 5h — reseta ${c.st.fiveHourReset}` : 'janela de 5h'}>
            ⚡{Math.round(c.st.fiveHourPct)}%
          </span>
        {/if}
        {#if c.st.weeklyPct != null}
          <span class="rs-win {pctClass(c.st.weeklyPct)}"
                title={c.st.weeklyReset ? `janela de 7 dias — reseta ${c.st.weeklyReset}` : 'janela de 7 dias'}>
            📅{Math.round(c.st.weeklyPct)}%
          </span>
        {/if}
      </span>
    {/each}
  </div>
{/if}

<style>
  .rate-strip {
    display: flex; flex-wrap: wrap; gap: var(--space-2);
    padding: var(--space-2) var(--space-3) 0;
  }
  .rs-chip {
    display: inline-flex; align-items: center; gap: 6px;
    font-size: var(--text-xs); color: var(--text-secondary);
    background: var(--bg-surface); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-full); padding: 3px 10px;
  }
  .rs-dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
  .rs-srv { font-weight: 600; }
  .rs-win { color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .rs-win.warn { color: var(--warning); }
  .rs-win.crit { color: var(--error); }
</style>
