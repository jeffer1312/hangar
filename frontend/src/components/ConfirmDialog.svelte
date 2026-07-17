<script lang="ts">
  // Chassi único dos modais de confirmação do desktop (era 6 cópias de
  // .confirm-backdrop/.confirm-card/.confirm-actions no Sidebar). O corpo entra por snippet
  // (children), compilado no escopo do CHAMADOR — CSS específico do corpo fica lá.
  import type { Snippet } from 'svelte';
  interface Action { label: string; kind?: 'danger' | 'primary'; disabled?: boolean; onClick: () => void }
  interface Props {
    title: string;
    aria: string;
    role?: 'dialog' | 'alertdialog';
    wide?: boolean;
    actions: Action[];
    onClose: () => void;
    children?: Snippet;
  }
  let { title, aria, role = 'alertdialog', wide = false, actions, onClose, children }: Props = $props();
</script>

<div class="confirm-backdrop" onclick={onClose} role="presentation"></div>
<div class="confirm-card" class:wide {role} aria-modal="true" aria-label={aria}>
  <p class="confirm-title">{title}</p>
  {@render children?.()}
  <div class="confirm-actions">
    {#each actions as a (a.label)}
      <button type="button" class="c-btn" class:c-danger={a.kind === 'danger'} class:c-primary={a.kind === 'primary'} disabled={a.disabled} onclick={a.onClick}>{a.label}</button>
    {/each}
  </div>
</div>

<style>
  .confirm-backdrop { position: fixed; inset: 0; z-index: 50; background: rgba(0, 0, 0, 0.5); }
  .confirm-card {
    position: fixed; z-index: 51; top: 50%; left: 50%; transform: translate(-50%, -50%);
    width: min(340px, 90vw); padding: var(--space-5);
    display: flex; flex-direction: column; gap: var(--space-2);
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-lg); box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    animation: confirm-in 160ms var(--ease-out) both;
  }
  .confirm-card.wide { width: min(440px, 92vw); }
  @keyframes confirm-in {
    from { opacity: 0; transform: translate(-50%, -48%) scale(0.97); }
    to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  }
  .confirm-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .confirm-actions { display: flex; gap: var(--space-2); margin-top: var(--space-2); }
  .c-btn {
    flex: 1; height: 40px; border-radius: var(--radius-md);
    font-size: var(--text-sm); font-weight: 600;
    background: var(--bg-hover); color: var(--text-secondary);
  }
  .c-btn:hover { background: var(--bg-surface); }
  .c-danger { background: var(--error); color: #fff; }
  .c-danger:hover { background: var(--error); filter: brightness(1.08); }
  .c-primary { background: var(--accent); border-color: var(--accent); color: var(--bg-base); font-weight: 600; }
</style>
