<script lang="ts">
  import type { SessionInfo, State } from '../lib/types';
  import { stateLabels, stateColors } from '../lib/format';
  import Lottie from './Lottie.svelte';
  import pensando from '../lib/lottie/pensando.json';

  interface Props {
    session: SessionInfo;
    serverBadge?: { label: string; color: string } | null;
    onClick: () => void;
    onDelete: () => void;
    onResume?: () => void;
    onRename?: (newName: string) => void;
    onGit?: () => void;
    // Modo seleção do broadcast (feature #9): row vira checkbox (toque alterna); swipe/rename ficam
    // fora enquanto seleciona, pra não competir com o toque de marcar.
    selectMode?: boolean;
    selected?: boolean;
    onToggleSelect?: () => void;
  }
  let {
    session, serverBadge = null, onClick, onDelete, onResume, onRename, onGit,
    selectMode = false, selected = false, onToggleSelect,
  }: Props = $props();


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

  // Travada (feature #7): "working" ha muito tempo sem avancar (watchdog do backend). Tinge o chip
  // de estado com um anel âmbar sutil — nao grita, so avisa.
  const stalled = $derived(session.stalled === true);

  // Rate-limit radar (feature #8): banner de limite de uso detectado no pane (best-effort). Chip
  // proprio "⏳ HH:MM" ao lado do state-chip — calmo, so avisa quando volta.
  const limited = $derived(session.limited === true);

  // ── Swipe-to-delete ────────────────────────────────────────────────────────
  // Arrasta a linha pra esquerda revelando "Excluir". touch-action:pan-y deixa o scroll vertical
  // pro navegador e o horizontal pra gente. Distingue tap / swipe-x / scroll-y por eixo dominante.
  const OPEN = -84;
  let offset = $state(0);
  let startX = 0, startY = 0, startOffset = 0;
  let dragging = false;
  let axis: 'x' | 'y' | null = null;
  let suppressClick = false;

  // ── Renomear por TOQUE LONGO (500ms parado, sem swipe) -> edita o nome inline (espelha o Sidebar) ──
  let editing = $state(false);
  let editValue = $state('');
  let longPressed = false;
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  function startPress() {
    longPressed = false;
    clearTimeout(pressTimer);
    if (untracked) return;                       // sessao sem id confiavel nao renomeia
    pressTimer = setTimeout(() => { longPressed = true; editValue = session.name; editing = true; }, 500);
  }
  function cancelPress() { clearTimeout(pressTimer); }
  function saveRename() {
    const nv = editValue.trim();
    editing = false;
    if (nv && nv !== session.name) onRename?.(nv);   // o SSE de sessions re-emite com o nome novo
  }
  function onEditKey(e: KeyboardEvent) {
    if (e.key === 'Enter') { e.preventDefault(); (e.target as HTMLInputElement).blur(); }
    else if (e.key === 'Escape') { editing = false; }
  }
  function editAutofocus(node: HTMLInputElement) { node.focus(); node.select(); }

  function onDown(e: PointerEvent) {
    if (editing || selectMode) return;            // selecionando: sem swipe/rename, so toggle no tap
    startX = e.clientX; startY = e.clientY; startOffset = offset;
    dragging = true; axis = null; suppressClick = false;
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    startPress();                                // arma o long-press (cancelado por movimento/soltar)
  }
  function onMove(e: PointerEvent) {
    if (!dragging) return;
    const dx = e.clientX - startX, dy = e.clientY - startY;
    if (axis === null) {
      if (Math.abs(dx) > 6 || Math.abs(dy) > 6) { axis = Math.abs(dx) > Math.abs(dy) ? 'x' : 'y'; cancelPress(); }
    }
    if (axis === 'y') { dragging = false; return; } // scroll vertical -> solta
    if (axis === 'x') {
      suppressClick = true;
      offset = Math.max(OPEN, Math.min(0, startOffset + dx));
    }
  }
  function onUp() {
    cancelPress();
    if (!dragging) return;
    dragging = false;
    if (axis === 'x') offset = offset < OPEN / 2 ? OPEN : 0; // snap aberto/fechado
  }

  // Tap na linha: toque longo (renomeou) nao navega; se aberto ou acabou de arrastar, fecha o swipe.
  function onRowClick() {
    if (selectMode) { if (!untracked) onToggleSelect?.(); return; }  // sem id -> nao entra no broadcast
    if (longPressed) { longPressed = false; return; }   // foi toque longo (renomear) -> nao abre o chat
    if (suppressClick || offset !== 0) { offset = 0; return; }
    if (!untracked) onClick();
  }
