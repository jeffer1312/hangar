<script lang="ts">
  import { isAuthenticated } from './lib/auth';
  import Login from './screens/Login.svelte';
  import SessionList from './screens/SessionList.svelte';
  import Chat from './screens/Chat.svelte';

  // ── Hash-based Router ────────────────────────────────────────────────
  type Route =
    | { name: 'login' }
    | { name: 'sessions' }
    | { name: 'chat'; sessionName: string };

  function parseHash(hash: string): Route {
    const path = hash.replace(/^#/, '');
    const chatMatch = path.match(/^\/chat\/(.+)$/);
    if (chatMatch) {
      return { name: 'chat', sessionName: decodeURIComponent(chatMatch[1]) };
    }
    return { name: 'sessions' };
  }

  let currentHash = $state(window.location.hash || '#/');
  let authenticated = $state(isAuthenticated());

  const route: Route = $derived(
    !authenticated ? { name: 'login' } : parseHash(currentHash)
  );

  // Listen for hash changes
  $effect(() => {
    function onHashChange() {
      currentHash = window.location.hash || '#/';
    }
    window.addEventListener('hashchange', onHashChange);
    return () => window.removeEventListener('hashchange', onHashChange);
  });

  function navigateTo(hash: string) {
    window.location.hash = hash;
  }

  function navigateToChat(name: string) {
    navigateTo('#/chat/' + encodeURIComponent(name));
  }

  function navigateToSessions() {
    navigateTo('#/');
  }

  function onLogin() {
    authenticated = true;
    navigateTo('#/');
  }

  function onLogout() {
    authenticated = false;
    navigateTo('#/');
  }
</script>

<div class="app-root">
  {#if route.name === 'login'}
    <Login {onLogin} />
  {:else if route.name === 'sessions'}
    <SessionList
      onNavigateToChat={navigateToChat}
      {onLogout}
    />
  {:else if route.name === 'chat'}
    <!-- Remonta ao trocar de sessao (switcher): re-roda loadHistory + reconecta o SSE. -->
    {#key route.sessionName}
      <Chat
        sessionName={route.sessionName}
        onBack={navigateToSessions}
        onNavigateToChat={navigateToChat}
      />
    {/key}
  {/if}
</div>

<style>
  .app-root {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
</style>
