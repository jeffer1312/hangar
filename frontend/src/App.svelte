<script lang="ts">
  import { isAuthenticated, setServers, listServers, onServersChanged, clearCredentials } from './lib/auth';
  import { getVault, decryptList, encryptList, putVault, logout as syncLogout } from './lib/sync';
  import Login from './screens/Login.svelte';
  import SessionList from './screens/SessionList.svelte';
  import Chat from './screens/Chat.svelte';
  import DesktopShell from './components/DesktopShell.svelte';

  // ── Hash-based Router ────────────────────────────────────────────────
  type Route =
    | { name: 'login' }
    | { name: 'sessions' }
    | { name: 'chat'; sessionName: string };

  function parseHash(hash: string): Route {
    const path = hash.replace(/^#/, '');
    const chatMatch = path.match(/^\/chat\/(.+)$/);
    if (chatMatch) {
      const sessionName = decodeURIComponent(chatMatch[1]);
      // Auto-cura: um hash #/chat/undefined (ou vazio) preso na URL fazia o Chat montar com
      // sessionName "undefined" -> SSE em /sessions/undefined/events (404 em loop eterno).
      // Trata como invalido e cai na lista, em vez de prender o usuario numa sessao fantasma.
      if (sessionName && sessionName !== 'undefined') {
        return { name: 'chat', sessionName };
      }
    }
    return { name: 'sessions' };
  }

  let currentHash = $state(window.location.hash || '#/');
  let authenticated = $state(isAuthenticated());

  // Desktop: >=820px renderiza o shell de duas colunas (sidebar + chat largo). Mobile (<820px)
  // mantem o fluxo de telas atual, INTOCADO (mesmos componentes, mesmas regras).
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    isDesktop = mq.matches;
    const on = () => (isDesktop = mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

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
    // Nunca cria um hash de chat com nome invalido (evita gerar #/chat/undefined na origem).
    if (!name || name === 'undefined') {
      navigateTo('#/');
      return;
    }
    navigateTo('#/chat/' + encodeURIComponent(name));
  }

  function navigateToSessions() {
    navigateTo('#/');
  }

  function onLogin() {
    authenticated = true;
    navigateTo('#/');
  }

  // Cloud-sync: chave em memoria (vive a sessao) + revisao do vault no hub. Nada disso toca o disco.
  let encKey: CryptoKey | null = null;
  let vaultRev = 0;

  // Apos login no hub: puxa o vault, decifra, hidrata a lista local e fica empurrando mutacoes locais.
  async function onSyncLogin(key: CryptoKey) {
    encKey = key;
    const { enc_blob, rev } = await getVault();
    vaultRev = rev;
    if (enc_blob) setServers(await decryptList(key, enc_blob));
    onServersChanged(async () => {
      if (!encKey) return;
      let res = await putVault(await encryptList(encKey, listServers()), vaultRev);
      if ('conflict' in res) {           // rev velha: adota a do hub e tenta de novo uma vez
        vaultRev = res.conflict.rev;
        res = await putVault(await encryptList(encKey, listServers()), vaultRev);
      }
      if ('rev' in res) vaultRev = res.rev;
    });
  }

  async function onLogout() {
    if (encKey) {
      await syncLogout();
      clearCredentials();
      encKey = null;
    }
    authenticated = false;
    navigateTo('#/');
  }
</script>

<!-- Filtro de refração do Liquid Glass (usado só em Chromium, via [data-liquid] na CSS do glass). -->
<svg width="0" height="0" style="position:absolute" aria-hidden="true" focusable="false">
  <filter id="liquid-glass" x="-20%" y="-20%" width="140%" height="140%" color-interpolation-filters="sRGB">
    <feTurbulence type="fractalNoise" baseFrequency="0.013 0.013" numOctaves="2" seed="7" result="n" />
    <feGaussianBlur in="n" stdDeviation="1.4" result="nb" />
    <feDisplacementMap in="SourceGraphic" in2="nb" scale="16" xChannelSelector="R" yChannelSelector="G" />
  </filter>
</svg>

<div class="app-root">
  {#if route.name === 'login'}
    <Login {onLogin} onSyncLogin={onSyncLogin} />
  {:else if isDesktop}
    <DesktopShell
      currentSession={route.name === 'chat' ? route.sessionName : null}
      onNavigateToChat={navigateToChat}
      {onLogout}
    />
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
