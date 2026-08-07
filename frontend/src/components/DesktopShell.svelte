<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from './Sidebar.svelte';
  import WorkspaceCommandPalette from './WorkspaceCommandPalette.svelte';
  import WorkspaceAttentionStrip from './WorkspaceAttentionStrip.svelte';
  import TerminalPanel from './TerminalPanel.svelte';
  import Chat from '../screens/Chat.svelte';
  import Board from '../screens/Board.svelte';
  import Canvas from '../screens/Canvas.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { getConfig } from '../lib/api';
  import { getActiveId, selectServer } from '../lib/auth';
  import type { AggSession } from '../lib/types';
  import {
    aggregateWorkspaceActions,
    resolveWorkspaceChatTarget,
    workspaceSessionKey,
    type WorkspaceAction,
    type WorkspaceSessionRef,
    type WorkspaceView,
  } from '../lib/workspaceCommands';

  // Shell de DESKTOP (>=820px): sidebar fixa + chat largo. Reusa o componente Chat do mobile
  // sem alteracao; abaixo de 820px o App nem monta isto (fica o fluxo mobile intacto).
  interface Props {
    currentSession: string | null;
    // Key de remontagem servidor-aware ("<serverId>::<nome>"): homônimas em servidores diferentes
    // têm o MESMO nome — sem o servidor na key, trocar entre elas não remontava o Chat (SSE preso
    // no servidor antigo com o composer já falando com o novo).
    currentKey?: string | null;
    view: WorkspaceView;   // quadro/canvas = visualizações irmãs da lista+chat, mesma sidebar
    // Overlay do quadro/canvas: vem da ROTA (#/board|#/canvas/<serverId>/<nome>), não é estado daqui.
    // O shell não aponta nem restaura servidor — quem faz isso é o $effect da rota no App, num lugar só.
    overlaySession: { name: string; serverId: string } | null;
    onOpenBoardSession: (name: string, serverId: string) => void;
    onOpenCanvasSession: (name: string, serverId: string) => void;
    onCloseOverlay: () => void;
    onToggleBoard: () => void;
    onToggleCanvas: () => void;
    onNavigateToChat: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
  }
  let {
    currentSession, currentKey = null, view, overlaySession,
    onOpenBoardSession, onOpenCanvasSession, onCloseOverlay, onToggleBoard, onToggleCanvas,
    onNavigateToChat, onCompare, onLogout,
  }: Props = $props();

  let commandOpen = $state(false);
  let lastSession = $state<WorkspaceSessionRef | null>(null);
  let sidebarActions = $state<WorkspaceAction[]>([]);
  let chatActions = $state<WorkspaceAction[]>([]);

  // Painel de terminal real (xterm.js), faixa no rodape do shell. Um so por vez: o shell nao tem
  // "pane focado" hoje, entao quem diz QUAL sessao e o proprio Chat que chamou (um dos tres mounts).
  let terminalOpen = $state(false);
  let terminalSession = $state('');
  // Chave SERVER-AWARE (igual currentKey/workspaceSessionKey): dois servidores podem ter uma sessao
  // homonima. So o nome nao bastava — trocar de servidor com o mesmo nome na tela nao reexecutava o
  // efeito do TerminalPanel, e o socket ficava falando pro servidor VELHO (termUrl usa o servidor
  // ATIVO no momento da conexao, e so a troca do servidor nao muda `sessionName`).
  let terminalKey = $state('');
  function abrirTerminal(nome: string, serverId: string) {
    terminalSession = nome;
    terminalKey = workspaceSessionKey({ serverId, name: nome });
    terminalOpen = true;
  }

  // Capacidade do painel (Task 6, Step 8): `pty` e POSIX-only, entao um servidor Windows na mistura
  // nao tem o painel -- sem o gate o botao abriria um painel morto. Le do SERVIDOR ATIVO (GET
  // /api/config). Default true (assume capaz) pra nao piscar pro espelho enquanto a resposta ainda
  // nao chegou.
  let terminalCapaz = $state(true);
  // `getActiveId()` NAO e reativo (le localStorage, mesma ressalva de `sessoesNaTela` acima) -- so
  // ha como o efeito SABER que precisa recalcular via `currentKey`/`overlaySession` (route-driven).
  // Mas a decisao de REFAZER O FETCH e pela identidade do SERVIDOR, nao da rota: sem este cache, o
  // toggle quadro<->canvas (que muda `overlaySession` sem trocar servidor) dispararia GET /api/config
  // de novo a toa a cada clique.
  let ultimoServidorConsultado: string | null = null;
  $effect(() => {
    void currentKey; void overlaySession;
    const sid = getActiveId();
    if (sid === ultimoServidorConsultado) return;
    ultimoServidorConsultado = sid;
    let vivo = true;
    getConfig()
      .then((c) => { if (vivo) terminalCapaz = c.somente_leitura.terminal_panel !== false; })
      // Falha de rede na config nao pode travar o botao: mantem o comportamento anterior (assume
      // capaz) e deixa o proprio clique do usuario revelar o erro real, se houver.
      .catch(() => { if (vivo) terminalCapaz = true; });
    return () => { vivo = false; };
  });
  const rows = $derived<AggSession[]>(sessionsStore.rows);
  const hasAttention = $derived(rows.some((row) => row.state === 'awaiting_input'));

  // O shell é o dono do chrome global (navegação/paleta/atenção), então também segura uma referência
  // ao store agregado. O singleton continua abrindo só 1 EventSource por servidor mesmo com
  // Sidebar/Board montados: retain/release é refcount, não cria streams por consumidor.
  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  $effect(() => {
    void currentKey;
    if (!currentSession) return;
    const serverId = getActiveId();
    if (serverId) lastSession = { serverId, name: currentSession };
  });

  function selectView(next: WorkspaceView) {
    if (next === view) {
      // Num chat aberto por cima do quadro/canvas, clicar na aba ativa volta à visualização atrás.
      if (overlaySession) onCloseOverlay();
      return;
    }
    if (next === 'chat') {
      const target = resolveWorkspaceChatTarget(lastSession, overlaySession);
      if (!target) {
        onNavigateToChat('');
        return;
      }
      selectServer(target.serverId);
      onNavigateToChat(target.name);
    } else if (next === 'board') {
      onToggleBoard();
    } else {
      onToggleCanvas();
    }
  }

  const navigationActions: WorkspaceAction[] = [
    {
      id: 'view:chat',
      title: 'Conversa',
      detail: 'Espaço principal de chat',
      keywords: ['chat', 'conversa'],
      group: 'Navegação',
      run: () => selectView('chat'),
    },
    {
      id: 'view:board',
      title: 'Quadro',
      detail: 'Sessões agrupadas por estado',
      keywords: ['board', 'quadro', 'kanban'],
      group: 'Navegação',
      run: () => selectView('board'),
    },
    {
      id: 'view:canvas',
      title: 'Canvas',
      detail: 'Organização livre das sessões',
      keywords: ['canvas', 'organização'],
      group: 'Navegação',
      run: () => selectView('canvas'),
    },
  ];
  const workspaceActions = $derived(
    aggregateWorkspaceActions([...navigationActions, ...sidebarActions, ...chatActions]),
  );

  function handleSidebarActionsChange(actions: WorkspaceAction[]) {
    sidebarActions = actions;
  }

  function handleChatActionsChange(actions: WorkspaceAction[]) {
    chatActions = actions;
  }

  function openSession(session: AggSession) {
    selectServer(session.serverId);
    onNavigateToChat(session.name);
  }

  function onShellKey(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
      e.preventDefault();
      commandOpen = true;
    }
  }

  // Split view (pareamento): N Chats lado a lado — assiste o GRUPO inteiro sem alternar.
  // Aberto pelo PairSheet (por membro ou "todas"); cada painel fecha no próprio ×; trocar a
  // sessão principal fecha tudo (o split é relativo a ela).
  let splitSessions = $state<string[]>([]);
  function openSplit(name: string) {
    if (name !== currentSession && !splitSessions.includes(name)) {
      splitSessions = [...splitSessions, name];
    }
  }
  $effect(() => {
    void (currentKey ?? currentSession);
    splitSessions = []; // trocou a principal (mesmo nome/outro servidor conta) -> fecha o split
  });

  // Chaves SERVER-AWARE das sessoes com um Chat montado agora (os mesmos tres mounts abaixo). Fecha
  // o terminal sozinho quando a chave dele sai da tela (navegou pro board/canvas, trocou a
  // principal, fechou o split, OU trocou de SERVIDOR com o mesmo nome na tela). Por CHAVE, nao por
  // nome: comparar so o nome deixava passar batido servidor A "api" -> servidor B "api" -- o Chat
  // remonta (currentKey muda), mas o nome exibido e o mesmo, entao `sessoesNaTela.includes(nome)`
  // continuava true e o painel ficava aberto, com o socket preso no servidor VELHO.
  // `void currentKey` e proposital: currentSession sozinho NAO muda de valor nessa troca (mesma
  // string "api" nos dois servidores), entao so ler currentKey aqui garante o recalculo na hora certa
  // -- getActiveId() em si nao e reativo.
  const sessoesNaTela = $derived.by(() => {
    if (view === 'board' || view === 'canvas') {
      return overlaySession ? [workspaceSessionKey(overlaySession)] : [];
    }
    void currentKey;
    if (!currentSession || currentSession === 'null' || currentSession === 'undefined') return [];
    const serverId = getActiveId() ?? '';
    return [currentSession, ...splitSessions].map((nome) => workspaceSessionKey({ serverId, name: nome }));
  });
  $effect(() => {
    if (terminalOpen && !sessoesNaTela.includes(terminalKey)) terminalOpen = false;
  });

  // Overlay do quadro: o Chat REAL (mesmo componente do resto do app) por cima do kanban, em vez de
  // navegar pra fora. O quadro fica montado atrás — volta intacto, com o mesmo scroll. Uma instância
  // por vez: o overlay cobre a .desktop-main inteira, então não dá pra clicar noutro card sem fechar.
  // Quem abre/fecha é a ROTA (#/board/<serverId>/<nome> vs #/board): não há estado local, nem
  // captura/restauração de servidor ativo, nem teardown a acertar — sair da rota já desfaz tudo.

  // Esc fecha — mas só quando o overlay é o dono do Esc. Todo sheet/espelho/preview aberto por dentro
  // já se fecha no próprio keydown de window (BottomSheet.svelte:116, Chat.svelte:739) e nenhum deles
  // para a propagação; sem esta guarda um único Esc fecharia o sheet E o overlay atrás dele.
  // CAPTURA (3o arg = true) de propósito: na fase de bubble o Svelte já teria feito o flush SÍNCRONO
  // do handler do sheet, e o dialog sumiria do DOM antes de eu poder vê-lo (verificado ao vivo — a
  // versão bubble fechava os dois de uma vez). Na captura nada reagiu ainda: o DOM ainda mostra quem
  // estava aberto ANTES da tecla. Por isso a checagem é no DOM e não em e.defaultPrevented — que só
  // pegaria os overlays que o Chat rastreia, e não os sheets abertos pelo Composer.
  $effect(() => {
    if (!overlaySession) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (document.querySelector('[role="dialog"]:not(.board-overlay)')) return;
      // CAPTURA na window roda ANTES do onkeydown (bolha) do TerminalPanel — sem esta guarda, Esc
      // digitado dentro do xterm (que deve chegar no agente, Task 5/I4) fechava o overlay do board
      // primeiro, ninguem via a tecla.
      if ((e.target as HTMLElement | null)?.closest('.tp')) return;
      onCloseOverlay();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });
