<script lang="ts">
  import type { SessionInfo, State } from '../lib/types';
  import { stateLabels, stateColors, untrackedReason, providerTag } from '../lib/format';
  import { loopBadge, LOOP_TONE_COLOR } from '../lib/loop';
  import { planBadge } from '../lib/plan';
  import PlanBar from './PlanBar.svelte';
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
    onLoop?: () => void;
    // Modo seleção do broadcast (feature #9): row vira checkbox (toque alterna); swipe/rename ficam
    // fora enquanto seleciona, pra não competir com o toque de marcar.
    selectMode?: boolean;
    selected?: boolean;
    onToggleSelect?: () => void;
  }
  let {
    session, serverBadge = null, onClick, onDelete, onResume, onRename, onGit, onLoop,
    selectMode = false, selected = false, onToggleSelect,
  }: Props = $props();


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

  // Celular e estreito: o cwd so entra quando ACRESCENTA algo. Quando o basename ja e o nome da
  // sessao ("claude-cockpit" + "/home/jeff…/claude-cockpit"), a linha inteira e redundante e so
  // roubava largura do nome/branch.
  const showCwd = $derived(!!session.cwd && cwdParts.base.toLowerCase() !== session.name.toLowerCase());

  // Chip de estado so quando o estado PEDE atencao. "pronto" repetido em toda linha e ruido: o
  // ponto colorido do lead ja diz que esta parada.
  const showStateChip = $derived(session.state !== 'idle');

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

  const loopChip = $derived(loopBadge(session.loop_status, session.loop_iter, session.loop_max));
  const planChip = $derived(planBadge(session));
  // Provider da linha — só as não-Claude ganham chip (ver providerTag em lib/format).
  const provTag = $derived(providerTag(session.provider));

  // ── Swipe-to-actions ───────────────────────────────────────────────────────
  // Arrasta a linha pra esquerda revelando Git / Loop / Excluir. touch-action:pan-y deixa o scroll
  // vertical pro navegador e o horizontal pra gente. Distingue tap / swipe-x / scroll-y por eixo
  // dominante. Git e Loop moraram na row-right ate aqui: 2 botoes de 40px + chip + chevron comiam
  // quase metade da largura e o cwd/nome viviam truncados no iPhone. Sao acoes raras -> swipe.
  const ACTION_W = 64;
  const OPEN = $derived(session.cwd ? -3 * ACTION_W : -ACTION_W);
  let offset = $state(0);
  let startX = 0, startY = 0, startOffset = 0;
  let dragging = $state(false);
  // OPEN depende do cwd, que vem do poll do tmux (pode aparecer/sumir a qualquer tick). Com a trilha
  // aberta, isso deixava offset num valor que nao e 0 nem OPEN — e ai o inert congelava justo os
  // botoes visiveis (toque nao fazia nada, calado). Divergiu fora do arrasto: fecha.
  $effect(() => { if (!dragging && offset !== 0 && offset !== OPEN) offset = 0; });
  let axis: 'x' | 'y' | null = null;
  let suppressClick = $state(false);
  let capturedTarget: HTMLElement | null = null;
  let capturedPointerId: number | null = null;

  // ── Renomear por TOQUE LONGO (500ms parado, sem swipe) -> edita o nome inline (espelha o Sidebar) ──
  let editing = $state(false);
  let editValue = $state('');
  let longPressed = $state(false);
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  function startPress() {
    longPressed = false;
    clearTimeout(pressTimer);
    pressTimer = undefined;
    if (untracked) return;                       // sessao sem id confiavel nao renomeia
    pressTimer = setTimeout(() => {
      pressTimer = undefined;
      if (!dragging || axis !== null) return;
      longPressed = true;
      editValue = session.name;
      editing = true;
    }, 500);
  }
  function cancelPress() {
    clearTimeout(pressTimer);
    pressTimer = undefined;
  }
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
    capturedTarget = e.currentTarget as HTMLElement;
    capturedPointerId = e.pointerId;
    capturedTarget.setPointerCapture?.(e.pointerId);
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
  function releasePointerCapture() {
    if (capturedTarget && capturedPointerId !== null && capturedTarget.hasPointerCapture?.(capturedPointerId)) {
      capturedTarget.releasePointerCapture?.(capturedPointerId);
    }
    capturedTarget = null;
    capturedPointerId = null;
  }
  function onUp() {
    cancelPress();
    releasePointerCapture();
    if (dragging && axis === 'x') offset = offset < OPEN / 2 ? OPEN : 0; // snap aberto/fechado
    dragging = false;
    axis = null;
  }
  function onCancel() {
    cancelPress();
    releasePointerCapture();
    if (dragging) offset = startOffset;
    dragging = false;
    axis = null;
    suppressClick = false;
  }

  // Tap na linha: toque longo (renomeou) nao navega; se aberto ou acabou de arrastar, fecha o swipe.
  function onRowClick() {
    if (selectMode) { if (!untracked) onToggleSelect?.(); return; }  // sem id -> nao entra no broadcast
    if (longPressed) { longPressed = false; return; }   // foi toque longo (renomear) -> nao abre o chat
    if (suppressClick || offset !== 0) { offset = 0; return; }
    if (!untracked) onClick();
  }