</script>

<div class="swipe-wrap" class:open={offset === OPEN}>
  <!-- inert enquanto fechado: fica ATRAS da row (z-order) e, sem isto, seguia focavel por Tab e na
       arvore de a11y — Enter deletava sem feedback visivel, e AT-click por coordenada caia na row. -->
  <button class="del-action" onclick={onDelete} aria-label="Excluir sessão {session.name}" inert={offset !== OPEN}>
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
    aria-pressed={selectMode ? selected : undefined}
    onclick={onRowClick}
    onkeydown={(e) => {
      if (e.key !== 'Enter' && e.key !== ' ') return;
      if (untracked) return;
      e.preventDefault();
      if (selectMode) onToggleSelect?.(); else onClick();
    }}
    onpointerdown={onDown}
    onpointermove={onMove}
    onpointerup={onUp}
    onpointercancel={onUp}
  >
    <span class="lead" aria-hidden="true">
      {#if selectMode}
        <!-- Checkbox visual (a semantica de check fica no role="checkbox" da row -> so decorativo). -->
        <input type="checkbox" class="select-check" checked={selected} tabindex="-1" aria-hidden="true" />
      {:else if session.state === 'working'}
        <!-- Working -> "pensando" animando, cores originais. -->
        <Lottie data={pensando as any} size={20} loop autoplay />
      {:else}
        <!-- Outros estados -> mesmo icone PARADO em f0 (anel cheio), cores ORIGINAIS. -->
        <Lottie data={pensando as any} size={20} loop={false} autoplay={false} frame={STATIC_FRAME} />
      {/if}
    </span>

    <div class="row-info">
      <span class="name-row">
        {#if editing}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="name-edit"
            bind:value={editValue}
            use:editAutofocus
            onclick={(e) => e.stopPropagation()}
            onpointerdown={(e) => e.stopPropagation()}
            onkeydown={onEditKey}
            onblur={saveRename}
            aria-label="Novo nome da sessão"
          />
        {:else}
          <span class="session-name">{title}</span>
        {/if}
        {#if untracked}
          <span class="untracked-badge" title="claude aberto sem --session-id: nao da pra rastrear o transcript com seguranca">⚠ sem id</span>
        {/if}
      </span>
      {#if session.state === 'awaiting_input' && session.question}
        <span class="status-sub asking" title={session.question}>{session.question}</span>
      {:else if session.state === 'working' && session.label}
        <span class="status-sub working" title={session.label}>{session.label}</span>
      {/if}
      <span class="meta-line">
        {#if serverBadge}
          <span class="srv" style="color: {serverBadge.color};">{serverBadge.label}</span>
          {#if session.cwd}<span class="meta-sep">·</span>{/if}
        {/if}
        {#if session.cwd}
          <span class="cwd" title={session.cwd}><span class="cwd-prefix">{cwdParts.prefix}</span><span class="cwd-base">{cwdParts.base}</span></span>
        {/if}
      </span>
      {#if session.branch}
        <!-- Branch numa linha própria: no celular ela concorria com o cwd e ficava truncada. -->
        <span class="meta-line">
          <span class="branch" title="branch git atual">⎇ {session.branch}</span>
        </span>
      {/if}
      {#if untracked}
        <button
          class="resume-btn"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onResume?.(); }}
        >↻ Retomar conversa</button>
      {/if}
    </div>

    <div class="row-right">
      {#if session.pair_peers?.length}
        <!-- Grupo de trabalho: paridade com o chain-chip da Sidebar (1 par = nome; N = contagem). -->
        <span class="paired-chip" title={`Grupo com ${session.pair_peers.join(', ')}`}>🤝&nbsp;{session.pair_peers.length === 1 ? session.pair_peers[0] : session.pair_peers.length + 1}</span>
      {/if}
      {#if limited}
        <span
          class="limited-chip"
          title={session.limit_reset ? `Limite de uso atingido — volta ${session.limit_reset}` : 'Limite de uso atingido'}
        >⏳{#if session.limit_reset}&nbsp;{session.limit_reset}{/if}</span>
      {/if}
      <span
        class="state-chip"
        class:stalled
        style="color: {stateColors[session.state]}; background: {stateChipBg[session.state]};"
        title={stalled ? 'Pode estar travada — sem atividade há um tempo' : undefined}
      >
        {stateLabels[session.state]}
      </span>
      <!-- Git da sessao: abre o gerenciador (GitSheet) no repo do cwd, sem abrir o chat. So aparece
           quando ha cwd (repo) — paridade com o menu de contexto do desktop. stopPropagation pra nao
           disparar swipe/navegacao da linha. -->
      {#if session.cwd}
        <button
          class="git-btn"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onGit?.(); }}
          aria-label="Git de {session.name}"
          title="Gerenciador git"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="6" y1="3" x2="6" y2="15"/>
            <circle cx="18" cy="6" r="3"/>
            <circle cx="6" cy="18" r="3"/>
            <path d="M18 9a9 9 0 0 1-9 9"/>
          </svg>
        </button>
      {/if}
      <!-- Caminho de exclusao por TECLADO/leitor de tela: o swipe-to-delete e pointer-only e deixava
           o usuario de teclado/SR sem como excluir no mobile. Escondido visualmente (mouse/touch usam
           swipe), mas sempre focavel e anunciado; reaparece no foco de teclado (:focus-visible). -->
      <button
        class="del-kbd"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={(e) => { e.stopPropagation(); onDelete(); }}
        aria-label="Excluir sessão {session.name}"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <polyline points="3 6 5 6 21 6"/>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
          <path d="M10 11v6M14 11v6"/>
        </svg>
      </button>
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
  /* Checkbox do modo seleção (feature #9): so decorativo (o toque na row inteira alterna). */
  .select-check {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
    pointer-events: none;
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
  /* Input do rename inline (toque longo). Mesmo visual do .server-edit da lista de servidores. */
  .name-edit {
    flex: 1;
    min-width: 0;
    height: 32px;
    background: var(--bg-base);
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    font-weight: 600;
    padding: 0 var(--space-2);
    outline: none;
  }

  .meta-line {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    font-size: var(--text-xs);
  }
  /* Subtítulo de estado vivo: a pergunta (awaiting) ou o texto do spinner (working), truncado —
     deixa a linha acionável sem abrir a sessão (feature #1). */
  .status-sub {
    font-size: var(--text-xs);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status-sub.asking { color: var(--warning); font-weight: 600; }
  .status-sub.working { color: var(--text-secondary); font-style: italic; }
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
  .branch {
    flex: 0 1 auto;
    min-width: 0;
    font-family: var(--font-mono);
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  /* Travada (feature #7): anel âmbar sutil no chip — avisa sem gritar. */
  .state-chip.stalled {
    box-shadow: inset 0 0 0 1px var(--warning);
  }

  /* Rate-limit radar (feature #8): chip proprio, mesma familia visual do stalled (âmbar, calmo). */
  .paired-chip {
    font-size: 11px;
    font-weight: 600;
    padding: 3px 9px;
    border-radius: var(--radius-full);
    white-space: nowrap;
    max-width: 9em;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--accent);
    background: var(--accent-dim);
  }

  .limited-chip {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 3px 9px;
    border-radius: var(--radius-full);
    white-space: nowrap;
    color: var(--warning);
    background: rgba(255, 159, 10, 0.12);
    font-variant-numeric: tabular-nums;
  }

  /* Escondido do layout (mouse/touch usam swipe) mas SEMPRE na arvore de a11y (SR anuncia "Excluir
     sessao X"). No foco de teclado (:focus-visible) vira um botao 40px visivel na row-right. Sobrescreve
     o min-height/min-width 44px global do <button> pra sumir de fato quando fechado. */
  .del-kbd {
    position: absolute;
    width: 1px; height: 1px; min-width: 0; min-height: 0;
    padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%);
    color: var(--text-muted); border-radius: var(--radius-sm);
  }
  .del-kbd:focus-visible {
    position: static;
    width: 40px; height: 40px; min-width: 40px; min-height: 40px;
    margin: 0; overflow: visible; clip-path: none;
    color: var(--error);
    outline: 2px solid var(--accent); outline-offset: -2px;
  }
  /* Botao git dedicado na row-right: area de toque 40px, cinza ate ser tocado. */
  .git-btn {
    width: 40px; height: 40px; min-width: 40px; min-height: 40px;
    flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--text-muted); border-radius: var(--radius-sm);
  }
  .git-btn:active { color: var(--accent); background: var(--bg-hover); }
  .chev {
    color: var(--text-muted);
    font-size: var(--text-lg);
    line-height: 1;
  }
</style>
