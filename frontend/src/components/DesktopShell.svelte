<script lang="ts">
  import Sidebar from './Sidebar.svelte';
  import Chat from '../screens/Chat.svelte';
  import Board from '../screens/Board.svelte';

  // Shell de DESKTOP (>=820px): sidebar fixa + chat largo. Reusa o componente Chat do mobile
  // sem alteracao; abaixo de 820px o App nem monta isto (fica o fluxo mobile intacto).
  interface Props {
    currentSession: string | null;
    // Key de remontagem servidor-aware ("<serverId>::<nome>"): homônimas em servidores diferentes
    // têm o MESMO nome — sem o servidor na key, trocar entre elas não remontava o Chat (SSE preso
    // no servidor antigo com o composer já falando com o novo).
    currentKey?: string | null;
    view: 'chat' | 'board';   // quadro kanban = visualização irmã da lista+chat, mesma sidebar
    // Overlay do quadro: vem da ROTA (#/board/<serverId>/<nome>), não é estado daqui. O shell não
    // aponta nem restaura servidor — quem faz isso é o $effect da rota no App, num lugar só.
    overlaySession: { name: string; serverId: string } | null;
    onOpenBoardSession: (name: string, serverId: string) => void;
    onCloseOverlay: () => void;
    onToggleBoard: () => void;
    onNavigateToChat: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
  }
  let {
    currentSession, currentKey = null, view, overlaySession,
    onOpenBoardSession, onCloseOverlay, onToggleBoard, onNavigateToChat, onCompare, onLogout,
  }: Props = $props();

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
      onCloseOverlay();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });
</script>

<div class="desktop-shell">
  <Sidebar {currentSession} onSelect={onNavigateToChat} {onCompare} {onLogout}
           boardActive={view === 'board'} {onToggleBoard} />

  <main class="desktop-main" class:split={splitSessions.length > 0}>
    {#if view === 'board'}
      <Board onOpenSession={onOpenBoardSession} />
      {#if overlaySession}
        <!-- {#key}: o Chat guarda estado pesado amarrado à sessão (SSE, histórico) e precisa
             remontar por sessão — mesma razão do {#key currentKey ?? currentSession} abaixo. Inclui o
             SERVIDOR pelo mesmo motivo do currentKey: homônimas em servidores diferentes têm o mesmo
             nome, e só o nome na key deixaria o Chat preso no servidor antigo. -->
        {#key overlaySession.serverId + '::' + overlaySession.name}
          <div class="board-overlay" role="dialog" aria-label="Chat da sessão">
            <button class="split-close" onclick={onCloseOverlay}
                    aria-label="Fechar chat" title="Fechar (Esc)">×</button>
            <Chat
              sessionName={overlaySession.name}
              desktop={true}
              onBack={onCloseOverlay}
              onNavigateToChat={onNavigateToChat}
            />
          </div>
        {/key}
      {/if}
    {:else if currentSession && currentSession !== 'null' && currentSession !== 'undefined'}
      {#key currentKey ?? currentSession}
        <div class="pane">
          <Chat
            sessionName={currentSession}
            desktop={true}
            onBack={() => onNavigateToChat('')}
            onNavigateToChat={onNavigateToChat}
            onOpenSplit={openSplit}
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
</div>

<style>
  .desktop-shell {
    display: flex;
    height: 100vh;
    width: 100%;
    overflow: hidden;
  }
  .desktop-main {
    flex: 1;
    min-width: 0;
    height: 100%;
    position: relative;
    overflow: hidden;
  }
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
    position: absolute; top: 8px; right: 10px; z-index: 20;
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
