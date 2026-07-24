<script lang="ts">
  import { tick } from 'svelte';
  import type { Snippet } from 'svelte';
  import { focusableElements, nextFocusIndex } from '../lib/focusCycle';

  interface Props {
    open: boolean;
    ariaLabel: string;
    onClose: () => void;
    initialFocus?: HTMLElement | null;
    closeOnBackdrop?: boolean;
    className?: string;
    layer?: 'default' | 'command';
    role?: 'dialog' | 'alertdialog';
    children: Snippet;
  }

  let {
    open,
    ariaLabel,
    onClose,
    initialFocus = null,
    closeOnBackdrop = true,
    className = '',
    layer = 'default',
    role = 'dialog',
    children,
  }: Props = $props();

  let dialog = $state<HTMLElement | null>(null);
  let previousFocus: HTMLElement | null = null;
  let pressOnBackdrop = false;

  // Escape transformed/filtered ancestors so fixed positioning always covers the viewport.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      },
    };
  }

  $effect.pre(() => {
    if (open) {
      previousFocus = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    }
  });

  $effect(() => {
    if (open) {
      void tick().then(() => {
        if (!open || !dialog) return;
        const preferred = initialFocus?.isConnected && dialog.contains(initialFocus)
          ? initialFocus
          : null;
        (preferred ?? focusableElements(dialog)[0] ?? dialog).focus();
      });
    } else if (previousFocus) {
      const target = previousFocus;
      previousFocus = null;
      void tick().then(() => {
        if (!open && target.isConnected) target.focus();
      });
    }
  });

  function onDialogKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== 'Tab' || !dialog) return;

    event.preventDefault();
    event.stopPropagation();

    const elements = focusableElements(dialog);
    if (!elements.length) {
      dialog.focus();
      return;
    }

    const activeIndex = elements.indexOf(document.activeElement as HTMLElement);
    const nextIndex = activeIndex < 0
      ? (event.shiftKey ? elements.length - 1 : 0)
      : nextFocusIndex(activeIndex, elements.length, event.shiftKey ? -1 : 1);
    elements[nextIndex].focus();
  }

  // Only a pointer gesture that both starts and clicks on the backdrop may dismiss.
  // Synthetic clicks (notably after closing native iOS controls) are intentionally ignored.
  function rememberBackdrop(event: PointerEvent) {
    pressOnBackdrop = event.target === event.currentTarget;
  }

  function closeBackdrop(event: MouseEvent) {
    const shouldClose = closeOnBackdrop
      && pressOnBackdrop
      && event.target === event.currentTarget;
    pressOnBackdrop = false;
    if (shouldClose) onClose();
  }
</script>

{#if open}
  <!-- The backdrop is pointer-only; dialog content must provide its own keyboard close control. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    use:portal
    class="modal-backdrop"
    class:command={layer === 'command'}
    onpointerdown={rememberBackdrop}
    onclick={closeBackdrop}
  >
    <div
      bind:this={dialog}
      class="modal-dialog {className}"
      {role}
      aria-modal="true"
      aria-label={ariaLabel}
      tabindex="-1"
      onkeydown={onDialogKeydown}
    >
      {@render children()}
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-4);
    background: rgba(4, 6, 10, 0.6);
  }

  .modal-backdrop.command {
    z-index: 1100;
  }

  .modal-dialog {
    width: min(560px, 100%);
    max-height: calc(100dvh - var(--space-8));
    overflow: auto;
    overscroll-behavior: contain;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-xl);
    box-shadow: 0 24px 80px rgba(0, 0, 0, 0.48);
    animation: modal-in 160ms var(--ease-out) both;
  }

  @keyframes modal-in {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.985);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }

  @keyframes modal-fade-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  @media (prefers-reduced-motion: reduce) {
    .modal-dialog {
      animation: modal-fade-in 0.01ms linear both;
    }
  }
</style>
