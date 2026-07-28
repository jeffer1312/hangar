<script lang="ts">
  import type { Snippet } from 'svelte';
  import { untrack } from 'svelte';
  import { focusableElements, nextFocusIndex } from '../lib/focusCycle';

  // Shell reutilizavel de bottom-sheet: backdrop + painel que sobe de baixo.
  // Fecha por tap no backdrop, Esc ou swipe pra baixo. Conteudo entra via children.
  interface Props {
    open: boolean;
    onClose: () => void;
    ariaLabel?: string;
    resizable?: boolean;   // opt-in: Git e Par. Habilita drag-resize no dock desktop (>=820px).
    // Chave da largura persistida + largura inicial. Sem isto todo sheet redimensionavel dividia a
    // MESMA largura: arrastar o painel do par mexia no do git, que nao tem nada a ver.
    widthKey?: string;
    defaultWidth?: number;
    wide?: boolean;        // opt-in: dock desktop usa largura fixa min(1100px, 92vw) em vez de --sheet-w.
    centered?: boolean;    // opt-in: no desktop vira MODAL centrado em vez de painel docado a direita.
    children: Snippet;
  }
  let { open, onClose, ariaLabel = 'Painel', resizable = false, widthKey = 'cp_gitsheet_w', defaultWidth = 460, wide = false, centered = false, children }: Props = $props();

  // ── Redimensionar (SO no dock desktop >=820px): arrasta a borda ESQUERDA do painel direito.
  // Largura persistida em localStorage; aplicada via --sheet-w (a media query desktop consome a var,
  // o mobile ignora -> sheet de baixo continua 100%). Mesma mecanica do resize da Sidebar.
  // WMAX subiu de 720: o painel do par carrega o contrato do grupo, que e um documento inteiro.
  const WMIN = 360, WMAX = 980;
  const clampW = (w: number) => Math.max(WMIN, Math.min(WMAX, w));
  // Leitura ÚNICA na montagem, de propósito (o aviso do compilador é sobre isso): a largura vira
  // estado local editável pelo arrasto; reagir à prop depois sobrescreveria o que o usuário puxou.
  let width = $state(0);
  // Leitura ÚNICA na montagem: a largura vira estado local editável pelo arrasto, então reagir à
  // prop depois sobrescreveria o que o usuário puxou. `untrack` diz isso ao compilador.
  $effect.pre(() => {
    if (width === 0) width = clampW(Number(localStorage.getItem(untrack(() => widthKey))) || untrack(() => defaultWidth));
  });
  let resizing = $state(false);
  function resizeStart(e: PointerEvent) {
    resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function resizeMove(e: PointerEvent) {
    if (resizing) width = clampW(window.innerWidth - e.clientX);   // painel colado na direita
  }
  function resizeEnd() {
    if (!resizing) return;
    resizing = false;
    try { localStorage.setItem(widthKey, String(width)); } catch { /* storage off/cheio */ }
  }

  // ── Swipe-to-dismiss: o painel acompanha o dedo; solto abaixo do limiar, volta ──
  let dragY = $state(0);
  let snapping = $state(false);
  let startY = 0;
  let dragging = false;
  const DISMISS_PX = 90; // distancia minima de arraste pra fechar ao soltar

  // Style combinado do painel: transform do swipe (mobile) + --sheet-w (desktop resizable).
  const sheetStyle = $derived(
    (dragY || snapping ? `transform: translateY(${dragY}px);` : '') +
    (resizable ? `--sheet-w: ${width}px;` : ''),
  );

  function onTouchStart(e: TouchEvent) {
    if (e.touches.length !== 1) return;
    // nao inicia o arraste quando o toque comeca num controle: preserva forms/rows
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, button, a')) return;
    // conteudo rolado pra baixo: o gesto e scroll interno, nao dismiss — senao o swipe
    // que deveria voltar o scroll pro topo arrastava o sheet junto
    if (sheetEl && sheetEl.scrollTop > 0) return;
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
    if (e.key === 'Escape') {
      // A sheet owns Escape while it is open. Prevent the event from reaching
      // Chat/DesktopShell global handlers, which would otherwise dismiss an
      // overlay behind this one in the same keypress.
      e.preventDefault();
      // This listener is attached to window; stopPropagation alone still
      // allows sibling window listeners to run. Stop them as well so only the
      // sheet that owns this fallback handles the key.
      e.stopImmediatePropagation();
      onClose();
    }
  }

  function onSheetKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      // Handle Escape at the dialog boundary as well as the window fallback.
      // This is the normal path for focused controls inside the sheet and
      // guarantees that lower global listeners never see the key.
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key !== 'Tab' || !sheetEl || !window.matchMedia('(min-width: 820px)').matches) return;
    e.preventDefault();
    e.stopPropagation();
    const elements = focusableElements(sheetEl);
    if (!elements.length) {
      sheetEl.focus();
      return;
    }
    const activeIndex = elements.indexOf(document.activeElement as HTMLElement);
    const nextIndex = activeIndex < 0
      ? (e.shiftKey ? elements.length - 1 : 0)
      : nextFocusIndex(activeIndex, elements.length, e.shiftKey ? -1 : 1);
    elements[nextIndex].focus();
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
    } else if (prevFocus?.isConnected) {
      prevFocus.focus();
      prevFocus = null;
    }
  });
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" class:centered onpointerdown={onBackdropPointerDown} onclick={onBackdropClick}>
    <div
      bind:this={sheetEl}
      class="sheet"
      class:snapping
      class:resizing
      class:wide
      class:centered
      role="dialog"
      aria-modal="true"
      aria-label={ariaLabel}
      tabindex="-1"
      style={sheetStyle || undefined}
      ontouchstart={onTouchStart}
      ontouchmove={onTouchMove}
      ontouchend={onTouchEnd}
      onkeydown={onSheetKeydown}
      ontransitionend={() => (snapping = false)}
    >
      {#if resizable}
        <!-- Handle de resize (so visivel no dock desktop via CSS): arrasta a borda esquerda. -->
        <div
          class="resize-handle"
          onpointerdown={resizeStart}
          onpointermove={resizeMove}
          onpointerup={resizeEnd}
          onpointercancel={resizeEnd}
          role="separator"
          aria-label="Redimensionar painel"
          aria-orientation="vertical"
        ></div>
      {/if}
      <div class="drag-handle" aria-hidden="true"></div>
      {@render children()}
    </div>
  </div>
{/if}

<style>
  /* Mesmo motivo do ModalDialog: o blur vai no dimmer, senao o filtro do painel so borra a cor
     chapada do proprio backdrop e a tela de tras continua nitida. */
  :global(html[data-liquid]) .backdrop {
    background: rgba(0, 0, 0, 0.34);
    backdrop-filter: blur(16px) saturate(150%);
  }

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
    position: relative;   /* ancora a camada de vidro (::before) tambem no mobile */
    width: 100%;
    max-width: 600px;
    /* Vidro: o fundo sai do elemento e vai pro leaf ::before, mesma mecanica do composer/navbar —
       o painel passa a pertencer ao mesmo material do resto do chrome. */
    background: transparent;
    border-radius: 20px 20px 0 0;
    padding: var(--space-4) var(--space-5);
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-5));
    animation: slide-up 360ms var(--spring) both;
    touch-action: pan-y;
    /* conteudo alto (LoopSheet com guia aberto, etc.) NUNCA pode passar da tela:
       teto + scroll interno; overscroll contido pra nao rolar a pagina atras */
    max-height: calc(100dvh - var(--space-8));
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    overscroll-behavior: contain;
  }

  /* Camada de vidro do painel: leaf sem conteudo, colada na caixa. WebKit/iOS fica no fundo quase
     opaco (sem backdrop-filter, que reproduz o bug do retangulo preto no scroll). */
  .sheet::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
    pointer-events: none;
    background: var(--glass-bg-solid);
  }
  /* Chromium (data-liquid): translucido de verdade, com blur.
     O backdrop-filter fica no ELEMENTO, nao no ::before: o painel anima com `transform`
     (slide-in-right, fill `both`), e um elemento transformado vira o backdrop ROOT dos proprios
     filhos — o pseudo passava a "borrar" o interior vazio do painel, ou seja, nada. Resultado: o
     painel ficava translucido SEM blur e o conteudo de tras aparecia cru (foi o que apareceu no
     "Uso & limites"). No elemento, o backdrop e o que esta atras dele de verdade. */
  :global(html[data-liquid]) .sheet::before {
    background: var(--glass-panel);
  }

  /* O painel leva .focus() programatico ao abrir (a11y: anuncia o dialog e prende o Tab dentro).
     Sem isto o Chrome desenha o anel `auto` em volta do painel INTEIRO — a borda branca grossa que
     aparecia no modal centrado (no dock lateral ela ficava colada na borda da tela e passava
     despercebida). :focus-visible NAO resolve: o Chrome casa nele em foco programatico de
     tabindex=-1. Tirar o anel do contorno e o que o <dialog> nativo faz; quem navega por teclado
     ainda ve o anel dos controles DE DENTRO, que e onde o Tab para. */
  .sheet:focus { outline: none; }

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
  /* Handle de resize: escondido por padrao (mobile = sheet de baixo, largura 100%). */
  .resize-handle { display: none; }

  @media (min-width: 820px) {
    .backdrop { align-items: stretch; justify-content: flex-end; background: rgba(0, 0, 0, 0.4); }
    .sheet {
      position: relative;   /* ancora o resize-handle */
      width: var(--sheet-w, min(420px, 92vw)); max-width: 92vw; height: 100%; max-height: none;
      border-radius: 0; border-left: 1px solid var(--border-default);
      padding: var(--space-5) var(--space-5);
      padding-bottom: var(--space-5);
      overflow-y: auto;
      animation: slide-in-right 300ms var(--ease-out) both;
      touch-action: auto;
    }
    .sheet.snapping { transition: none; }
    /* Enquanto arrasta o resize: sem animacao (segue o ponteiro sem lag). */
    .sheet.resizing { animation: none; }
    .drag-handle { display: none; }
    /* Handle na borda ESQUERDA do painel docado (arrasta pra esquerda -> alarga). */
    .resize-handle {
      display: block; position: absolute; top: 0; left: 0; width: 8px; height: 100%;
      cursor: col-resize; touch-action: none; z-index: 6;
    }
    .resize-handle:hover { background: var(--accent-dim); }
    .sheet.wide { width: min(1100px, 92vw); max-width: 92vw; }
    .sheet.wide .resize-handle { display: none; }  /* largura fixa no modo largo */

    /* `centered`: em vez de docar na direita, vira MODAL centrado. Opt-in porque a maioria dos
       sheets é painel de acompanhamento (fica aberto do lado do chat); um formulário que exige
       decisão antes de voltar — como o de motores — cabe melhor no meio, longe da borda. */
    .backdrop.centered { align-items: center; justify-content: center; background: rgba(0, 0, 0, 0.55); }
    .sheet.centered {
      height: auto; max-height: calc(100dvh - var(--space-8));
      border: 1px solid var(--border-default); border-radius: var(--radius-lg);
      animation: modal-in 220ms var(--ease-out) both;
    }
    .sheet.centered .resize-handle { display: none; }
  }
  @keyframes modal-in {
    from { transform: scale(0.97); opacity: 0; }
    to   { transform: scale(1);    opacity: 1; }
  }
  @keyframes slide-in-right {
    from { transform: translateX(100%); opacity: 0; }
    to   { transform: translateX(0);    opacity: 1; }
  }
</style>