</script>

<svelte:window onkeydown={onShellKey} />

<div class="desktop-shell">
  <Sidebar {currentSession} onSelect={onNavigateToChat} {onCompare} {onLogout}
           boardActive={view === 'board'}
           canvasActive={view === 'canvas'}
           onWorkspaceActionsChange={handleSidebarActionsChange}
           {view} onSelectView={selectView} onOpenCommand={() => (commandOpen = true)} />

  <div class="desktop-com-terminal">
  <main class="desktop-main" class:split={splitSessions.length > 0} class:has-attention={hasAttention}>
    {#if hasAttention}
      <div class="workspace-attention-layer">
        <WorkspaceAttentionStrip {rows} onOpenSession={openSession} />
      </div>
    {/if}

    {#if view === 'board' || view === 'canvas'}
      <div class="workspace-view">
        {#if view === 'board'}
          <Board onOpenSession={onOpenBoardSession} />
        {:else}
          <Canvas onOpenSession={onOpenCanvasSession} />
        {/if}
      </div>
      {#if overlaySession}
        <!-- Overlay do chat compartilhado entre board e canvas (mesma rota-overlay). {#key}: o Chat
             guarda estado pesado amarrado à sessão (SSE, histórico) e precisa remontar por sessão —
             mesma razão do {#key currentKey ?? currentSession} abaixo. Inclui o SERVIDOR pelo mesmo
             motivo do currentKey: homônimas em servidores diferentes têm o mesmo nome, e só o nome na
             key deixaria o Chat preso no servidor antigo. -->
        {#key workspaceSessionKey(overlaySession)}
          {@const overlayName = overlaySession.name}
          <div class="board-overlay" role="region" aria-label="Chat da sessão">
            <button class="split-close" onclick={onCloseOverlay}
                    aria-label="Fechar chat" title="Fechar (Esc)">×</button>
            <Chat
              sessionName={overlayName}
              desktop={true}
              onBack={onCloseOverlay}
              onNavigateToChat={onNavigateToChat}
              onOpenTerminalPanel={() => abrirTerminal(overlayName, overlaySession.serverId)}
              terminalPanelOpen={terminalOpen && terminalKey === workspaceSessionKey(overlaySession)}
              terminalPanelDisponivel={terminalCapaz}
              topInset={hasAttention ? 52 : 0}
              onOpenWorkspacePalette={() => (commandOpen = true)}
              showContextPanel={true}
              publishWorkspaceActions={true}
              onWorkspaceActionsChange={handleChatActionsChange}
            />
          </div>
        {/key}
      {/if}
    {:else if currentSession && currentSession !== 'null' && currentSession !== 'undefined'}
      {#key currentKey ?? currentSession}
        {@const cur = currentSession}
        <div class="pane">
          <Chat
            sessionName={cur}
            desktop={true}
            onBack={() => onNavigateToChat('')}
            onNavigateToChat={onNavigateToChat}
            onOpenSplit={openSplit}
            onOpenTerminalPanel={() => abrirTerminal(cur, getActiveId() ?? '')}
            terminalPanelOpen={terminalOpen && terminalKey === workspaceSessionKey({ serverId: getActiveId() ?? '', name: cur })}
            terminalPanelDisponivel={terminalCapaz}
            topInset={hasAttention ? 52 : 0}
            onOpenWorkspacePalette={() => (commandOpen = true)}
            showContextPanel={splitSessions.length === 0}
            publishWorkspaceActions={true}
            onWorkspaceActionsChange={handleChatActionsChange}
          />
        </div>
      {/key}
      {#each splitSessions as split (split)}
        <div class="pane pane--split">
          <button class="split-close" onclick={() => (splitSessions = splitSessions.filter((s) => s !== split))}
                  aria-label={`Fechar painel de ${split}`} title="Fechar painel">×</button>
          <Chat
            sessionName={split}
            desktop={true}
            onBack={() => (splitSessions = splitSessions.filter((s) => s !== split))}
            onNavigateToChat={onNavigateToChat}
            onOpenTerminalPanel={() => abrirTerminal(split, getActiveId() ?? '')}
            terminalPanelOpen={terminalOpen && terminalKey === workspaceSessionKey({ serverId: getActiveId() ?? '', name: split })}
            terminalPanelDisponivel={terminalCapaz}
            topInset={hasAttention ? 52 : 0}
            onOpenWorkspacePalette={() => (commandOpen = true)}
          />
        </div>
      {/each}
    {:else}
      <div class="desktop-empty">
        <p class="empty-title">Selecione uma sessão</p>
        <p class="empty-sub">ou crie uma nova na barra lateral</p>
      </div>
    {/if}
  </main>
  <TerminalPanel sessionName={terminalSession} connKey={terminalKey} open={terminalOpen}
                 onClose={() => (terminalOpen = false)} />
  </div>

  <WorkspaceCommandPalette
    open={commandOpen}
    {rows}
    {view}
    actions={workspaceActions}
    onClose={() => (commandOpen = false)}
    onOpenSession={openSession}
  />
</div>

<style>
  .desktop-shell {
    display: flex;
    height: 100vh;
    width: 100%;
    overflow: hidden;
  }
  /* Coluna AQUI, no wrapper — a .desktop-main segue exatamente como estava (regra abaixo intocada),
     entao .desktop-main.split continua "display: flex" e os dois chats do split seguem lado a lado.
     position:relative faz o terminal maximizado (.tp.max { position: absolute; inset: 0 }) cobrir
     so esta area, nao a Sidebar. */
  .desktop-com-terminal {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    position: relative;
  }
  .desktop-main {
    flex: 1;
    min-width: 0;
    height: 100%;
    position: relative;
    overflow: hidden;
  }
  .workspace-attention-layer {
    position: absolute;
    /* Subiu junto com a saida do seletor de view (que ocupava a faixa de 4-48px). */
    top: 8px;
    left: 0;
    right: 0;
    z-index: 39;
    display: flex;
    justify-content: center;
    pointer-events: none;
  }
  .workspace-view {
    height: 100%;
    box-sizing: border-box;
    /* Sem o seletor flutuante, so sobra a folga do topo; com a fila "Precisa de voce" no ar,
       o quadro/canvas ainda desce pra nao ficar por baixo dela. */
    padding-top: 8px;
  }
  .desktop-main.has-attention .workspace-view { padding-top: 60px; }
  /* Split: dois chats lado a lado, divisor sutil. Cada pane é um contexto próprio (NavBar/composer). */
  .desktop-main.split { display: flex; }
  .pane { height: 100%; position: relative; overflow: hidden; }
  .desktop-main.split .pane { flex: 1; min-width: 0; }
  .pane--split { border-left: 1px solid var(--border-default); }
  /* Overlay: cobre só a .desktop-main (a sidebar segue viva ao lado), com o quadro montado atrás
     preservando o scroll. Sem border-left — a sidebar já tem border-right, dobraria a linha.
     Fade SEM transform de propósito: os sheets do Chat são position:fixed (BottomSheet.svelte:159) e
     um transform aqui viraria containing block deles, clipando-os na pane (mesma regra do
     Chat.svelte:912). Só a opacidade anima. */
  .board-overlay {
    position: absolute; inset: 0; z-index: 30;
    background: var(--bg-base);
    animation: overlay-in 160ms var(--ease-out);
  }
  @keyframes overlay-in { from { opacity: 0; } to { opacity: 1; } }
  .split-close {
    /* right soma --cp-wco-right (app.css): no PWA em window-controls-overlay este × cairia
       exatamente embaixo do × da janela. Zero fora desse modo. */
    position: absolute; top: 8px; right: calc(10px + var(--cp-wco-right)); z-index: 20;
    width: 28px; height: 28px;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--bg-elevated); color: var(--text-secondary);
    font-size: 16px; line-height: 1; cursor: pointer;
  }
  .split-close:hover { color: var(--text-primary); background: var(--bg-hover); }
  .desktop-empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
  }
  .empty-title { font-size: var(--text-lg); color: var(--text-secondary); font-weight: 500; }
  .empty-sub { font-size: var(--text-sm); color: var(--text-muted); }
</style>
