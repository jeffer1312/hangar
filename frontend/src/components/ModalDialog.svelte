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

  /* O BLUR mora aqui, no dimmer — nao no painel. O backdrop-filter do painel so alcanca o que esta
     pintado atras dele, e atras dele estava justamente esta camada chapada: borrar uma cor solida
     nao muda nada, e a pagina continuava nitida atraves da transparencia. Borrando o overlay
     inteiro, o que fica atras do modal e de fato desfocado. */
  :global(html[data-liquid]) .modal-backdrop {
    background: rgba(4, 6, 10, 0.34);
    backdrop-filter: blur(16px) saturate(150%);
  }

  /* Foco programatico ao abrir (a11y) desenhava o anel `auto` do Chrome em volta do dialog e do
     botao fechar — a moldura branca grossa dos prints. O container nao precisa de anel: ele nao e
     um alvo de teclado, so o ponto de partida do Tab. */
  .modal-dialog:focus { outline: none; }

  /* MATERIAL do dialog (vidro, sombra, animação): specificity normal, ninguém sobrescreve. */
  .modal-dialog {
    position: relative;   /* ancora a camada de vidro */
    overflow: auto;
    overscroll-behavior: contain;
    /* O fundo mora NO ELEMENTO (regra logo abaixo), nao mais so no ::before: o pseudo e absoluto
       dentro deste proprio scroller e cobria so a altura visivel. Esta linha fica como base do
       caminho liquid, que devolve `transparent` pra refracao do ::before aparecer. */
    background: transparent;
    box-shadow: 0 24px 80px var(--glass-shadow), inset 0 1px 1px var(--glass-specular);
    animation: modal-in 160ms var(--ease-out) both;
  }

  /* CAIXA (tamanho, borda, raio) em `:where()` = specificity ZERO. O consumidor passa `className`
     e estiliza com `:global(.x-dialog)`, que é UMA classe (0,1,0) — perdia pro `.modal-dialog` daqui,
     que o Svelte escopa com a classe de hash (0,2,0). Resultado: TODO modal com className ficava
     nos 560px e nos cantos de 24px, ignorando o próprio CSS. Media: terminal e QR pediam tela
     cheia e abriam como card de 560px; a Atividade pedia 520px e abria 560, deixando faixa vazia.
     Com `:where` o padrão vale só enquanto ninguém disser o contrário. */
  :where(.modal-dialog) {
    width: min(560px, 100%);
    max-height: calc(100dvh - var(--space-8));
    border: 1px solid var(--glass-border);
    border-radius: var(--radius-xl);
  }

  /* Mesmo material do composer/navbar/sidebar: solido no WebKit (o backdrop-filter reproduz o bug
     do retangulo preto), refracao real so no Chromium com data-liquid. */
  /* Mesmo motivo do BottomSheet: o ::before e absoluto e o proprio dialogo e o scroller, entao
     `inset: 0` cobre so a altura VISIVEL e rola junto — conteudo mais alto que a tela (o Avancado
     dos motores, um transcript longo) fica sem vidro na parte de baixo. Fundo no elemento cobre a
     area rolavel inteira; no liquid ele volta a transparente pra refracao do ::before aparecer. */
  .modal-dialog { background: var(--glass-bg-solid); }
  :global(html[data-liquid]) .modal-dialog { background: transparent; }
  .modal-dialog::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
    pointer-events: none;
    background: var(--glass-bg-solid);
  }
  :global(html[data-liquid]) .modal-dialog::before {
    background: var(--glass-panel);
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
