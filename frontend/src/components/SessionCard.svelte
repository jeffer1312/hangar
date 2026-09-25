<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { AggSession, SessionInfo } from '@hangar/core';
import * as m from '../paraglide/messages';
import { textoProblema } from '../lib/problema';
  import { cwdParts, rotuloEstado, stateColors, untrackedReason, providerTag, relativeTime, fmtWhen } from '@hangar/core';
  import { chipDaConta } from '../lib/conta';
  import { loopBadge, LOOP_TONE_COLOR } from '@hangar/core';
  import IconFolder from './icons/IconFolder.svelte';
  import IconWorktree from './icons/IconWorktree.svelte';
  import StateChip from './StateChip.svelte';
  import BottomSheet from './BottomSheet.svelte';
  import HangarWorking from './icons/HangarWorking.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import GroupGlyph from './icons/GroupGlyph.svelte';
  import SessionSignals from './SessionSignals.svelte';
  import { navegadorPanel } from '../lib/navegadorPanel.svelte';
  import { workspaceSessionKey } from '../lib/workspaceCommands';

  interface Props {
    session: SessionInfo;
    serverBadge?: { label: string; color: string } | null;
    onClick: () => void;
    onDelete: () => void;
    onResume?: () => void;
    onRename?: (newName: string) => void;
    onGit?: () => void;
    // Alça de arrastar-para-agrupar (Task 6): a mecânica de arrasto (fantasma, alvo sob o dedo,
    // auto-scroll) mora na LISTA — aqui só plumbing de pointer capture, repassando fase+evento.
    onGroupDrag?: (phase: 'down' | 'move' | 'up' | 'cancel', e: PointerEvent) => void;
    // Modo seleção do broadcast (feature #9): row vira checkbox (toque alterna); swipe/rename ficam
    // fora enquanto seleciona, pra não competir com o toque de marcar.
    selectMode?: boolean;
    selected?: boolean;
    onToggleSelect?: () => void;
    // Só quando a lista MISTURA agentes (mesma regra da Sidebar): com tudo em Claude a marca é a
    // mesma em toda linha e não separa nada — vira textura ao lado do nome.
    showProvider?: boolean;
  }
  let {
    session, serverBadge = null, onClick, onDelete, onResume, onRename, onGit, onGroupDrag,
    selectMode = false, selected = false, onToggleSelect, showProvider = false,
  }: Props = $props();


  const title = $derived(session.name);
  const pendingQuestions = $derived(session.pending_questions ?? 0);
  const serverId = $derived('serverId' in session ? (session as AggSession).serverId : '');
  const navKey = $derived(serverId ? workspaceSessionKey({ serverId, name: session.name }) : '');
  const browserOpen = $derived(!!navKey && navKey in navegadorPanel.abertos);

  const cwdPartes = $derived(cwdParts(session.cwd));

  // Celular e estreito: o cwd so entra quando ACRESCENTA algo. Quando o basename ja e o nome da
  // sessao ("hangar" + "/home/jeff…/hangar"), a linha inteira e redundante e so
  // roubava largura do nome/branch.
  const showCwd = $derived(!!session.cwd && cwdPartes.base.toLowerCase() !== session.name.toLowerCase());
  // Worktree é a EXCEÇÃO: a pasta aparece mesmo repetindo o nome da sessão, porque é ela que
  // carrega a marca de worktree — e é o nome dela que distingue duas cópias do mesmo repositório.
  const mostraPasta = $derived(showCwd || session.worktree === true);

  // Chip de estado so quando o estado PEDE atencao. "pronto" repetido em toda linha e ruido: o
  // ponto colorido do lead ja diz que esta parada.
  const showStateChip = $derived(session.state !== 'idle');

  // Sessao sem vinculo confiavel (claude manual sem --session-id): NAO da pra abrir o chat com
  // seguranca. Marca "sem id" e bloqueia o clique (delete continua valendo).
  const untracked = $derived(session.tracked === false);
  // EXCECAO kimi: "sem id" e o estado NORMAL pre-1o-prompt — a sessao (id + wire.jsonl) so nasce no
  // primeiro envio. Bloquear o clique fechava um ciclo sem saida: sem chat -> sem 1o prompt -> sem
  // id, pra sempre. O badge/hint continuam; so renomear e broadcast seguem bloqueados (precisam do
  // vinculo). O /input do backend nao depende de jsonl (vai por tmux), entao o chat abre e envia.
  // Codex entra pela MESMA razão, com uma diferença: lá o ciclo sem saída não é o 1º prompt, e sim
  // a TUI parada num seletor dela (aprovar hooks, escolher login). Sem abrir, a única saída era um
  // `tmux attach` na máquina — que no celular não existe.
  const abreChat = $derived(!untracked || session.provider === 'kimi' || session.provider === 'codex');

  // "Precisa de voce": aguardando input -> barra de acao + fundo tingido.
  const action = $derived(session.state === 'awaiting_input');

  // Travada (feature #7): "working" ha muito tempo sem avancar (watchdog do backend). Tinge o chip
  // de estado com um anel âmbar sutil — nao grita, so avisa.
  const stalled = $derived(session.stalled === true);
  // Código -> texto: o backend manda só o código (regra de i18n). Código desconhecido some em vez
  // de virar um id cru na tela.
  const problema = $derived(textoProblema(session.problema));

  // Radar de limite (feature #8): parada esperando o limite voltar. O state-chip vira "limite ·
  // volta HH:MM" e o ícone para de animar — a sessão está `working` pro hook, não pra pessoa.
  const limited = $derived(session.limited === true);
  const contaChip = $derived(chipDaConta(session.conta));

  const loopChip = $derived(loopBadge(session.loop_status, session.loop_iter, session.loop_max));
  // Provider da linha — só as não-Claude ganham chip (ver providerTag em lib/format).
  const provTag = $derived(providerTag(session.provider));
  // Tempo relativo da última atividade ("51 min atrás" — o "51m ago" do card do super.engineering).
  const agoLabel = $derived(session.last_reply ? '' : relativeTime(session.last_activity));
  const replyTime = $derived(relativeTime(session.last_reply_at));

  // ── Swipe-to-actions ───────────────────────────────────────────────────────
  // Arrasta a linha pra esquerda revelando Git / Loop / Excluir. touch-action:pan-y deixa o scroll
  // vertical pro navegador e o horizontal pra gente. Distingue tap / swipe-x / scroll-y por eixo
  // dominante. Git e Loop moraram na row-right ate aqui: 2 botoes de 40px + chip + chevron comiam
  // quase metade da largura e o cwd/nome viviam truncados no iPhone. Sao acoes raras -> swipe.
  const ACTION_W = 64;
  // Alça de arrastar-para-agrupar (Task 6) sempre entra na trilha, ao lado de Git (só com cwd) e
  // Excluir (sempre) — a largura de abertura tem que contar o botão a mais.
  const OPEN = $derived(-(session.cwd ? 3 : 2) * ACTION_W);
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

  // ── TOQUE LONGO (500ms parado, sem swipe) -> menu de ações da sessão. Renomear direto era
  // invisível: quem segurava esperando opções caía num campo de edição sem pedir. O swipe segue
  // existindo como atalho pras mesmas ações.
  let editing = $state(false);
  let editValue = $state('');
  let longPressed = $state(false);
  let menuOpen = $state(false);
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  function startPress() {
    longPressed = false;
    clearTimeout(pressTimer);
    pressTimer = undefined;
    pressTimer = setTimeout(() => {
      pressTimer = undefined;
      if (!dragging || axis !== null) return;
      longPressed = true;
      menuOpen = true;
    }, 500);
  }
  function menuPick(fn: () => void) {
    menuOpen = false;
    fn();
  }
  function startRename() {
    // Depois do restoreFocus do BottomSheet: no mesmo flush, a devolução de foco do sheet
    // roubava o autofocus do campo e o blur salvava/fechava a edição antes de ela aparecer.
    setTimeout(() => {
      editValue = session.name;
      editing = true;
    }, 0);
  }
  function cancelPress() {
    clearTimeout(pressTimer);
    pressTimer = undefined;
  }
  onDestroy(cancelPress);
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
    // pointercancel não gera click, então o reset de onRowClick nunca roda — a flag presa
    // engolia o toque seguinte no card, calada.
    longPressed = false;
  }

  // Alça de arrastar-para-agrupar: pointer capture pra continuar recebendo move/up mesmo quando o
  // dedo sai da area do botao (padrao ja usado no onDown da linha, so aqui o botao captura a si
  // mesmo). Sem `dragging`/eixo pra decidir — a alca so serve pra isso, entao TODO movimento e drag.
  function onHandleDown(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    onGroupDrag?.('down', e);
  }
  function onHandleMove(e: PointerEvent) {
    onGroupDrag?.('move', e);
  }
  function onHandleUp(e: PointerEvent) {
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    onGroupDrag?.('up', e);
    offset = 0;   // fecha a trilha do swipe (mesmo gesto do botão Git) — sem isto ficava aberta pós-drop
  }
  function onHandleCancel(e: PointerEvent) {
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    onGroupDrag?.('cancel', e);
    offset = 0;
  }

  // Tap na linha: toque longo (renomeou) nao navega; se aberto ou acabou de arrastar, fecha o swipe.
  function onRowClick() {
    if (selectMode) { if (!untracked) onToggleSelect?.(); return; }  // sem id -> nao entra no broadcast
    if (longPressed) { longPressed = false; return; }   // foi toque longo (menu) -> nao abre o chat
    if (suppressClick || offset !== 0) { offset = 0; return; }
    if (abreChat) onClick();
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
    <!-- Alça de arrastar pra agrupar (Task 6): único gesto livre é a partir DAQUI — a linha já usa
         o ponteiro pro swipe horizontal, a rolagem vertical e o toque longo. -->
    <button
      class="act grip"
      onpointerdown={onHandleDown}
      onpointermove={onHandleMove}
      onpointerup={onHandleUp}
      onpointercancel={onHandleCancel}
      aria-label={m.grupo_arrastar_aria({ nome: session.name })}
      title={m.grupo_arrastar_alca()}
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <circle cx="9" cy="6" r="1.6"/><circle cx="9" cy="12" r="1.6"/><circle cx="9" cy="18" r="1.6"/>
        <circle cx="15" cy="6" r="1.6"/><circle cx="15" cy="12" r="1.6"/><circle cx="15" cy="18" r="1.6"/>
      </svg>
    </button>
    {#if session.cwd}
      <button class="act git" onclick={() => { offset = 0; onGit?.(); }} aria-label={m.sessao_aria_git({ n: session.name })}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <line x1="6" y1="3" x2="6" y2="15"/>
          <circle cx="18" cy="6" r="3"/>
          <circle cx="6" cy="18" r="3"/>
          <path d="M18 9a9 9 0 0 1-9 9"/>
        </svg>
        <span>{m.sessao_git()}</span>
      </button>
    {/if}
    <button class="act del" onclick={onDelete} aria-label={m.sessao_aria_excluir_sessao({ n: session.name })}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <polyline points="3 6 5 6 21 6"/>
        <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
        <path d="M10 11v6M14 11v6"/>
      </svg>
      <span>{m.sessao_excluir_curto()}</span>
    </button>
  </div>

  <div
    class="session-row"
    class:action
    class:untracked
    class:untracked-open={untracked && abreChat}
    class:dragging
    style="transform: translateX({offset}px);"
    role="button"
    tabindex="0"
    aria-disabled={!abreChat}
    aria-pressed={selectMode ? selected : undefined}
    onclick={onRowClick}
    onkeydown={(e) => {
      if (e.key !== 'Enter' && e.key !== ' ') return;
      if (!abreChat) return;
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
      {:else if session.state === 'working' && !limited}
        <!-- Working -> a marca desenhando, na COR DO ESTADO. O Lottie antigo trazia a cor dentro
             do arquivo; este herda via currentColor, então quem pinta é o container — sem isto ele
             sai na cor do texto do card, que é apagada. -->
        <span class="work-mark" style="color: {stateColors[session.state]};"><HangarWorking size={20} /></span>
      {:else}
        <!-- Parada -> ponto na COR do estado. O icone parado era igual em todos os estados: numa
             lista sem nada animando as linhas ficavam indistinguiveis. -->
        <span class="state-dot" style="background: {limited ? 'var(--pill-limite-fg)' : stateColors[session.state]};"></span>
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
            aria-label={m.sessao_novo_nome()}
          />
        {:else}
          <span class="session-name">{title}</span>
          <SessionSignals browser={browserOpen} headless={session.headless === true} />
          <!-- Marca do agente junto dos outros sinais do nome, não numa fila de chips própria: cada
               agente tem marca colorida, o nome ao lado repetia o desenho e segue no title. -->
          {#if showProvider}
            <span class="prov-chip prov-chip--so-icone" title={`${m.sessao_grupo()} ${provTag ?? 'Claude'}`}><span class="sr-only">{m.sessao_grupo()}&nbsp;{provTag ?? 'Claude'}</span><ProviderGlyph provider={session.provider} size={12} /></span>
          {/if}
          {#if pendingQuestions > 0}
            <span class="untracked-badge pending-questions" title={`${m.ask_perguntas()}: ${pendingQuestions}`} aria-label={`${m.ask_perguntas()}: ${pendingQuestions}`}>? {pendingQuestions}</span>
          {/if}
        {/if}
        {#if untracked}
          <span class="untracked-badge" title={untrackedReason(session.provider)}>⚠ {m.sessao_sem_id()}</span>
        {/if}
        <!-- Conta no FIM DA LINHA DO NOME (paridade com a Sidebar): aqui sobra largura, e ela
             deixa de ocupar um lugar na fila de chips, que é a linha que enche primeiro. -->
        {#if contaChip}
          <span class="conta-chip conta-no-nome" style="--conta-cor: {contaChip.cor};" title={m.sessao_conta({ n: contaChip.nome })}><span class="conta-label">{contaChip.label}</span></span>
        {/if}
      </span>
      {#if (session.state === 'awaiting_input' || pendingQuestions > 0) && session.question}
        <span class="status-sub asking" title={session.question}>{session.question}</span>
      {:else if session.state === 'working' && session.label}
        <span class="status-sub working" title={session.label}>{session.label}</span>
      {:else if session.state === 'idle' && session.last_reply}
        <span class="status-sub reply" title={session.last_reply}>
          <span class="reply-mark" aria-hidden="true">◆</span>
          <span class="reply-text">{session.last_reply}</span>
          {#if replyTime}<span class="reply-time">{replyTime}</span>{/if}
        </span>
      {/if}
      <!-- UMA meta-line só (antes eram duas: cwd e branch). Ordem = importancia: a branch e o que
           muda, entao vem primeiro e nunca some; o cwd fecha a linha e trunca primeiro. O tempo
           relativo ("51 min atrás") vem por último, colado à direita — informação de contexto, não
           de identidade. -->
      {#if serverBadge || session.branch || mostraPasta || agoLabel
           || session.git_ahead || session.git_behind || session.git_added || session.git_removed}
        <span class="meta-line">
          {#if serverBadge}
            <span class="srv" style="color: {serverBadge.color};">{serverBadge.label}</span>
            {#if session.branch || mostraPasta}<span class="meta-sep">·</span>{/if}
          {/if}
          {#if session.branch}
            <span class="branch" title={m.sessao_branch_git_atual()}>⎇ {session.branch}</span>
            {#if mostraPasta}<span class="meta-sep">·</span>{/if}
          {/if}
          <!-- Diff do working tree (referência: cards do super.engineering, "+128 −24" ao lado da
               branch). IRMÃO da branch, não filho: HEAD destacado (sem branch) ainda tem diff. -->
          <!-- ↑ falta enviar, ↓ falta trazer — mesmas setas e cores do painel Git (GitColuna).
               Zero não desenha: a ausência de seta É "está em dia". -->
          {#if session.git_ahead || session.git_behind}
            <span class="sync" title={m.git_sync_titulo({ ahead: session.git_ahead ?? 0, behind: session.git_behind ?? 0 })}>
              <!-- sr-only pela mesma razão da pasta: o `title` de um span não é lido de forma
                   confiável, e a seta sozinha não diz o que significa. -->
              <span class="sr-only">{m.git_sync_titulo({ ahead: session.git_ahead ?? 0, behind: session.git_behind ?? 0 })}</span>
              {#if session.git_ahead}<span class="sync-ah" aria-hidden="true">↑{session.git_ahead}</span>{/if}{#if session.git_behind}<span class="sync-be" aria-hidden="true">↓{session.git_behind}</span>{/if}
            </span>
          {/if}
          {#if session.git_added || session.git_removed}
            <span class="diff-stats" aria-hidden="true">{#if session.git_added}<span class="diff-add">+{session.git_added}</span>{/if}{#if session.git_removed}<span class="diff-del">−{session.git_removed}</span>{/if}</span>
          {/if}
          {#if mostraPasta}
            <!-- Só a última pasta, com ícone no lugar do prefixo (mesma razão da Sidebar: o
                 prefixo truncava o nome que identifica). Caminho inteiro no title. -->
            <!-- sr-only com o caminho inteiro: mesma razão da Sidebar, onde está o comentário. -->
            <!-- Worktree troca o ÍCONE da pasta em vez de uma pílula escrita "worktree": a worktree
                 é a pasta, e é o nome dela que distingue duas cópias do mesmo repo na mesma branch. -->
            <span class="cwd" class:cwd--worktree={session.worktree} title={session.worktree ? `${m.sessao_worktree()}: ${session.cwd}` : session.cwd}><span class="sr-only">{session.worktree ? `${m.sessao_worktree()}: ${session.cwd}` : session.cwd}</span><span class="cwd-icone" aria-hidden="true">{#if session.worktree}<IconWorktree size={11} />{:else}<IconFolder size={11} />{/if}</span><span class="cwd-base" aria-hidden="true">{cwdPartes.base}</span></span>
          {/if}
          {#if agoLabel}
            {#if serverBadge || session.branch || mostraPasta}<span class="meta-sep" aria-hidden="true">·</span>{/if}
            <span class="ago" title={fmtWhen(session.last_activity)}>{agoLabel}</span>
          {/if}
        </span>
      {/if}
      <!-- 🤝 grupo, ⏳ rate-limit, 🔁 loop e ⚙ motor, no fluxo da coluna de texto (na row-right
           esmagavam o nome — visto no iPhone). A linha só existe quando tem chip: a marca do agente
           subiu pra linha do nome e a conta também, e sem elas sobrava uma linha inteira vazia. -->
      {#if session.pair_peers?.length || loopChip || session.engine}
      <span class="badges-line">
          {#if session.pair_peers?.length}
            <span class="paired-chip" title={m.sessao_grupo_com({ n: session.pair_peers.join(', ') })}><GroupGlyph size={12} />&nbsp;{session.pair_peers.length === 1 ? session.pair_peers[0] : session.pair_peers.length + 1}</span>
          {/if}
          {#if loopChip}
            <span
              class="paired-chip"
              style="color: {LOOP_TONE_COLOR[loopChip.tone]}; background: color-mix(in srgb, {LOOP_TONE_COLOR[loopChip.tone]} 14%, transparent);"
              title={m.sessao_loop_runner()}
            >{loopChip.label}</span>
          {/if}
          {#if session.engine}
            <!-- Sem isto nada na lista distingue uma sessão de motor de uma da conta Anthropic. NÃO
                 mostramos custo aqui: o preço que o Claude Code calcula é tabela Anthropic e mentiria. -->
            <span class="engine-chip" title={m.sessao_motor({ n: session.engine })}>⚙&nbsp;{session.engine}</span>
          {/if}
        </span>
      {/if}
      <!-- Retomar e Claude-only de ponta a ponta (candidatos de ~/.claude/projects + relance com
           `claude --resume`): numa sessao Pi/Kimi/OMP o botao so poderia errar, entao mostramos a
           razao no lugar dele. O backend recusa igual, pra um cliente velho nao matar o pane. -->
      {#if untracked && (session.provider === 'pi' || session.provider === 'kimi' || session.provider === 'omp' || session.provider === 'codex')}
        <span class="untracked-hint">{untrackedReason(session.provider)}</span>
      {:else if untracked}
        <button
          class="resume-btn"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); onResume?.(); }}
        >↻ {m.sessao_retomar()}</button>
      {/if}
    </div>

    <div class="row-right">
      <!-- Chip so quando o estado pede atencao (idle = ponto colorido no lead, sem pilula "pronto"
           repetida em toda linha). -->
      {#if problema}
        <!-- Problema desta sessão (hoje: sessão Codex trabalhando sem hook aprovado, que sem isto
             apareceria "pronta" para sempre). Mesma família visual do travada: âmbar, calmo. -->
        <span class="state-chip stalled" title={problema}>
          <StateChip state={session.state} title={problema} />
        </span>
      {:else if showStateChip}
        <!-- O envelope .state-chip existe pelo anel de travada e pela regra de papel de parede do
             app.css que ja mirava essa classe; a pilula em si e o StateChip. -->
        <span class="state-chip" class:stalled={stalled && !limited}>
          <StateChip state={session.state} {limited} limitReset={session.limit_reset} title={limited ? undefined : stalled ? m.sessao_travada() : undefined} />
        </span>
      {:else}
        <!-- Sem chip, o estado so existia como COR (o .lead e aria-hidden) — leitor de tela ficava
             sem saber que a sessao esta pronta. Texto so pra AT (SC 1.4.1). -->
        <span class="sr-only">{rotuloEstado(session.state)}</span>
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
          aria-label={m.sessao_aria_git({ n: session.name })}
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="6" y1="3" x2="6" y2="15"/>
            <circle cx="18" cy="6" r="3"/>
            <circle cx="6" cy="18" r="3"/>
            <path d="M18 9a9 9 0 0 1-9 9"/>
          </svg>
        </button>
      {/if}
      <button
        class="kbd-only del"
        inert={offset === OPEN}
        onpointerdown={(e) => e.stopPropagation()}
        onclick={(e) => { e.stopPropagation(); onDelete(); }}
        aria-label={m.sessao_aria_excluir_sessao({ n: session.name })}
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
        aria-label={m.sessao_aria_acoes({ n: session.name })}
        aria-expanded={offset === OPEN}
      >›</button>
    </div>
  </div>
</div>

<!-- Menu de ações por toque longo: as mesmas ações do swipe (+ renomear), agora descobríveis.
     Fechar cai na mesma confirmação do onDelete de sempre. -->
<BottomSheet open={menuOpen} onClose={() => (menuOpen = false)} ariaLabel={m.sessao_aria_acoes({ n: session.name })}>
  <div class="card-menu">
    <h2 class="card-menu-title">{session.name}</h2>
    {#if !untracked}
      <button class="card-menu-item" onclick={() => menuPick(startRename)}>
        <span class="card-menu-ico" aria-hidden="true">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/>
          </svg>
        </span>
        <span class="card-menu-label">{m.ctx_renomear()}</span>
      </button>
    {/if}
    {#if session.cwd}
      <button class="card-menu-item" onclick={() => menuPick(() => onGit?.())}>
        <span class="card-menu-ico" aria-hidden="true">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="6" y1="3" x2="6" y2="15"/>
            <circle cx="18" cy="6" r="3"/>
            <circle cx="6" cy="18" r="3"/>
            <path d="M18 9a9 9 0 0 1-9 9"/>
          </svg>
        </span>
        <span class="card-menu-label">{m.sessao_git()}</span>
      </button>
    {/if}
    <button class="card-menu-item danger" onclick={() => menuPick(onDelete)}>
      <span class="card-menu-ico" aria-hidden="true">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="3 6 5 6 21 6"/>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
          <path d="M10 11v6M14 11v6"/>
        </svg>
      </span>
      <span class="card-menu-label">{m.sessao_excluir_curto()}</span>
    </button>
  </div>
</BottomSheet>

<style>
  /* Wrapper do swipe: esconde o "Excluir" que fica atras da linha. */
  .swipe-wrap {
    position: relative;
    overflow: hidden;
    border-bottom: 1px solid var(--border-subtle);
  }

  /* Trilha de acoes revelada pelo swipe: Git | Excluir. Loop runner fica só no menu ⋯ do card. */
  .swipe-actions {
    position: absolute;
    right: 0;
    top: 0;
    bottom: 0;
    display: flex;
  }
  /* opacity, nao display:none: o layout da trilha (2 botoes de 64px) precisa continuar medido pro
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
  .swipe-actions .del { background: var(--error); color: #fff; }
  /* Alça de arrastar (Task 6): touch-action none pra o navegador não brigar com o drag custom
     (senão o gesto vira scroll/zoom nativo no meio do arrasto). Sem legenda embaixo (só ícone) —
     "Arrastar para agrupar" não cabe nos 64px sem quebrar feio; o texto vive no aria-label/title. */
  .swipe-actions .grip {
    background: var(--bg-elevated);
    color: var(--text-muted);
    touch-action: none;
    -webkit-touch-callout: none;
    user-select: none;
  }

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
    /* O toque longo aqui abre o menu de acoes; no iOS ele tambem inicia selecao de texto, e o
       realce azul + as alcas ficam por cima da folha. */
    -webkit-touch-callout: none;
    user-select: none;
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
  /* Kimi "sem id" (pre-1o-prompt) ABRE o chat: o cursor nao pode mentir que a linha e inerte. */
  .session-row.untracked.untracked-open {
    cursor: pointer;
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
  .work-mark { display: inline-flex; }
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
    align-items: flex-start;
    gap: var(--space-2);
    min-width: 0;
  }
  .session-name {
    flex: 0 1 auto;
    min-width: min-content;
    max-width: 100%;
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    line-height: 1.25;
    overflow-wrap: anywhere;
  }
  /* O nome é a identidade da sessão: a conta cede quase toda a largura antes de ele quebrar. */
  .conta-no-nome { margin-left: auto; flex: 1 100 auto; min-width: 0; max-width: min(32%, 92px); }
  /* Input do rename inline (toque longo). Mesmo visual do .server-edit da lista de servidores. */
  .name-edit {
    flex: 1;
    min-width: 0;
    user-select: text;   /* o none da .session-row impediria selecionar no campo de renomear */
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
    /* Os itens curtos da linha (setas, diff, tempo) não encolhem; sem isto, quando a soma deles
       passa da largura, eles vazam POR CIMA do vizinho em vez de serem cortados — o tempo
       aparecia escrito sobre o nome da pasta. */
    overflow: hidden;
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
  .status-sub.reply { display: flex; align-items: center; gap: 4px; color: var(--text-secondary); }
  .reply-mark { flex-shrink: 0; color: var(--text-muted); font-size: 8px; }
  .reply-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .reply-time { flex-shrink: 0; margin-left: auto; color: var(--text-muted); font-size: 10px; }
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
  .cwd-icone {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    margin-right: 3px;
    color: var(--text-muted);
  }
  .cwd-base {
    /* encolhe COM ellipsis: "flex: 0 0 auto" nunca encolhia e o basename vazava por baixo
       da row-right (overlap visto no iPhone). O prefixo continua encolhendo primeiro. */
    flex: 0 1 auto;
    /* min-width ZERO, e não um piso: com piso a pasta ou vira "p.." (um caractere e reticências,
       que não identificam nada) ou para de encolher e passa por baixo do tempo. Cedendo até o fim
       sobra só o ícone — que é o marcador — e o caminho segue no title. */
    min-width: 0;
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
  /* Worktree: a pasta inteira muda de cor junto com o ícone. Tingir só o ícone de 11px não se
     lia; é o NOME da pasta que diz qual das cópias é esta. */
  .cwd--worktree,
  .cwd--worktree .cwd-icone { color: var(--pill-working-fg); }
  /* "+128 −24" do working tree: mono como a branch ao lado, nas cores semânticas de sempre
     (verde/vermelho do diff, não accent — é dado de código, não identidade). Não trunca: são 2
     números curtos e é informação que muda; quem cede é o cwd, como sempre. */
  .diff-stats {
    flex-shrink: 0;
    display: inline-flex;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .diff-add { color: var(--success); }
  .diff-del { color: var(--error); }
  /* ↑/↓ do upstream: mesmas cores do painel Git, e negrito porque é o único item da meta-line que
     pede ação — o resto dela descreve onde a sessão está. */
  .sync {
    flex-shrink: 0;
    display: inline-flex;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .sync-ah { color: var(--accent); }
  .sync-be { color: var(--warning); }
  /* Tempo relativo no fim da meta-line: mutado de propósito, não disputa com nome/branch. */
  .ago { flex-shrink: 0; color: var(--text-muted); white-space: nowrap; }

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
  /* Envelope da pilula (o desenho dela vive no StateChip.svelte). */
  .state-chip { display: inline-flex; flex-shrink: 0; border-radius: var(--radius-full); }
  /* Travada (feature #7): anel âmbar sutil no chip — avisa sem gritar. Outline, e nao box-shadow
     inset: a sombra do envelope ficaria por baixo do fundo da pilula. */
  .state-chip.stalled {
    outline: 1px solid var(--warning); outline-offset: -1px;
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
    /* O glifo colorido do provider entrou na frente do texto: sem flex ele quebrava a linha de base do chip. */
    display: inline-flex; align-items: center; gap: 4px;
  }
  /* Claude (sem texto, só a marca): chip só-ícone fica redondo e menor que os chips com texto. */
  .prov-chip--so-icone { padding: 1px 3px; }

  /* Conta da sessão (Anthropic ou Codex): chip neutro com um ponto na cor da conta — mesma receita
     da Sidebar, onde está o porquê. */
  .conta-chip {
    display: inline-flex; align-items: center; gap: 4px;
    font-size: 10px; font-weight: var(--fw-medium); letter-spacing: 0.02em;
    padding: 1px 6px; border-radius: var(--radius-full);
    background: var(--fill-subtle); color: var(--text-secondary); min-width: 0;
  }
  .conta-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .conta-chip::before {
    content: ''; flex: 0 0 auto;
    width: 5px; height: 5px; border-radius: 50%;
    background: var(--conta-cor, currentColor);
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

  /* Menu do toque longo: mesma família visual do MoreSheet (item de 56px, ícone em caixa). */
  .card-menu {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-4) var(--space-5);
  }
  .card-menu-title {
    margin: 0 0 var(--space-2);
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-menu-item {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: var(--space-3);
    width: 100%;
    min-height: 52px;
    padding: var(--space-2);
    background: transparent;
    border-radius: var(--radius-md);
    text-align: left;
    -webkit-tap-highlight-color: transparent;
  }
  .card-menu-item:active { background: var(--bg-hover); }
  .card-menu-ico {
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    color: var(--text-secondary);
  }
  .card-menu-label { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .card-menu-item.danger .card-menu-ico { color: var(--error); background: rgba(255,69,58,0.12); }
  .card-menu-item.danger .card-menu-label { color: var(--error); }
</style>
