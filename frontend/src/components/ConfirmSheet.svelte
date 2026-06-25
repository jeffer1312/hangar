<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';

  interface Props {
    open: boolean;
    title: string;
    message?: string | null;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onClose: () => void;
  }
  let { open, title, message = null, confirmLabel = 'Confirmar', cancelLabel = 'Cancelar', danger = false, onConfirm, onClose }: Props = $props();

  function confirm() {
    onConfirm();
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={title}>
  <div class="confirm">
    <h2 class="confirm-title">{title}</h2>
    {#if message}<p class="confirm-msg">{message}</p>{/if}
    <div class="confirm-actions">
      <button class="btn btn-cancel" onclick={onClose}>{cancelLabel}</button>
      <button class="btn btn-confirm" class:danger onclick={confirm}>{confirmLabel}</button>
    </div>
  </div>
</BottomSheet>

<style>
  .confirm { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .confirm-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .confirm-msg { font-size: var(--text-sm); color: var(--text-secondary); }
  .confirm-actions { display: flex; gap: var(--space-3); margin-top: var(--space-2); }
  .btn { flex: 1; height: 48px; border-radius: var(--radius-md); font-size: var(--text-sm); font-weight: 500; }
  .btn-cancel { background: var(--bg-hover); color: var(--text-secondary); }
  .btn-confirm { background: var(--accent); color: #fff; }
  .btn-confirm.danger { background: var(--error); color: #fff; }
</style>
