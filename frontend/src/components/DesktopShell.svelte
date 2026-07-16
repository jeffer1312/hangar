<script lang="ts">
  import Sidebar from './Sidebar.svelte';
  import Chat from '../screens/Chat.svelte';

  // Shell de DESKTOP (>=820px): sidebar fixa + chat largo. Reusa o componente Chat do mobile
  // sem alteracao; abaixo de 820px o App nem monta isto (fica o fluxo mobile intacto).
  interface Props {
    currentSession: string | null;
    // Key de remontagem servidor-aware ("<serverId>::<nome>"): homônimas em servidores diferentes
    // têm o MESMO nome — sem o servidor na key, trocar entre elas não remontava o Chat (SSE preso
    // no servidor antigo com o composer já falando com o novo).
    currentKey?: string | null;
    onNavigateToChat: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
  }
  let { currentSession, currentKey = null, onNavigateToChat, onCompare, onLogout }: Props = $props();

  // Split view (pareamento): segundo Chat lado a lado — assiste a conversa das duas sessões sem
  // alternar. Aberto pelo PairSheet ("Abrir lado a lado"); fecha no X ou ao trocar a sessão principal.
  let splitSession = $state<string | null>(null);
  $effect(() => {
    void (currentKey ?? currentSession);
    splitSession = null; // trocou a principal (mesmo nome/outro servidor conta) -> fecha o split
  });
</script>

<div class="desktop-shell">
  <Sidebar {currentSession} onSelect={onNavigateToChat} {onCompare} {onLogout} />

  <main class="desktop-main" class:split={!!splitSession}>
    {#if currentSession && currentSession !== 'null' && currentSession !== 'undefined'}
      {#key currentKey ?? currentSession}
        <div class="pane">
          <Chat
            sessionName={currentSession}
            desktop={true}
            onBack={() => onNavigateToChat('')}
            onNavigateToChat={onNavigateToChat}
            onOpenSplit={(name) => (splitSession = name)}
          />
        </div>
      {/key}
      {#if splitSession}
        {#key splitSession}
          <div class="pane pane--split">
            <button class="split-close" onclick={() => (splitSession = null)}
                    aria-label="Fechar painel lado a lado" title="Fechar painel">×</button>
            <Chat
              sessionName={splitSession}
              desktop={true}
              onBack={() => (splitSession = null)}
              onNavigateToChat={onNavigateToChat}
            />
          </div>
        {/key}
      {/if}
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
