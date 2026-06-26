<script lang="ts">
  import Sidebar from './Sidebar.svelte';
  import Chat from '../screens/Chat.svelte';

  // Shell de DESKTOP (>=820px): sidebar fixa + chat largo. Reusa o componente Chat do mobile
  // sem alteracao; abaixo de 820px o App nem monta isto (fica o fluxo mobile intacto).
  interface Props {
    currentSession: string | null;
    onNavigateToChat: (name: string) => void;
    onLogout: () => void;
  }
  let { currentSession, onNavigateToChat, onLogout }: Props = $props();
</script>

<div class="desktop-shell">
  <Sidebar {currentSession} onSelect={onNavigateToChat} {onLogout} />

  <main class="desktop-main">
    {#if currentSession}
      {#key currentSession}
        <Chat
          sessionName={currentSession}
          onBack={() => onNavigateToChat('')}
          onNavigateToChat={onNavigateToChat}
        />
      {/key}
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
