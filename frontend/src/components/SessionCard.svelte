<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import type { AggSession, SessionInfo } from '@hangar/core';
import * as m from '../paraglide/messages';
import { textoProblema } from '../lib/problema';
  import { cwdParts, rotuloEstado, stateColors, untrackedReason, providerName, relativeTime, fmtWhen, isOrq, worktreeLabel } from '@hangar/core';
  import { getPushSettings, setSessionMute, getBranches, checkoutBranch, gitAction, openEditor, setThenLink, clearThenLink, canPair, canLeave } from '@hangar/core';
  import { worktreeStatus } from '../lib/worktreeStatus.svelte';
  import { listOwnServers, withServer } from '../lib/auth';
  import { copyText } from '../lib/clipboard';
  import { arrastarGrupo } from '../lib/arrastarGrupo.svelte';
  import { dragChave } from '../lib/dragToGroup';
  import WorktreeSheet from './WorktreeSheet.svelte';
  import { chipDaConta } from '../lib/conta';
  import { loopBadge, LOOP_TONE_COLOR } from '@hangar/core';
  import IconFolder from './icons/IconFolder.svelte';
  import IconWorktree from './icons/IconWorktree.svelte';
  import BottomSheet from './BottomSheet.svelte';
  import HangarWorking from './icons/HangarWorking.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
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
    // Pasta na linha de meta: o nativo só a mostra na lista por servidor (por projeto o
    // cabeçalho já a diz); worktree aparece sempre.
    showFolder?: boolean;
    // Sessões do mesmo servidor (encadear e agrupar pelo toque longo); lidas só ao abrir o subnível.
    peers?: () => AggSession[];
    onShare?: () => void;
    onBastao?: () => void;
    onOpenArbiter?: () => void;
  }
  let {
    session, serverBadge = null, onClick, onDelete, onResume, onRename, onGit, onGroupDrag,
    selectMode = false, selected = false, onToggleSelect,
    showFolder = false, peers, onShare, onBastao, onOpenArbiter,
  }: Props = $props();


  const title = $derived(session.name);
  const pendingQuestions = $derived(session.pending_questions ?? 0);
  const serverId = $derived('serverId' in session ? (session as AggSession).serverId : '');
  const navKey = $derived(serverId ? workspaceSessionKey({ serverId, name: session.name }) : '');
  const browserOpen = $derived(!!navKey && navKey in navegadorPanel.abertos);

  const cwdPartes = $derived(cwdParts(session.cwd));
  const wtPath = $derived(session.worktree_path ?? (session.worktree ? session.cwd : null));
  const wtNome = $derived(worktreeLabel(session));
  const wtJuntada = $derived(wtPath ? worktreeStatus.get(serverId, wtPath)?.merged === true : false);
  let wtAberta = $state(false);
  // A sessão só traz o id do servidor; a janela precisa do objeto para chamar a API. Convite fica
  // de fora: o backend recusa worktrees ao convidado, e o chip volta a ser só texto.
  const wtServer = $derived(wtPath && serverId ? listOwnServers().find((s) => s.id === serverId) ?? null : null);
  // Worktree é a EXCEÇÃO: a pasta aparece mesmo repetindo o nome da sessão, porque é ela que
  // carrega a marca de worktree — e é o nome dela que distingue duas cópias do mesmo repositório.
  const mostraPasta = $derived(session.worktree === true || session.worktree_gone === true || !!wtNome || (showFolder && !!session.cwd));
  // main/master não distingue nada na lista: só a branch de trabalho aparece.
  const branchShown = $derived(session.branch && session.branch !== 'main' && session.branch !== 'master' ? session.branch : null);
  // Sessão de motor roda outro modelo pelo Claude Code: o rótulo do provider vira o motor.
  const engineLabel = $derived(session.engine && session.provider === 'claude' ? session.engine : null);
  const asking = $derived(session.state === 'awaiting_input' || pendingQuestions > 0);
  const workLabel = $derived(session.label ? session.label.split(' (')[0] : '');

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

  // "Precisa de voce": aguardando input -> fundo tingido.
  const action = $derived(session.state === 'awaiting_input');

  // Travada (feature #7): "working" ha muito tempo sem avancar (watchdog do backend). O rotulo de
  // trabalho fica âmbar — nao grita, so avisa.
  const stalled = $derived(session.stalled === true);
  // Código -> texto: o backend manda só o código (regra de i18n). Código desconhecido some em vez
  // de virar um id cru na tela.
  const problema = $derived(textoProblema(session.problema));

  // Radar de limite (feature #8): parada esperando o limite voltar. A segunda linha vira "limite ·
  // volta HH:MM" e a marca para de animar — a sessão está `working` pro hook, não pra pessoa.
  const limited = $derived(session.limited === true);
  const contaChip = $derived(chipDaConta(session.conta));

  const loopChip = $derived(loopBadge(session.loop_status, session.loop_iter, session.loop_max));
  // Orquestrador sem LLM: sem renomear e sem excluir; o backend recusa os dois.
  const orq = $derived(isOrq(session));
  // Tempo só da última resposta de uma sessão parada, como no nativo: em trabalho ele mente.
  const replyTime = $derived(session.state === 'idle' ? relativeTime(session.last_reply_at) : '');

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

  // ── Itens do toque longo, na ordem do menu da sessão do nativo. Cada ação mostra a falha na
  // própria folha: no celular não há toast nem console à vista.
  const invite = $derived((session as AggSession).serverInvite === true);
  // Sessão da outra pessoa num par: o servidor dela recusa tudo que não é ler.
  const readOnly = $derived((session as SessionInfo & { guest_kind?: string | null }).guest_kind === 'pair');
  const hasGit = $derived(!!session.cwd && session.branch != null);
  const leave = $derived(!!serverId && canLeave(session as AggSession));
  type MenuView = 'main' | 'branch' | 'chain' | 'group';
  let view = $state<MenuView>('main');
  let note = $state('');
  let noteError = $state(false);
  let busy = $state(false);
  let muted = $state<boolean | null>(null);
  let muteError = $state('');
  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
  function say(text: string, error = false) { note = text; noteError = error; }

  // Silenciar depende da leitura do servidor: sem ela o item não grava.
  // untrack: o SSE reemite a sessão a cada poucos segundos e não pode voltar a folha ao início.
  $effect(() => {
    if (!menuOpen) return;
    untrack(() => {
      view = 'main';
      note = '';
      muted = null;
      muteError = '';
      if (invite || readOnly || !serverId || orq) return;
      const srv = serverId, name = session.name;
      withServer(srv, () => getPushSettings())
        .then((p) => { muted = p.muted.includes(name); })
        .catch((e) => { muteError = errMsg(e); });
    });
  });

  async function act(fn: () => Promise<string | null>) {
    busy = true;
    say('');
    try {
      const done = await fn();
      if (done === null) menuOpen = false; else say(done);
    } catch (e) {
      say(errMsg(e), true);
    } finally {
      busy = false;
    }
  }
  function toggleMute() {
    const next = !muted;
    void act(async () => { await withServer(serverId, () => setSessionMute(session.name, next)); muted = next; return null; });
  }
  async function copyCwd() {
    if (await copyText(session.cwd ?? '')) menuOpen = false;
    else say(m.toast_copiar_falhou(), true);
  }
  function doOpenEditor() {
    void act(async () => { await withServer(serverId, () => openEditor(session.name)); return null; });
  }
  function doGitPull() {
    void act(async () => {
      say(m.ctx_flash_git_pull());
      const r = await withServer(serverId, () => gitAction(session.name, 'pull'));
      const line = r.output.trim().split('\n')[0];
      if (!r.ok) throw new Error(m.ctx_flash_git_pull_erro({ n: line }));
      return line || m.ctx_flash_pull_ok();
    });
  }

  let branches = $state<{ list: string[]; current: string | null; dirty: boolean } | null>(null);
  let dirtyPick = $state<string | null>(null);
  function openBranches() {
    view = 'branch';
    branches = null;
    dirtyPick = null;
    void act(async () => {
      const info = await withServer(serverId, () => getBranches(session.name));
      branches = { list: info.branches, current: info.current, dirty: info.dirty ?? false };
      return '';
    });
  }
  function pickBranch(b: string, stash = false) {
    if (!branches || b === branches.current) return;
    if (branches.dirty && !stash) { dirtyPick = b; return; }
    dirtyPick = null;
    void act(async () => {
      await withServer(serverId, async () => {
        if (stash) {
          const r = await gitAction(session.name, 'stash');
          if (!r.ok) throw new Error(r.output || m.sessao_stash_falhou());
        }
        await checkoutBranch(session.name, b);
      });
      return null;
    });
  }

  // Lida ao abrir o subnível: congelada enquanto ele está aberto, a lista não pula sob o dedo.
  let peerList = $state<AggSession[]>([]);
  function readPeers() {
    peerList = (peers?.() ?? []).filter((s) => s.serverId === serverId && s.name !== session.name);
  }
  let chainTarget = $state<string | null>(null);
  let chainText = $state('');
  function openChain() {
    readPeers();
    view = 'chain';
    say('');
    chainTarget = session.then_target ?? null;
    chainText = '';
  }
  function openGroup() {
    readPeers();
    view = 'group';
    say('');
  }
  function saveChain() {
    const target = chainTarget, text = chainText.trim();
    if (!target || !text) return;
    void act(async () => { await withServer(serverId, () => setThenLink(session.name, target, text)); return null; });
  }
  function removeChain() {
    void act(async () => { await withServer(serverId, () => clearThenLink(session.name)); return null; });
  }
  // Mesmo diálogo do arrastar: quem não arrasta escolhe o alvo aqui.
  function groupWith(alvo: AggSession) {
    menuOpen = false;
    arrastarGrupo.comecar({ serverId, name: session.name });
    arrastarGrupo.soltar(dragChave(alvo));
  }

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
    {#if !orq}
      <button class="act del" onclick={onDelete} aria-label={m.sessao_aria_excluir_sessao({ n: session.name })}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <polyline points="3 6 5 6 21 6"/>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
          <path d="M10 11v6M14 11v6"/>
        </svg>
        <span>{m.sessao_excluir_curto()}</span>
      </button>
    {/if}
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
        <!-- Working -> a marca desenhando, na COR DO ESTADO. Este herda via currentColor, então
             quem pinta é o container. -->
        <span class="work-mark" style="color: {stateColors[session.state]};"><HangarWorking size={18} /></span>
      {:else}
        <!-- Parada -> ponto na COR do estado: é ele, e não uma pílula, que diz o estado na lista. -->
        <span class="state-dot" style="background: {limited ? 'var(--pill-limite-fg)' : stateColors[session.state]};"></span>
      {/if}
    </span>

    <!-- Três linhas, na ordem do nativo: nome com selos, conta e tempo; o que a sessão disse ou
         pergunta; e a meta (agente, pasta, branch, sincronia, diff). -->
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
          <SessionSignals browser={browserOpen} headless={session.headless === true}
                          shared={session.shared === true} guest={(session as AggSession).serverInvite === true} />
          {#if orq}
            <span class="untracked-badge orq-badge">{m.orq_row_badge()}</span>
          {/if}
          {#if session.owner}
            <span class="owner-badge" title={m.sessao_do_convidado({ n: session.owner })}>👤&nbsp;{session.owner}</span>
          {/if}
          {#if pendingQuestions > 0}
            <span class="untracked-badge pending-questions" title={`${m.ask_perguntas()}: ${pendingQuestions}`} aria-label={`${m.ask_perguntas()}: ${pendingQuestions}`}>? {pendingQuestions}</span>
          {/if}
          {#if untracked}
            <span class="untracked-badge" title={untrackedReason(session.provider)}>⚠ {m.sessao_sem_id()}</span>
          {/if}
        {/if}
        {#if contaChip || replyTime}
          <span class="name-end">
            {#if contaChip}
              <span class="conta-chip conta-no-nome" style="--conta-cor: {contaChip.cor};" title={m.sessao_conta({ n: contaChip.nome })}><span class="conta-label">{contaChip.label}</span></span>
            {/if}
            {#if replyTime}
              <span class="ago" title={fmtWhen(session.last_reply_at)}>{replyTime}</span>
            {/if}
          </span>
        {/if}
      </span>
      {#if asking && session.question}
        <span class="status-sub asking" title={session.question}>{session.question}</span>
      {:else if problema}
        <!-- Problema desta sessão (ex.: Codex trabalhando sem hook aprovado, que sem isto
             apareceria "pronta" para sempre). -->
        <span class="status-sub asking" title={problema}>{problema}</span>
      {:else if limited}
        <span class="status-sub asking">{session.limit_reset ? m.estado_limite_volta({ n: session.limit_reset }) : m.estado_limite()}</span>
      {:else if session.state === 'working' && workLabel}
        <span class="status-sub working" class:stalled title={stalled ? m.sessao_travada() : session.label}>{workLabel}</span>
      {:else if session.state === 'idle' && session.last_reply}
        <span class="status-sub reply" title={session.last_reply}>
          <span class="reply-mark" aria-hidden="true">◆</span>
          <span class="reply-text">{session.last_reply}</span>
        </span>
      {/if}
      {#if !orq || serverBadge || branchShown || mostraPasta || loopChip
           || session.git_ahead || session.git_behind || session.git_added || session.git_removed}
        <span class="meta-line">
          {#if !orq}
            {#if engineLabel}
              <span class="prov-chip prov-engine" title={m.sessao_motor({ n: engineLabel })}>⚙&nbsp;{engineLabel}</span>
            {:else}
              <span class="prov-chip"><ProviderGlyph provider={session.provider} size={12} />{providerName(session.provider)}</span>
            {/if}
          {/if}
          {#if serverBadge}
            <span class="srv" style="color: {serverBadge.color};">{serverBadge.label}</span>
          {/if}
          {#if mostraPasta}
            <!-- Só a última pasta, com ícone no lugar do prefixo; caminho inteiro no title e no
                 sr-only. Worktree troca o ÍCONE da pasta em vez de uma pílula escrita "worktree". -->
            {#if session.worktree_gone}
              <span class="cwd cwd--gone" title={session.worktree_path ?? ''}><span class="cwd-icone" aria-hidden="true"><IconWorktree size={11} /></span><span class="cwd-base">{m.worktree_apagada()}</span></span>
            {:else if wtNome && wtPath && wtServer}
              <!-- keydown também para aqui: o Enter subiria até a linha e abriria o chat. -->
              <button type="button" class="cwd cwd--worktree cwd--botao" title={`${m.sessao_worktree()}: ${wtPath}`}
                onpointerdown={(e) => e.stopPropagation()}
                onkeydown={(e) => e.stopPropagation()}
                onclick={(e) => { e.stopPropagation(); wtAberta = true; }}>
                <span class="cwd-icone" aria-hidden="true"><IconWorktree size={11} /></span><span class="cwd-base">{wtNome}{wtJuntada ? ' ✓' : ''}</span>
              </button>
            {:else}
            <span class="cwd" class:cwd--worktree={session.worktree} title={session.worktree ? `${m.sessao_worktree()}: ${session.cwd}` : session.cwd}><span class="sr-only">{session.worktree ? `${m.sessao_worktree()}: ${session.cwd}` : session.cwd}</span><span class="cwd-icone" aria-hidden="true">{#if session.worktree}<IconWorktree size={11} />{:else}<IconFolder size={11} />{/if}</span><span class="cwd-base" aria-hidden="true">{cwdPartes.base}</span></span>
            {/if}
          {/if}
          {#if branchShown}
            <span class="branch" title={m.sessao_branch_git_atual()}>⎇ {branchShown}</span>
          {/if}
          <!-- ↑ falta enviar, ↓ falta trazer — mesmas setas e cores do painel Git. Zero não
               desenha: a ausência de seta É "está em dia". -->
          {#if session.git_ahead || session.git_behind}
            <span class="sync" title={m.git_sync_titulo({ ahead: session.git_ahead ?? 0, behind: session.git_behind ?? 0 })}>
              <span class="sr-only">{m.git_sync_titulo({ ahead: session.git_ahead ?? 0, behind: session.git_behind ?? 0 })}</span>
              {#if session.git_ahead}<span class="sync-ah" aria-hidden="true">↑{session.git_ahead}</span>{/if}{#if session.git_behind}<span class="sync-be" aria-hidden="true">↓{session.git_behind}</span>{/if}
            </span>
          {/if}
          {#if session.git_added || session.git_removed}
            <span class="diff-stats" aria-hidden="true">{#if session.git_added}<span class="diff-add">+{session.git_added}</span>{/if}{#if session.git_removed}<span class="diff-del">−{session.git_removed}</span>{/if}</span>
          {/if}
          {#if loopChip}
            <span class="loop-mark" style="color: {LOOP_TONE_COLOR[loopChip.tone]};" title={m.sessao_loop_runner()}>{loopChip.label}</span>
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
      <!-- O estado na lista é só a cor da marca (o .lead é aria-hidden): texto para leitor de tela
           (SC 1.4.1). -->
      <span class="sr-only">{rotuloEstado(session.state)}</span>
      <!-- Caminho por TECLADO/leitor de tela pras acoes do swipe (git / excluir), que e
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
      {#if !orq}
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
      {/if}
      <!-- "›" abre a conversa, como o toque na linha; as ações moram no toque longo, que é a
           alternativa sem arrasto ao swipe (WCAG 2.2 SC 2.5.7). -->
      <button
        class="chev"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={(e) => { e.stopPropagation(); if (selectMode) { onRowClick(); return; } offset = 0; if (abreChat) onClick(); }}
        aria-label={m.sessao_aria_abrir({ n: session.name })}
        aria-disabled={!abreChat}
      >›</button>
    </div>
  </div>
</div>

{#snippet ico(d: string)}
  <span class="card-menu-ico" aria-hidden="true">
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path {d}/></svg>
  </span>
{/snippet}

{#snippet item(label: string, d: string, onclick: () => void, opts: { value?: string | null; more?: boolean; disabled?: boolean; danger?: boolean })}
  <button class="card-menu-item" class:danger={opts.danger} disabled={opts.disabled || busy} {onclick}>
    {@render ico(d)}
    <span class="card-menu-label">{label}</span>
    {#if opts.value}<span class="card-menu-value">{opts.value}</span>{/if}
    {#if opts.more}<span class="card-menu-more" aria-hidden="true">›</span>{/if}
  </button>
{/snippet}

<!-- Toque longo: o menu da sessão do nativo, por grupos. Encadear, trocar branch e agrupar abrem
     um subnível na mesma folha. Fechar cai na mesma confirmação do onDelete de sempre. -->
<BottomSheet open={menuOpen} onClose={() => (menuOpen = false)} ariaLabel={m.sessao_aria_acoes({ n: session.name })}>
  <div class="card-menu">
    {#if view === 'main'}
      <h2 class="card-menu-title">{session.name}</h2>
      {#if orq}
        <!-- O orquestrador não tem nome, processo nem turno próprios: o menu dele só leva ao árbitro. -->
        <div class="card-menu-group">
          {@render item(m.orq_talk_to_arbiter(), 'M12 3v4M12 11a3 3 0 1 0 0-.1M6 21v-3a6 6 0 0 1 12 0v3', () => menuPick(() => onOpenArbiter?.()), { disabled: !onOpenArbiter })}
        </div>
      {:else if readOnly}
        {#if session.cwd}
          <div class="card-menu-group">
            {@render item(m.ctx_copiar_cwd(), 'M9 9h11v11H9zM5 15H4V4h11v1', copyCwd, {})}
          </div>
        {/if}
      {:else}
        <div class="card-menu-group">
          {#if !untracked}
            {@render item(m.ctx_renomear(), 'M4 20h4L19 9l-4-4L4 16v4z', () => menuPick(startRename), {})}
          {/if}
          {#if !invite}
            {@render item(muteError ? m.ctx_flash_silenciar({ n: muteError }) : muted ? m.ctx_reativar_notif() : m.ctx_silenciar_notif(),
              'M6 16V11a6 6 0 0 1 12 0v5l2 2H4l2-2zM10 21h4', toggleMute, { disabled: muted === null })}
            {#if onShare}
              {@render item(m.compartilhar_menu(), 'M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1', () => menuPick(() => onShare?.()), {})}
            {/if}
          {/if}
        </div>
        {#if session.cwd}
          <div class="card-menu-group">
            {@render item(m.ctx_copiar_cwd(), 'M9 9h11v11H9zM5 15H4V4h11v1', copyCwd, {})}
            {#if !invite}
              {@render item(m.ctx_abrir_editor(), 'M8 6l-6 6 6 6M16 6l6 6-6 6', doOpenEditor, {})}
            {/if}
          </div>
        {/if}
        {#if hasGit}
          <div class="card-menu-group">
            {@render item(m.sessao_git(), 'M6 4v16M6 12h6a6 6 0 0 0 6-6', () => menuPick(() => onGit?.()), { more: true })}
            {@render item(m.ctx_git_pull(), 'M12 4v12M7 11l5 5 5-5M5 20h14', doGitPull, {})}
            {@render item(m.ctx_trocar_branch(), 'M6 4v10a4 4 0 0 0 4 4h8M15 15l3 3-3 3', openBranches, { value: session.branch, more: true })}
          </div>
        {/if}
        {#if !invite}
          <div class="card-menu-group">
            {@render item(m.sessao_chain_enviar(), 'M5 12h12M13 7l5 5-5 5', openChain, { value: session.then_target, more: true })}
            {#if serverId}
              {@render item(m.sessao_agrupar_com(), 'M8 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM16 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8z', openGroup, { more: true })}
            {/if}
            {#if leave}
              {@render item(m.sessao_sair_grupo(), 'M15 4h4v16h-4M10 8l-4 4 4 4M6 12h10', () => menuPick(() => arrastarGrupo.pedirSaida({ serverId, name: session.name })), {})}
            {/if}
            {#if onBastao}
              {@render item(m.bastao_menu(), 'M3 12h10M10 8l4 4-4 4M19 4v16', () => menuPick(() => onBastao?.()), { more: true })}
            {/if}
          </div>
        {/if}
        <div class="card-menu-group">
          {@render item(invite ? m.convite_parar() : m.sessao_excluir_curto(), 'M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13', () => menuPick(onDelete), { danger: true })}
        </div>
      {/if}
    {:else}
      <div class="card-menu-head">
        <button class="card-menu-back" onclick={() => { view = 'main'; say(''); }} aria-label={m.comum_voltar()}>‹</button>
        <h2 class="card-menu-title">
          {view === 'branch' ? m.ctx_trocar_branch()
            : view === 'group' ? m.sessao_agrupar_com()
            : session.then_target ? m.sessao_chain_encadeado({ n: session.then_target }) : m.sessao_chain_enviar()}
        </h2>
      </div>
      {#if view === 'branch'}
        {#if !branches}
          {#if busy}<p class="card-menu-info" role="status">{m.comum_carregando()}</p>{/if}
        {:else if dirtyPick}
          <p class="card-menu-info">{m.sessao_trocar_branch_sujo()}</p>
          <button class="card-menu-cta" disabled={busy} onclick={() => dirtyPick && pickBranch(dirtyPick, true)}>{m.sessao_guardar_trocar()}</button>
        {:else if branches.list.length === 0}
          <p class="card-menu-info">{m.ctx_sem_branches()}</p>
        {:else}
          <div class="card-menu-list">
            {#each branches.list as b (b)}
              <button class="card-menu-pick" class:current={b === branches.current} disabled={busy} onclick={() => pickBranch(b)}>
                <span class="card-menu-pick-name">{b}</span>
                {#if b === branches.current}<span class="card-menu-check" aria-hidden="true">✓</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
      {:else if view === 'chain'}
        {#if peerList.length === 0}
          <p class="card-menu-info">{m.ctx_nenhuma_outra()}</p>
        {:else}
          <div class="card-menu-list" role="radiogroup" aria-label={m.sessao_chain_enviar()}>
            {#each peerList as p (p.name)}
              <button class="card-menu-pick" class:current={p.name === chainTarget} role="radio" aria-checked={p.name === chainTarget}
                      onclick={() => (chainTarget = p.name)}>
                <span class="card-menu-dot" style="background: {stateColors[p.state]};" aria-hidden="true"></span>
                <span class="card-menu-pick-text">
                  <span class="card-menu-pick-name">{p.name}</span>
                  <span class="card-menu-pick-sub">{rotuloEstado(p.state)}{#if p.branch} · ⎇ {p.branch}{/if}</span>
                </span>
                {#if p.name === chainTarget}<span class="card-menu-check" aria-hidden="true">✓</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
        <input
          type="text"
          class="card-menu-input"
          bind:value={chainText}
          placeholder={m.ctx_prompt_enviar()}
          aria-label={m.ctx_prompt_alvo()}
          onkeydown={(e) => { if (e.key === 'Enter') saveChain(); }}
        />
        <div class="card-menu-actions">
          {#if session.then_target}
            <button class="card-menu-cta secondary danger" disabled={busy} onclick={removeChain}>{m.ctx_remover_vinculo()}</button>
          {/if}
          <button class="card-menu-cta" disabled={busy || !chainTarget || !chainText.trim()} onclick={saveChain}>{m.ctx_salvar()}</button>
        </div>
      {:else}
        {@const alvos = peerList.filter((p) => canPair(session as AggSession, p).ok)}
        {#if alvos.length === 0}
          <p class="card-menu-info">{m.ctx_nenhuma_outra()}</p>
        {:else}
          <div class="card-menu-list">
            {#each alvos as p (p.name)}
              <button class="card-menu-pick" onclick={() => groupWith(p)}>
                <span class="card-menu-dot" style="background: {stateColors[p.state]};" aria-hidden="true"></span>
                <span class="card-menu-pick-text">
                  <span class="card-menu-pick-name">{p.name}</span>
                  <span class="card-menu-pick-sub">{rotuloEstado(p.state)}{#if p.branch} · ⎇ {p.branch}{/if}</span>
                </span>
              </button>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}
    {#if note}<p class="card-menu-note" class:error={noteError} role="status">{note}</p>{/if}
  </div>
</BottomSheet>
{#if wtPath && wtServer}<WorktreeSheet open={wtAberta} server={wtServer} path={wtPath} onClose={() => (wtAberta = false)} />{/if}

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
  .swipe-actions .git { background: var(--surface-raised); color: var(--text-secondary); }
  .swipe-actions .del { background: var(--error); color: #fff; }
  /* Alça de arrastar (Task 6): touch-action none pra o navegador não brigar com o drag custom
     (senão o gesto vira scroll/zoom nativo no meio do arrasto). Sem legenda embaixo (só ícone) —
     "Arrastar para agrupar" não cabe nos 64px sem quebrar feio; o texto vive no aria-label/title. */
  .swipe-actions .grip {
    background: var(--surface-raised);
    color: var(--text-muted);
    touch-action: none;
    -webkit-touch-callout: none;
    user-select: none;
  }

  .session-row {
    position: relative;
    display: flex;
    /* A marca acompanha a linha do nome; só a coluna da direita fica centrada. */
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
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

  /* "Precisa de voce": fundo levemente tingido. Sem papel de parede a tinta fica OPACA (camada
     sobre o --bg-base): translucida deixava o "Excluir" vermelho atras vazar no swipe-to-delete.
     Com fundo do app (papel de parede, textura) a linha ja e transparente, entao a tinta tambem. */
  .session-row.action {
    background: linear-gradient(color-mix(in srgb, var(--warning) 6%, transparent), color-mix(in srgb, var(--warning) 6%, transparent)), var(--bg-base);
  }
  :global(html[data-bg]) .session-row.action {
    background: color-mix(in srgb, var(--warning) 6%, transparent);
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
    height: 20px;
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
    align-items: center;
    gap: 6px;
    min-width: 0;
    min-height: 20px;
  }
  /* Uma linha só, como no nativo: o nome corta antes de empurrar o cartão para quatro linhas. */
  .session-name {
    flex: 0 1 auto;
    min-width: 0;
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name-end { margin-left: auto; flex: 0 1 auto; min-width: 0; display: inline-flex; align-items: center; gap: 6px; }
  .conta-no-nome { flex: 0 1 auto; min-width: 0; max-width: 96px; }
  .owner-badge { flex-shrink: 0; font-size: 10px; color: var(--text-muted); white-space: nowrap; }
  /* Input do rename inline (toque longo). Mesmo visual do .server-edit da lista de servidores. */
  .name-edit {
    flex: 1;
    min-width: 0;
    user-select: text;   /* o none da .session-row impediria selecionar no campo de renomear */
    height: 32px;
    background: var(--surface-inset);
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
    gap: 6px;
    min-width: 0;
    color: var(--text-muted);
    /* Os itens curtos da linha (setas, diff, tempo) não encolhem; sem isto, quando a soma deles
       passa da largura, eles vazam POR CIMA do vizinho em vez de serem cortados — o tempo
       aparecia escrito sobre o nome da pasta. */
    overflow: hidden;
    font-size: 11.5px;
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
  .status-sub.working.stalled { color: var(--warning); }
  .status-sub.reply { display: flex; align-items: center; gap: 4px; color: var(--text-secondary); }
  .reply-mark { flex-shrink: 0; color: var(--text-muted); font-size: 8px; }
  .reply-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .srv {
    font-weight: 600;
    flex-shrink: 0;
    max-width: 90px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
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
  /* Sem o alvo de toque global de 44px: o chip mora dentro da linha de meta, que tem uma linha só. */
  .cwd--botao { background: none; border: 0; padding: 0; font: inherit; font-family: var(--font-mono); cursor: pointer; min-height: 0; min-width: 0; }
  .cwd--gone, .cwd--gone .cwd-icone { color: var(--text-muted); }
  /* "+128 −24" do working tree: mono como a branch ao lado, nas cores semânticas de sempre
     (verde/vermelho do diff, não accent — é dado de código, não identidade). Não trunca: são 2
     números curtos e é informação que muda; quem cede é o cwd, como sempre. */
  .diff-stats {
    flex-shrink: 0;
    display: inline-flex;
    gap: 4px;
    font-family: var(--font-mono);
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
    font-weight: 600;
  }
  .sync-ah { color: var(--accent); }
  .sync-be { color: var(--warning); }
  /* Tempo da última resposta no fim da linha do nome: mutado, não disputa com o nome. */
  .ago { flex-shrink: 0; color: var(--text-muted); font-size: 10px; white-space: nowrap; }
  .loop-mark { flex-shrink: 0; font-weight: 600; white-space: nowrap; }

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
  .untracked-badge.orq-badge { color: var(--text-muted); border-color: var(--border-subtle); }
  .row-right {
    display: flex;
    align-items: center;
    align-self: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  /* O nome do harness abre a meta, para não sumir quando a pasta é longa. Texto, não pílula. */
  .prov-chip {
    font-weight: 600;
    color: var(--text-secondary);
    white-space: nowrap; flex-shrink: 0;
    /* O glifo colorido do provider vem na frente do texto: sem flex ele quebrava a linha de base. */
    display: inline-flex; align-items: center; gap: 4px;
  }
  .prov-engine { color: var(--accent); }

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

  /* Menu do toque longo: mesma família visual do MoreSheet (item de 52px, ícone em caixa). A
     BottomSheet já dá a margem lateral. */
  .card-menu {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) 0 var(--space-5);
  }
  .card-menu-group { display: flex; flex-direction: column; gap: 2px; }
  .card-menu-group + .card-menu-group { border-top: 1px solid var(--border-subtle); padding-top: var(--space-1); margin-top: var(--space-1); }
  .card-menu-head { display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-2); }
  .card-menu-head .card-menu-title { margin: 0; }
  .card-menu-back {
    width: 44px; height: 44px; flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    background: transparent; color: var(--text-secondary); font-size: var(--text-lg);
    border-radius: var(--radius-sm);
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
  .card-menu-item:disabled { opacity: 0.45; }
  .card-menu-ico {
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    background: var(--surface-raised);
    color: var(--text-secondary);
  }
  .card-menu-label { flex: 1; min-width: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .card-menu-value {
    flex: 0 1 auto; min-width: 0; max-width: 45%;
    font-size: var(--text-sm); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .card-menu-more { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; }
  .card-menu-item.danger .card-menu-ico { color: var(--error); background: color-mix(in srgb, var(--error) 12%, transparent); }
  .card-menu-item.danger .card-menu-label { color: var(--error); }
  .card-menu-info { margin: var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .card-menu-note { margin: var(--space-2) var(--space-2) 0; font-size: var(--text-sm); color: var(--text-secondary); overflow-wrap: anywhere; }
  .card-menu-note.error { color: var(--error); }
  .card-menu-list { display: flex; flex-direction: column; gap: 2px; max-height: 50vh; overflow-y: auto; -webkit-overflow-scrolling: touch; }
  .card-menu-pick {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; min-height: 52px; padding: var(--space-2) var(--space-3);
    background: transparent; border-radius: var(--radius-md); text-align: left;
    -webkit-tap-highlight-color: transparent;
  }
  .card-menu-pick:active { background: var(--bg-hover); }
  .card-menu-pick.current { background: var(--accent-dim); }
  .card-menu-dot { width: 8px; height: 8px; flex-shrink: 0; border-radius: 50%; }
  .card-menu-pick-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .card-menu-pick-name {
    flex: 1; min-width: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .card-menu-pick-sub { font-size: var(--text-xs); color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .card-menu-check { flex-shrink: 0; color: var(--accent); font-weight: 700; }
  .card-menu-input {
    width: 100%; box-sizing: border-box; height: 44px; margin-top: var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-primary); font-family: var(--font-ui); font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3); outline: none;
  }
  .card-menu-input:focus { border-color: var(--accent); }
  .card-menu-actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: var(--space-3); }
  .card-menu-cta {
    min-height: 44px; padding: 0 var(--space-4); border-radius: var(--radius-md);
    background: var(--accent); color: #fff; font-size: var(--text-sm); font-weight: 600;
  }
  .card-menu-cta:disabled { opacity: 0.5; }
  .card-menu-cta.secondary { background: transparent; border: 1px solid var(--border-default); color: var(--text-primary); }
  .card-menu-cta.danger { color: var(--error); }
</style>