</script>

<div class="swipe-wrap">
  <!-- inert enquanto fechado: fica ATRAS da row (z-order) e, sem isto, seguia focavel por Tab e na
       arvore de a11y — Enter deletava sem feedback visivel, e AT-click por coordenada caia na row.
       Teclado/leitor de tela usam os botoes .kbd-only da row-right (o swipe e pointer-only). -->
  <!-- `hidden` (visibilidade, nao display) quando a linha esta no lugar: a linha agora e
       TRANSLUCIDA com papel de parede, e a trilha atras dela vazaria pela frente — inclusive a
       faixa vermelha do Excluir. Sai junto do `inert`, que ja escondia do teclado/leitor. -->
  <div class="swipe-actions" class:oculta={offset === 0} inert={offset !== OPEN}>
    {#if session.cwd}
      <button class="act git" onclick={() => { offset = 0; onGit?.(); }} aria-label="Git de {session.name}">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <line x1="6" y1="3" x2="6" y2="15"/>
          <circle cx="18" cy="6" r="3"/>
          <circle cx="6" cy="18" r="3"/>
          <path d="M18 9a9 9 0 0 1-9 9"/>
        </svg>
        <span>Git</span>
      </button>
      <button class="act loop" onclick={() => { offset = 0; onLoop?.(); }} aria-label="Loop de {session.name}">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="m17 2 4 4-4 4"/>
          <path d="M3 11v-1a4 4 0 0 1 4-4h14"/>
          <path d="m7 22-4-4 4-4"/>
          <path d="M21 13v1a4 4 0 0 1-4 4H3"/>
        </svg>
        <span>Loop</span>
      </button>
    {/if}
    <button class="act del" onclick={onDelete} aria-label="Excluir sessão {session.name}">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <polyline points="3 6 5 6 21 6"/>
        <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
        <path d="M10 11v6M14 11v6"/>
      </svg>
      <span>Excluir</span>
    </button>
  </div>

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
    onpointercancel={onCancel}
  >
    <span class="lead" aria-hidden="true">
      {#if selectMode}
        <!-- Checkbox visual (a semantica de check fica no role="checkbox" da row -> so decorativo). -->
        <input type="checkbox" class="select-check" checked={selected} tabindex="-1" aria-hidden="true" />
      {:else if session.state === 'working'}
        <!-- Working -> "pensando" animando, cores originais. -->
        <Lottie data={pensando as any} size={20} loop autoplay />
      {:else}
        <!-- Parada -> ponto na COR do estado. O icone parado era igual em todos os estados: numa
             lista sem nada animando as linhas ficavam indistinguiveis. -->
        <span class="state-dot" style="background: {stateColors[session.state]};"></span>
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
          <span class="untracked-badge" title={untrackedReason(session.provider)}>⚠ sem id</span>
        {/if}
      </span>
      {#if session.state === 'awaiting_input' && session.question}
        <span class="status-sub asking" title={session.question}>{session.question}</span>
      {:else if session.state === 'working' && session.label}
        <span class="status-sub working" title={session.label}>{session.label}</span>
      {/if}
      <!-- UMA meta-line só (antes eram duas: cwd e branch). Ordem = importancia: a branch e o que
           muda, entao vem primeiro e nunca some; o cwd fecha a linha e trunca primeiro. -->
      {#if serverBadge || session.branch || showCwd}
        <span class="meta-line">
          {#if serverBadge}
            <span class="srv" style="color: {serverBadge.color};">{serverBadge.label}</span>
            {#if session.branch || showCwd}<span class="meta-sep">·</span>{/if}
          {/if}
          {#if session.branch}
            <span class="branch" title="branch git atual">⎇ {session.branch}</span>
            {#if showCwd}<span class="meta-sep">·</span>{/if}
          {/if}
          {#if showCwd}
            <span class="cwd" title={session.cwd}><span class="cwd-prefix">{cwdParts.prefix}</span><span class="cwd-base">{cwdParts.base}</span></span>
          {/if}
        </span>
      {/if}
      {#if provTag || session.pair_peers?.length || limited || loopChip || session.engine || planChip}
        <!-- Chips informativos (🤝 grupo, ⏳ rate-limit, 🔁 loop, ⚙ motor) moram AQUI, no fluxo da
             coluna de texto — na row-right eles esmagavam o nome e o cwd vazava por baixo (visto no iPhone). -->
        <span class="badges-line">
          {#if provTag}
            <!-- Identidade, não estado: vem primeiro e em tinta neutra (nada de accent/warning, que
                 já falam "motor" e "erro" nesta mesma linha). "Sessão Pi" pro leitor de tela, senão
                 sairia um "Pi" solto entre os outros chips. -->
            <span class="prov-chip" title={`Sessão ${provTag}`}><span class="sr-only">Sessão&nbsp;</span>{provTag}</span>
          {/if}
          {#if session.pair_peers?.length}
            <span class="paired-chip" title={`Grupo com ${session.pair_peers.join(', ')}`}>🤝&nbsp;{session.pair_peers.length === 1 ? session.pair_peers[0] : session.pair_peers.length + 1}</span>
          {/if}
          {#if limited}
            <span
              class="limited-chip"
              title={session.limit_reset ? `Limite de uso atingido — volta ${session.limit_reset}` : 'Limite de uso atingido'}
            >⏳{#if session.limit_reset}&nbsp;{session.limit_reset}{/if}</span>
          {/if}
          {#if loopChip}
            <span
              class="paired-chip"
              style="color: {LOOP_TONE_COLOR[loopChip.tone]}; background: color-mix(in srgb, {LOOP_TONE_COLOR[loopChip.tone]} 14%, transparent);"
              title="Loop runner"
            >{loopChip.label}</span>
          {/if}
          {#if planChip}
            <span class="plan-chip" class:plan-chip--done={planChip.complete} title={planChip.title}>{planChip.label}</span>
          {/if}
          {#if session.engine}
            <!-- Sem isto nada na lista distingue uma sessão de motor de uma da conta Anthropic. NÃO
                 mostramos custo aqui: o preço que o Claude Code calcula é tabela Anthropic e mentiria. -->
            <span class="engine-chip" title={`Motor: ${session.engine}`}>⚙&nbsp;{session.engine}</span>
          {/if}
        </span>
      {/if}
      <PlanBar {session} />
      <!-- Retomar e Claude-only de ponta a ponta (candidatos de ~/.claude/projects + relance com
           `claude --resume`): numa sessao Pi o botao so poderia errar, entao mostramos a razao no
           lugar dele. O backend recusa igual, pra um cliente velho nao matar o pane. -->
      {#if untracked && session.provider === 'pi'}
        <span class="untracked-hint">{untrackedReason(session.provider)}</span>
      {:else if untracked}
        <button
          class="resume-btn"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onResume?.(); }}
        >↻ Retomar conversa</button>
      {/if}
    </div>

    <div class="row-right">
      <!-- Chip so quando o estado pede atencao (idle = ponto colorido no lead, sem pilula "pronto"
           repetida em toda linha). -->
      {#if showStateChip}
        <span
          class="state-chip"
          class:stalled
          style="color: {stateColors[session.state]}; background: {stateChipBg[session.state]};"
          title={stalled ? 'Pode estar travada — sem atividade há um tempo' : undefined}
        >
          {stateLabels[session.state]}
        </span>
      {:else}
        <!-- Sem chip, o estado so existia como COR (o .lead e aria-hidden) — leitor de tela ficava
             sem saber que a sessao esta pronta. Texto so pra AT (SC 1.4.1). -->
        <span class="sr-only">{stateLabels[session.state]}</span>
      {/if}
      <!-- Caminho por TECLADO/leitor de tela pras acoes do swipe (git / loop / excluir), que e
           pointer-only. Escondidos visualmente, sempre focaveis e anunciados; reaparecem como botao
           de 40px no foco de teclado (:focus-visible). -->
      {#if session.cwd}
        <button
          class="kbd-only"
          inert={offset === OPEN}
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onGit?.(); }}
          aria-label="Git de {session.name}"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="6" y1="3" x2="6" y2="15"/>
            <circle cx="18" cy="6" r="3"/>
            <circle cx="6" cy="18" r="3"/>
            <path d="M18 9a9 9 0 0 1-9 9"/>
          </svg>
        </button>
        <button
          class="kbd-only"
          inert={offset === OPEN}
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onLoop?.(); }}
          aria-label="Loop de {session.name}"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m17 2 4 4-4 4"/>
            <path d="M3 11v-1a4 4 0 0 1 4-4h14"/>
            <path d="m7 22-4-4 4-4"/>
            <path d="M21 13v1a4 4 0 0 1-4 4H3"/>
          </svg>
        </button>
      {/if}
      <button
        class="kbd-only del"
        inert={offset === OPEN}
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
      <!-- Alternativa por TOQUE SIMPLES pras acoes do swipe (WCAG 2.2 SC 2.5.7, Dragging Movements):
           quem nao completa um arrasto abre a trilha no tap. O swipe continua igual. -->
      <button
        class="chev"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={(e) => { e.stopPropagation(); offset = offset === OPEN ? 0 : OPEN; }}
        aria-label="Ações de {session.name}"
        aria-expanded={offset === OPEN}
      >›</button>
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

  /* Trilha de acoes revelada pelo swipe: Git | Loop | Excluir. */
  .swipe-actions {
    position: absolute;
    right: 0;
    top: 0;
    bottom: 0;
    display: flex;
  }
  /* opacity, nao display:none: o layout da trilha (3 botoes de 64px) precisa continuar medido pro
     OPEN bater com a largura real. */
  .swipe-actions.oculta { opacity: 0; }
  .swipe-actions .act {
    width: 64px;
    min-width: 64px;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    font-size: 12px;
    font-weight: 600;
    border-radius: 0;
  }
  .swipe-actions .git { background: var(--bg-elevated); color: var(--text-secondary); }
  .swipe-actions .loop { background: var(--accent-dim); color: var(--accent); }
  .swipe-actions .del { background: var(--error); color: #fff; }

  .session-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    min-height: 60px;
    /* Com papel de parede esta linha some (regra em app.css, junto do #app/.chat-screen): sobre a
       foto ela nao pinta nada, e o que separa uma sessao da outra e a divisoria do .swipe-wrap. */
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
  /* Sem botao de retomar (Pi): a razao vira texto, mesma faixa da linha. */
  .untracked-hint {
    align-self: flex-start;
    margin-top: 3px;
    font-size: var(--text-xs);
    color: var(--text-secondary);
    line-height: 1.35;
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
  /* Estado parado: ponto na cor do estado (verde pronto / âmbar aguardando / vermelho encerrado). */
  .state-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex-shrink: 0;
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
  /* Chips informativos no fluxo da coluna de texto (nao na row-right). */
  .badges-line {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-wrap: wrap;
    min-width: 0;
    margin-top: 2px;
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
    /* Dividindo a meta-line com a branch, o cwd cede primeiro (shrink maior) — a branch e o que
       muda e o que o usuario procura. */
    flex-shrink: 4;
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
    /* encolhe COM ellipsis: "flex: 0 0 auto" nunca encolhia e o basename vazava por baixo
       da row-right (overlap visto no iPhone). O prefixo continua encolhendo primeiro. */
    flex: 0 1 auto;
    min-width: 3ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  /* Progresso do plano do superpowers (Task 3). */
  .plan-chip {
    padding: 1px 6px;
    border-radius: var(--radius-full);
    background: var(--accent-dim);
    color: var(--accent);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    /* O rotulo agora carrega o NOME do plano, que e longo e variavel: sem teto ele empurrava o resto
       da linha de chips pra fora. Corta o nome com reticencias e mantem a linha inteira. */
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 22ch;
  }
  .plan-chip--done {
    background: color-mix(in srgb, var(--success) 14%, transparent);
    color: var(--success);
  }

  /* Motor de modelo (Task 5): sessao rodando fora da conta Anthropic. */
  .engine-chip {
    font-size: 10px; font-weight: 700; letter-spacing: 0.02em;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
    flex-shrink: 0;
  }

  /* Provider da sessão (Codex/Pi). Mesma caixa do engine-chip, tinta NEUTRA: é rótulo de identidade,
     não alarme nem destaque — não pode competir com o estado, o ⚠ sem id ou o motor. */
  .prov-chip {
    font-size: 10px; font-weight: 700; letter-spacing: 0.02em;
    color: var(--text-muted); background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    padding: 1px 6px; border-radius: var(--radius-full);
    white-space: nowrap; flex-shrink: 0;
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
  .kbd-only {
    position: absolute;
    width: 1px; height: 1px; min-width: 0; min-height: 0;
    padding: 0; margin: -1px; overflow: hidden; clip-path: inset(50%);
    color: var(--text-muted); border-radius: var(--radius-sm);
  }
  .kbd-only:focus-visible {
    position: static;
    width: 40px; height: 40px; min-width: 40px; min-height: 40px;
    margin: 0; overflow: visible; clip-path: none;
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--accent);
    outline: 2px solid var(--accent); outline-offset: -2px;
  }
  .kbd-only.del:focus-visible { color: var(--error); }
  /* Vira botao (abre a trilha no tap): tamanho explicito pra sobrescrever o min 44px global do
     <button>, que esticaria a row. 32x40 continua acima do minimo de alvo (SC 2.5.8, 24x24). */
  .chev {
    width: 32px; height: 40px; min-width: 32px; min-height: 40px;
    flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    padding: 0;
    color: var(--text-muted);
    font-size: var(--text-lg);
    line-height: 1;
    border-radius: var(--radius-sm);
  }
  .chev:active { color: var(--text-secondary); background: var(--bg-hover); }
</style>
