<script lang="ts">
  import { getThemePref, setThemePref, type ThemePref } from '../lib/theme';

  let pref = $state<ThemePref>(getThemePref());
  function pick(p: ThemePref) {
    pref = p;
    setThemePref(p);
  }

  const opts: { v: ThemePref; label: string; aria: string }[] = [
    { v: 'system', label: 'Auto', aria: 'Seguir o sistema' },
    { v: 'light', label: '☀', aria: 'Claro' },
    { v: 'dark', label: '☾', aria: 'Escuro' },
  ];
</script>

<div class="tt" role="group" aria-label="Tema">
  {#each opts as o (o.v)}
    <button
      class="tt-opt"
      class:active={pref === o.v}
      onclick={() => pick(o.v)}
      aria-pressed={pref === o.v}
      aria-label={o.aria}
    >{o.label}</button>
  {/each}
</div>

<style>
  .tt {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .tt-opt {
    min-height: 32px;
    min-width: 44px;
    padding: 0 var(--space-3);
    border-radius: 9px;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .tt-opt.active {
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
  }
</style>
