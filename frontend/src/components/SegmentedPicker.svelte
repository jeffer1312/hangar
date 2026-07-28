<script lang="ts" generics="T extends string">
  // Segmentado curto (escolha imediata, sem confirmar) — o mesmo formato do ThemeToggle e do
  // BackgroundToggle, extraído porque a Aparência passou a ter três fileiras iguais.
  interface Props {
    value: T;
    options: { v: T; label: string; aria: string }[];
    ariaLabel: string;
    onPick: (v: T) => void;
  }
  let { value, options, ariaLabel, onPick }: Props = $props();
</script>

<div class="seg" role="group" aria-label={ariaLabel}>
  {#each options as o (o.v)}
    <button
      class="seg-opt"
      class:active={value === o.v}
      onclick={() => onPick(o.v)}
      aria-pressed={value === o.v}
      aria-label={o.aria}
      title={o.aria}
    >{o.label}</button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .seg-opt {
    min-height: 32px;
    min-width: 0;
    padding: 0 var(--space-3);
    border-radius: 9px;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    white-space: nowrap;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .seg-opt:hover { color: var(--text-primary); }
  .seg-opt.active {
    background: var(--bg-elevated);
    color: var(--text-primary);
    box-shadow: inset 0 0 0 1px var(--border-default);
  }
</style>
