<script lang="ts">
  import type { SessionInfo, State } from '../lib/types';
  import Lottie from './Lottie.svelte';
  import pensando from '../lib/lottie/pensando.json';

  interface Props {
    session: SessionInfo;
    serverBadge?: { label: string; color: string } | null;
    onClick: () => void;
    onDelete: () => void;
    onResume?: () => void;
  }
  let { session, serverBadge = null, onClick, onDelete, onResume }: Props = $props();

  const stateLabels: Record<State, string> = {
    working: 'em execução',
    idle: 'pronto',
    awaiting_input: 'aguardando',
    dead: 'encerrado',
  };

  const stateColors: Record<State, string> = {
    working: 'var(--accent)',
    idle: 'var(--success)',
    awaiting_input: 'var(--warning)',
    dead: 'var(--error)',
  };

  // Frame parado da "pensando": f0 = anel cheio e simetrico (os frames do meio do loop ficam ralos e
  // parecem bug). Mesmo frame em todos os estados parados; o que muda entre eles e a COR (tint).
  const STATIC_FRAME = 0;

  // Fundo translucido do chip de status, por estado.
  const stateChipBg: Record<State, string> = {
    working: 'var(--accent-dim)',
    idle: 'rgba(52,199,89,0.12)',
    awaiting_input: 'rgba(255,159,10,0.14)',
    dead: 'rgba(255,69,58,0.12)',
  };

  const title = $derived(session.name);

  // O que identifica a sessao e a ULTIMA pasta do cwd (nome do projeto). Ellipsis padrao corta o
  // fim e some justo com ela; entao split em prefixo (truncavel) + basename (nunca encolhe).
  const cwdParts = $derived.by(() => {
    const p = (session.cwd ?? '').replace(/\/+$/, '');
    const i = p.lastIndexOf('/');
    return i < 0 ? { prefix: '', base: p } : { prefix: p.slice(0, i + 1), base: p.slice(i + 1) };
  });

  // Sessao sem vinculo confiavel (claude manual sem --session-id): NAO da pra abrir o chat com
  // seguranca. Marca "sem id" e bloqueia o clique (delete continua valendo).
  const untracked = $derived(session.tracked === false);

  // "Precisa de voce": aguardando input -> barra de acao + fundo tingido.
  const action = $derived(session.state === 'awaiting_input');

  // ── Swipe-to-delete ────────────────────────────────────────────────────────
  // Arrasta a linha pra esquerda revelando "Excluir". touch-action:pan-y deixa o scroll vertical
  // pro navegador e o horizontal pra gente. Distingue tap / swipe-x / scroll-y por eixo dominante.
  const OPEN = -84;
  let offset = $state(0);
  let startX = 0, startY = 0, startOffset = 0;
  let dragging = false;
  let axis: 'x' | 'y' | null = null;
  let suppressClick = false;

  function onDown(e: PointerEvent) {
    startX = e.clientX; startY = e.clientY; startOffset = offset;
    dragging = true; axis = null; suppressClick = false;
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  }
  function onMove(e: PointerEvent) {
    if (!dragging) return;
    const dx = e.clientX - startX, dy = e.clientY - startY;
    if (axis === null) {
      if (Math.abs(dx) > 6 || Math.abs(dy) > 6) axis = Math.abs(dx) > Math.abs(dy) ? 'x' : 'y';
    }
    if (axis === 'y') { dragging = false; return; } // scroll vertical -> solta
    if (axis === 'x') {
      suppressClick = true;
      offset = Math.max(OPEN, Math.min(0, startOffset + dx));
    }
  }
  function onUp() {
    if (!dragging) return;
    dragging = false;
    if (axis === 'x') offset = offset < OPEN / 2 ? OPEN : 0; // snap aberto/fechado
  }

  // Tap na linha: se aberto ou se acabou de arrastar, nao navega (fecha o swipe). Senao abre o chat.
  function onRowClick() {
    if (suppressClick || offset !== 0) { offset = 0; return; }
    if (!untracked) onClick();
  }
</script>

