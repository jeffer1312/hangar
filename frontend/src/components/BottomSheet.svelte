<script lang="ts">
  import type { Snippet } from 'svelte';

  // Shell reutilizavel de bottom-sheet: backdrop + painel que sobe de baixo.
  // Fecha por tap no backdrop, Esc ou swipe pra baixo. Conteudo entra via children.
  interface Props {
    open: boolean;
    onClose: () => void;
    ariaLabel?: string;
    children: Snippet;
  }
  let { open, onClose, ariaLabel = 'Painel', children }: Props = $props();

  // ── Swipe-to-dismiss: o painel acompanha o dedo; solto abaixo do limiar, volta ──
  let dragY = $state(0);
  let snapping = $state(false);
  let startY = 0;
  let dragging = false;
  const DISMISS_PX = 90; // distancia minima de arraste pra fechar ao soltar

  function onTouchStart(e: TouchEvent) {
    if (e.touches.length !== 1) return;
    // nao inicia o arraste quando o toque comeca num controle: preserva forms/rows
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, button, a')) return;
    startY = e.touches[0].clientY;
    dragging = true;
    snapping = false;
  }

  function onTouchMove(e: TouchEvent) {
    if (!dragging) return;
    // so permite arrastar pra baixo (delta positivo)
    dragY = Math.max(0, e.touches[0].clientY - startY);
  }

  function onTouchEnd() {
    if (!dragging) return;
    dragging = false;
    if (dragY > DISMISS_PX) {
      dragY = 0;
      onClose();
      return;
    }
    // volta ao lugar com uma transicao curta
    snapping = true;
    dragY = 0;
  }

  // Fechar por backdrop SO quando o gesto comeca E termina no backdrop. Sem isto, o overlay
  // nativo do <select> no iOS, ao ser descartado, dispara um click-fantasma que cai no backdrop
  // e fechava o sheet inteiro antes do usuario chegar no botao. O click sintetico nao vem com um
  // pointerdown real no backdrop -> pressOnBackdrop fica false -> nao fecha.
  let pressOnBackdrop = false;
  function onBackdropPointerDown(e: PointerEvent) {
    pressOnBackdrop = e.target === e.currentTarget;
  }
  function onBackdropClick(e: MouseEvent) {
    const close = pressOnBackdrop && e.target === e.currentTarget;
    pressOnBackdrop = false;
    if (close) onClose();
  }

  function onKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') onClose();
  }

  // Foco a11y: ao abrir, move o foco pra DENTRO da sheet (a menos que um filho ja tenha focado — ex.
  // a busca do switcher) pra o leitor de tela anunciar o dialog e o Tab ficar no conteudo. Ao fechar,
  // devolve o foco pro gatilho; senao ele cai no body, atras do conteudo.
  let sheetEl = $state<HTMLElement | null>(null);
  let prevFocus: HTMLElement | null = null;
  $effect(() => {
    if (open) {
      prevFocus = document.activeElement as HTMLElement | null;
      requestAnimationFrame(() => {
        if (open && sheetEl && !sheetEl.contains(document.activeElement)) sheetEl.focus();
      });
    } else if (prevFocus) {
      prevFocus.focus?.();
      prevFocus = null;
    }
  });
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onpointerdown={onBackdropPointerDown} onclick={onBackdropClick}>
    <div
      bind:this={sheetEl}
      class="sheet"
      class:snapping
      role="dialog"
      aria-modal="true"
      aria-label={ariaLabel}
      tabindex="-1"
      style={dragY || snapping ? `transform: translateY(${dragY}px)` : undefined}
      ontouchstart={onTouchStart}
      ontouchmove={onTouchMove}
      ontouchend={onTouchEnd}
      ontransitionend={() => (snapping = false)}
    >
      <div class="drag-handle" aria-hidden="true"></div>
      {@render children()}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    z-index: 100;
    display: flex;
    align-items: flex-end;
    justify-content: center;
  }

  .sheet {
    width: 100%;
    max-width: 600px;
    background: var(--bg-elevated);
    border-radius: 20px 20px 0 0;
    padding: var(--space-4) var(--space-5);
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-5));
    animation: slide-up 360ms var(--spring) both;
    touch-action: pan-y;
  }

  /* Snap-back apos um swipe curto (entra so durante o retorno). */
  .sheet.snapping {
    transition: transform 200ms var(--ease-out);
  }

  @keyframes slide-up {
    from { transform: translateY(100%); opacity: 0; }
    to   { transform: translateY(0);    opacity: 1; }
  }

  .drag-handle {
    width: 36px;
    height: 4px;
    background: var(--border-strong);
    border-radius: var(--radius-full);
    margin: 0 auto var(--space-4);
  }

  /* Desktop (>=820px, mesmo corte do DesktopShell): em vez de subir de baixo, DOCA como painel
     lateral direito de altura cheia. Todos os sheets (Git/Usage/...) herdam sem tocar em cada um. */
  @media (min-width: 820px) {
    .backdrop { align-items: stretch; justify-content: flex-end; background: rgba(0, 0, 0, 0.4); }
    .sheet {
      width: min(420px, 92vw); max-width: none; height: 100%;
      border-radius: 0; border-left: 1px solid var(--border-default);
      padding: var(--space-5) var(--space-5);
      padding-bottom: var(--space-5);
      overflow-y: auto;
      animation: slide-in-right 300ms var(--ease-out) both;
      touch-action: auto;
    }
    .sheet.snapping { transition: none; }
    .drag-handle { display: none; }
  }
  @keyframes slide-in-right {
    from { transform: translateX(100%); opacity: 0; }
    to   { transform: translateX(0);    opacity: 1; }
  }
</style>
