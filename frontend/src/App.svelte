<script lang="ts">
  import { isAuthenticated, setServers, listServers, mergeServers, onServersChanged, clearCredentials, selectServer } from './lib/auth';
  import { getVault, decryptList, encryptList, putVault, logout as syncLogout, syncStatus, stashKey, loadKey, clearKey } from './lib/sync';
  import { encodeCompareIds, parseCompareIds, type CompareId } from './lib/format';
  import Login from './screens/Login.svelte';
  import SessionList from './screens/SessionList.svelte';
  import Costs from './screens/Costs.svelte';
  import Archive from './screens/Archive.svelte';
  import Chat from './screens/Chat.svelte';
  import Compare from './screens/Compare.svelte';
  import DesktopShell from './components/DesktopShell.svelte';

  // ── Hash-based Router ────────────────────────────────────────────────
  type Route =
    | { name: 'loading' }
    | { name: 'login' }
    | { name: 'sessions' }
    | { name: 'costs' }
    | { name: 'archive'; deepLink?: { serverId: string; project: string; sessionId: string } }
    | { name: 'chat'; sessionName: string }
    | { name: 'board' }
    | { name: 'compare'; ids: CompareId[] };

  function parseHash(hash: string): Route {
    const path = hash.replace(/^#/, '');
    const chatMatch = path.match(/^\/chat\/(.+)$/);
    if (chatMatch) {
      const sessionName = decodeURIComponent(chatMatch[1]);
      // Auto-cura: um hash #/chat/undefined (ou vazio) preso na URL fazia o Chat montar com
      // sessionName "undefined" -> SSE em /sessions/undefined/events (404 em loop eterno).
      // Trata como invalido e cai na lista, em vez de prender o usuario numa sessao fantasma.
      // Barra "undefined" E "null" (string): ambos viravam #/chat/null -> currentSession="null"
      // (truthy) -> Chat monta -> openEventStream("null") -> GET /api/sessions/null/events 404 em loop.
      if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
        return { name: 'chat', sessionName };
      }
    }
    // Grade de comparação (feature #11): #/compare/<ids codificados>, ver encodeCompareIds/
    // parseCompareIds em lib/format.ts. Sem decodeURIComponent aqui — parseCompareIds já decodifica
    // cada campo por dentro (decodificar o param inteiro de novo ia dar decode duplo).
    const compareMatch = path.match(/^\/compare\/(.+)$/);
    if (compareMatch) return { name: 'compare', ids: parseCompareIds(compareMatch[1]) };
    if (path === '/costs') return { name: 'costs' };
    // Deep-link da busca (feature #10): #/archive/<serverId>/<project>/<sid> abre a conversa arquivada
    // direto no servidor dono. #/archive puro segue no browser normal de pastas.
    const archiveDeep = path.match(/^\/archive\/([^/]+)\/([^/]+)\/([^/]+)$/);
    if (archiveDeep) {
      return {
        name: 'archive',
        deepLink: {
          serverId: decodeURIComponent(archiveDeep[1]),
          project: decodeURIComponent(archiveDeep[2]),
          sessionId: decodeURIComponent(archiveDeep[3]),
        },
      };
    }
    if (path === '/archive') return { name: 'archive' };
    // Quadro kanban (visualização irmã da lista+chat) — só existe no desktop; no mobile o render
    // trata board como a lista normal.
    if (path === '/board') return { name: 'board' };
    return { name: 'sessions' };
  }

  // Deep-link do push (feature #5): a notif abre '/?server=<id>&session=<name>' — o router so olha
  // window.location.hash, entao sem isto os query params eram ignorados e sempre caia na lista.
  // Roda ANTES do currentHash ler o hash (uma vez, no load do modulo).
  (function applyPushDeepLink() {
    const params = new URLSearchParams(window.location.search);
    const server = params.get('server');
    const session = params.get('session');
    if (server) selectServer(server); // no-op se o id nao existir na lista local
    if (session) window.location.hash = '#/chat/' + encodeURIComponent(session);
  })();

  let currentHash = $state(window.location.hash || '#/');
  let authenticated = $state(isAuthenticated());

  // Cloud-sync gate. syncEnabled: null enquanto sondamos /sync/status; depois true/false.
  // syncReady: ha encKey (login fresco OU restaurado do sessionStorage). Com sync ligado, o app so
  // entra com syncReady -> senao forca o login do hub, mesmo havendo servers em cache (senao os
  // pushes pro hub ficariam mudos por falta de chave).
  let syncEnabled = $state<boolean | null>(null);
  let syncReady = $state(false);

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
    syncEnabled === null
      ? { name: 'loading' }                                            // sondando o hub
      : syncEnabled
        ? (syncReady ? parseHash(currentHash) : { name: 'login' })     // sync: exige sessao com chave
        : (authenticated ? parseHash(currentHash) : { name: 'login' }) // sem sync: regra antiga
  );

  // Boot: sonda o hub. Se ligado, tenta restaurar a sessao do sessionStorage (encKey sobrevive ao
  // reload) sem repedir senha; senao cai no login do hub. Sem sync, segue a regra de localStorage.
  $effect(() => {
    (async () => {
      const s = await syncStatus();
      if (!s?.enabled) { syncEnabled = false; return; }
      syncEnabled = true;
      const key = await loadKey();
      if (key) {
        try {
          await establishSync(key);          // cookie ainda valido -> restaura sem repedir senha
        } catch {
          clearKey();                         // sessao morta (cookie expirado) -> cai no login do hub
        }
      }
    })();
  });

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
    // Nunca cria um hash de chat com nome invalido (evita gerar #/chat/undefined|null na origem).
    if (!name || name === 'undefined' || name === 'null') {
      navigateTo('#/');
      return;
    }
    navigateTo('#/chat/' + encodeURIComponent(name));
  }

  function navigateToSessions() {
    navigateTo('#/');
  }

  // Entrada da grade de comparação (feature #11): vem da seleção múltipla da lista de sessões
  // (Sidebar/SessionList), reusando a MESMA seleção do broadcast. Menos de 2 não abre — comparar
  // 0/1 sessão não faz sentido (o botão de origem já fica desabilitado antes disso, mas defensivo).
  function navigateToCompare(ids: CompareId[]) {
    if (ids.length < 2) return;
    navigateTo('#/compare/' + encodeCompareIds(ids));
  }

  // Abrir um card da grade: troca pro servidor dono e vai pro chat completo dele.
  function openCompareSession(name: string, serverId: string) {
    selectServer(serverId);
    navigateToChat(name);
  }

  function onLogin() {
    authenticated = true;
    navigateTo('#/');
  }

  // Cloud-sync: chave em memoria (vive a sessao) + revisao do vault no hub. Nada disso toca o disco.
  let encKey: CryptoKey | null = null;
  let vaultRev = 0;

  // Login fresco no hub (vindo da tela de login): persiste a chave na aba e estabelece a sessao.
  async function onSyncLogin(key: CryptoKey) {
    await stashKey(key);
    await establishSync(key);
  }

  // Estabelece a sessao de sync (login fresco OU restauracao no boot): puxa o vault, decifra,
  // reconcilia com a lista local, semeia o que faltava no hub e fica empurrando mutacoes locais.
  async function establishSync(key: CryptoKey) {
    encKey = key;
    const { enc_blob, rev } = await getVault();
    vaultRev = rev;
    // Reconcilia: junta o que o hub tem (remote) com o que ja existia so neste navegador. Sem isso,
    // servers adicionados ANTES do login nunca subiam -- o login so BAIXAVA. setServers nao re-empurra.
    const remote = enc_blob ? await decryptList(key, enc_blob) : [];
    const merged = mergeServers(remote, listServers());
    setServers(merged);
    if (merged.length !== remote.length) {
      // havia servers locais fora do hub -> semeia/atualiza o vault agora (1a subida)
      const seed = await putVault(await encryptList(key, merged), vaultRev);
      if ('rev' in seed) vaultRev = seed.rev;
    }
    onServersChanged(async () => {
      if (!encKey) return;
      try {
        let res = await putVault(await encryptList(encKey, listServers()), vaultRev);
        if ('conflict' in res) {           // rev velha: adota a do hub e tenta de novo uma vez
          vaultRev = res.conflict.rev;
          res = await putVault(await encryptList(encKey, listServers()), vaultRev);
        }
        if ('rev' in res) vaultRev = res.rev;
      } catch (e) {
        // Nao engole: um push que falha (rede/sessao) some sem isso. Loga pra ficar visivel.
        console.error('sync push falhou — server nao subiu pro hub:', e);
      }
    });
    syncReady = true;
    authenticated = true;
  }

  async function onLogout() {
    if (encKey) {
      try { await syncLogout(); } catch { /* hub unreachable — log out locally regardless */ }
      clearKey();
      clearCredentials();
      encKey = null;
      syncReady = false;
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
  {#if route.name === 'loading'}
    <div class="boot" aria-busy="true"></div>
  {:else if route.name === 'login'}
    <Login {onLogin} onSyncLogin={onSyncLogin} />
  {:else if route.name === 'costs'}
    <Costs onBack={() => navigateTo('#/')} />
  {:else if route.name === 'archive'}
    <!-- Remonta ao trocar de deep-link (busca -> outra conversa): reabre com o novo alvo. -->
    {#key route.deepLink ? `${route.deepLink.serverId}/${route.deepLink.project}/${route.deepLink.sessionId}` : ''}
      <Archive onBack={() => navigateTo('#/')} deepLink={route.deepLink ?? null} />
    {/key}
  {:else if route.name === 'compare'}
    <!-- Remonta ao trocar o conjunto comparado: fecha os streams antigos e abre os novos. -->
    {#key encodeCompareIds(route.ids)}
      <Compare ids={route.ids} onOpenSession={openCompareSession} onBack={navigateToSessions} />
    {/key}
  {:else if isDesktop}
    <!-- Desktop cobre sessions/chat/board (as demais rotas já saíram nos branches acima). -->
    <DesktopShell
      currentSession={route.name === 'chat' ? route.sessionName : null}
      view={route.name === 'board' ? 'board' : 'chat'}
      onToggleBoard={() => navigateTo(route.name === 'board' ? '#/' : '#/board')}
      onNavigateToChat={navigateToChat}
      onCompare={navigateToCompare}
      {onLogout}
    />
  {:else if route.name === 'sessions' || route.name === 'board'}
    <!-- Quadro é só desktop: #/board no mobile cai na lista normal (em vez de tela em branco). -->
    <SessionList
      onNavigateToChat={navigateToChat}
      onCompare={navigateToCompare}
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
