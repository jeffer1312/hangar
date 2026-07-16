<script lang="ts">
  import Sidebar from './Sidebar.svelte';
  import Chat from '../screens/Chat.svelte';
  import Board from '../screens/Board.svelte';
  import { selectServer, getActiveId } from '../lib/auth';

  // Shell de DESKTOP (>=820px): sidebar fixa + chat largo. Reusa o componente Chat do mobile
  // sem alteracao; abaixo de 820px o App nem monta isto (fica o fluxo mobile intacto).
  interface Props {
    currentSession: string | null;
    // Key de remontagem servidor-aware ("<serverId>::<nome>"): homônimas em servidores diferentes
    // têm o MESMO nome — sem o servidor na key, trocar entre elas não remontava o Chat (SSE preso
    // no servidor antigo com o composer já falando com o novo).
    currentKey?: string | null;
    view: 'chat' | 'board';   // quadro kanban = visualização irmã da lista+chat, mesma sidebar
    onToggleBoard: () => void;
    onNavigateToChat: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
  }
  let { currentSession, currentKey = null, view, onToggleBoard, onNavigateToChat, onCompare, onLogout }: Props = $props();

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
  let overlaySession = $state<{ name: string; serverId: string } | null>(null);
  // Servidor a restaurar ao fechar, ou null quando não há nada a desfazer. NÃO é $state: só o
  // fluxo de abrir/fechar lê, nunca o template.
  let prevActive: string | null = null;

  // selectServer muta estado GLOBAL (localStorage cp_active + cookie do token) e o Chat fala com o
  // servidor ATIVO via apiFetch — então abrir o card exige apontar o ativo pro dono dele. Sem
  // restaurar no fechamento, criar sessão / git / broadcast pela sidebar passariam a mirar o servidor
  // do último card aberto: bug silencioso. Mesmo par capture/restore do withServer (Sidebar.svelte:467).
  function openOverlay(name: string, serverId: string) {
    const prev = getActiveId();
    prevActive = prev !== serverId ? prev : null; // já era o ativo -> nada a restaurar
    selectServer(serverId);
    overlaySession = { name, serverId };
  }
  function closeOverlay() {
    if (prevActive) selectServer(prevActive);
    prevActive = null;
    overlaySession = null;
  }

  // Navegar pra fora a partir do overlay (ex.: "próxima aguardando" ou o switcher do Chat) PROMOVE a
  // sessão pro chat normal: o servidor do overlay vira o ativo de verdade e o restore acima seria um
  // bug — o chat que vai montar é dele. Por isso desarma o prevActive antes de sair.
  // Serve OS DOIS caminhos de saída por navegação (o nome só nomeia o primeiro): o de dentro do Chat
  // acima, e o clique na sidebar — que segue viva ao lado do overlay (ele é absolute dentro da
  // .desktop-main, ela é irmã dela) e já faz selectServer(dono) antes de delegar (Sidebar.svelte:287).
  // Sem isto o round-trip do hash acorda o $effect de view -> closeOverlay -> restore CLOBBERA esse
  // selectServer, e o chat de destino monta apontado pro servidor errado. Fora do overlay é
  // passthrough puro: prevActive/overlaySession já são null.
  function navigateFromOverlay(name: string) {
    prevActive = null;
    overlaySession = null;
    onNavigateToChat(name);
  }

  // Sair do quadro (toggle da sidebar) com o overlay aberto fecha ele junto — e restaura o servidor.
  // Espelha o effect do splitSessions acima. No-op quando não há overlay.
  // O teardown cobre o caso em que o DesktopShell DESMONTA em vez de trocar de view: #/costs, #/archive
  // e #/compare casam ANTES do branch isDesktop (App.svelte:242/244/249), e os três saem do kebab da
  // sidebar, que segue visível no rail com o quadro aberto. Sem isto o prevActive morre com o componente
  // e o ativo fica no servidor do card — o mesmo bug silencioso que o capture/restore existe pra evitar.
  // Idempotente: navigateFromOverlay já zerou o prevActive, então não reintroduz o clobber do 05d3434.
  $effect(() => {
    if (view !== 'board') closeOverlay();
    return closeOverlay; // registrado SEMPRE: com o quadro aberto é justamente quando o teardown importa
  });

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
      closeOverlay();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });
</script>

<div class="desktop-shell">
  <Sidebar {currentSession} onSelect={navigateFromOverlay} {onCompare} {onLogout}
           boardActive={view === 'board'} {onToggleBoard} />

  <main class="desktop-main" class:split={splitSessions.length > 0}>
    {#if view === 'board'}
      <Board onOpenSession={openOverlay} />
      {#if overlaySession}
        <!-- {#key}: o Chat guarda estado pesado amarrado à sessão (SSE, histórico) e precisa
             remontar por sessão — mesma razão do {#key currentKey ?? currentSession} abaixo. Inclui o
             SERVIDOR pelo mesmo motivo do currentKey: homônimas em servidores diferentes têm o mesmo
             nome, e só o nome na key deixaria o Chat preso no servidor antigo. -->
        {#key overlaySession.serverId + '::' + overlaySession.name}
          <div class="board-overlay" role="dialog" aria-label="Chat da sessão">
            <button class="split-close" onclick={closeOverlay}
                    aria-label="Fechar chat" title="Fechar (Esc)">×</button>
            <Chat
              sessionName={overlaySession.name}
              desktop={true}
              onBack={closeOverlay}
              onNavigateToChat={navigateFromOverlay}
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