<div class="swipe-wrap" class:open={offset === OPEN}>
  <button class="del-action" onclick={onDelete} aria-label="Excluir sessão {session.name}">
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <polyline points="3 6 5 6 21 6"/>
      <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
      <path d="M10 11v6M14 11v6"/>
    </svg>
    <span>Excluir</span>
  </button>

  <div
    class="session-row"
    class:action
    class:untracked
    class:dragging
    style="transform: translateX({offset}px);"
    role="button"
    tabindex="0"
    aria-disabled={untracked}
    onclick={onRowClick}
    onkeydown={(e) => e.key === 'Enter' && !untracked && onClick()}
    onpointerdown={onDown}
    onpointermove={onMove}
    onpointerup={onUp}
    onpointercancel={onUp}
  >
    <span class="lead" aria-hidden="true">
      {#if session.state === 'working'}
        <!-- Working -> "pensando" animando, cores originais. -->
        <Lottie data={pensando as any} size={20} loop autoplay />
      {:else}
        <!-- Outros estados -> mesmo icone PARADO em f0 (anel cheio), cores ORIGINAIS. -->
        <Lottie data={pensando as any} size={20} loop={false} autoplay={false} frame={STATIC_FRAME} />
      {/if}
    </span>

    <div class="row-info">
      <span class="name-row">
        <span class="session-name">{title}</span>
        {#if untracked}
          <span class="untracked-badge" title="claude aberto sem --session-id: nao da pra rastrear o transcript com seguranca">⚠ sem id</span>
        {/if}
      </span>
      <span class="meta-line">
        {#if serverBadge}
          <span class="srv" style="color: {serverBadge.color};">{serverBadge.label}</span>
          {#if session.cwd}<span class="meta-sep">·</span>{/if}
        {/if}
        {#if session.cwd}
          <span class="cwd" title={session.cwd}><span class="cwd-prefix">{cwdParts.prefix}</span><span class="cwd-base">{cwdParts.base}</span></span>
        {/if}
      </span>
      {#if untracked}
        <button
          class="resume-btn"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onResume?.(); }}
        >↻ Retomar conversa</button>
      {/if}
    </div>

    <div class="row-right">
      <span class="state-chip" style="color: {stateColors[session.state]}; background: {stateChipBg[session.state]};">
        {stateLabels[session.state]}
      </span>
      <span class="chev" aria-hidden="true">›</span>
    </div>
  </div>
</div>

<style>
  /* Wrapper do swipe: esconde o "Excluir" que fica atras da linha. */
  .swipe-wrap {
    position: relative;
    overflow: hidden;
    border-bottom: 1px solid var(--border-subtle);
  }

  .del-action {
    position: absolute;
    right: 0;
    top: 0;
    bottom: 0;
    width: 84px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    background: var(--error);
    color: #fff;
    font-size: 12px;
    font-weight: 600;
    border-radius: 0;
  }

  .session-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    min-height: 60px;
    background: var(--bg-base);
    cursor: pointer;
    touch-action: pan-y;
    transition: transform 200ms var(--ease-out), background 160ms ease-out;
  }
  /* Enquanto arrasta, sem transicao no transform (segue o dedo). */
  .session-row.dragging {
    transition: background 160ms ease-out;
  }
  .session-row:active {
    background: var(--bg-surface);
  }

  /* "Precisa de voce": barra de acao na lateral + fundo levemente tingido. Tinta OPACA (camada
     sobre o --bg-base): translucida deixava o "Excluir" vermelho atras vazar no swipe-to-delete. */
  .session-row.action {
    background: linear-gradient(rgba(255, 159, 10, 0.06), rgba(255, 159, 10, 0.06)), var(--bg-base);
  }
  .session-row.action::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: var(--warning);
  }

  /* Sem id confiavel: textos apagados (chat off), mas o botao Retomar fica nitido/clicavel. NAO usar
     opacity na row inteira: translucida deixa o "Excluir" vermelho de tras vazar no swipe-to-delete
     (mesmo motivo do fundo OPACO no .action). */
  .session-row.untracked {
    cursor: not-allowed;
  }
  .session-row.untracked .session-name,
  .session-row.untracked .meta-line,
  .session-row.untracked .lead {
    opacity: 0.55;
  }
  .resume-btn {
    align-self: flex-start;
    margin-top: 3px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--accent);
    background: var(--accent-dim);
    border: 1px solid var(--accent);
    border-radius: var(--radius-full);
    padding: 3px 10px;
    cursor: pointer;
  }
  .resume-btn:active {
    background: var(--accent);
    color: #fff;
  }

  /* Slot do indicador: largura fixa pra alinhar os nomes (anim 20px centralizada). */
  .lead {
    width: 20px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .row-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .name-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .session-name {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta-line {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    font-size: var(--text-xs);
  }
  .srv {
    font-weight: 600;
    flex-shrink: 0;
    max-width: 90px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta-sep { color: var(--text-muted); flex-shrink: 0; }
  .cwd {
    display: flex;
    min-width: 0;
    font-family: var(--font-mono);
  }
  .cwd-prefix {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
  }
  .cwd-base {
    flex: 0 0 auto;
    white-space: nowrap;
    color: var(--text-secondary);
  }

  .untracked-badge {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 1px 7px;
    border-radius: var(--radius-full);
    color: var(--warning);
    border: 1px solid var(--warning);
    white-space: nowrap;
  }
  .row-right {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }
  .state-chip {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 3px 9px;
    border-radius: var(--radius-full);
    white-space: nowrap;
  }
  .chev {
    color: var(--text-muted);
    font-size: var(--text-lg);
    line-height: 1;
  }
</style>
