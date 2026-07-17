<script lang="ts">
  import { isAuthenticated, setServers, listServers, mergeServers, onServersChanged, clearCredentials, selectServer, getActiveId } from './lib/auth';
  import { getVault, decryptList, encryptList, putVault, logout as syncLogout, syncStatus, stashKey, loadKey, clearKey } from './lib/sync';
  import { encodeCompareIds, parseCompareIds, type CompareId } from './lib/format';
  import { peekStep, initialPeek } from './lib/peek';
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
    | { name: 'chat'; sessionName: string; serverId: string | null }
    // Quadro: sessionName/serverId preenchidos = overlay do card aberto por cima dele (#/board puro
    // = quadro sem overlay). Mesmo par de campos do 'chat' de propósito — é o que deixa o $effect
    // do servidor ativo servir às duas rotas sem duplicar a regra.
    | { name: 'board'; sessionName: string | null; serverId: string | null }
    | { name: 'compare'; ids: CompareId[] };

  function parseHash(hash: string): Route {
    const path = hash.replace(/^#/, '');
    // Rota de chat COM servidor: #/chat/<serverId>/<nome>. Sessões homônimas em servidores
    // diferentes precisam de hashes distintos — só o nome fazia o clique "não trocar" (hash igual
    // não dispara hashchange) e pior: o composer já falava com o servidor novo enquanto a tela
    // mostrava o transcript do antigo (cross-wire). Forma legada #/chat/<nome> segue aceita
    // (serverId null = servidor ativo).
    const chatServerMatch = path.match(/^\/chat\/([^/]+)\/(.+)$/);
    const chatMatch = chatServerMatch ? null : path.match(/^\/chat\/(.+)$/);
    if (chatServerMatch || chatMatch) {
      const serverId = chatServerMatch ? decodeURIComponent(chatServerMatch[1]) : null;
      const sessionName = decodeURIComponent(chatServerMatch ? chatServerMatch[2] : chatMatch![1]);
      // Auto-cura: um hash #/chat/undefined (ou vazio) preso na URL fazia o Chat montar com
      // sessionName "undefined" -> SSE em /sessions/undefined/events (404 em loop eterno).
      // Trata como invalido e cai na lista, em vez de prender o usuario numa sessao fantasma.
      // Barra "undefined" E "null" (string): ambos viravam #/chat/null -> currentSession="null"
      // (truthy) -> Chat monta -> openEventStream("null") -> GET /api/sessions/null/events 404 em loop.
      if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
        return { name: 'chat', sessionName, serverId };
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
    // trata board como a lista normal. ANTES do regex de 2 segmentos: só pra deixar explícito que o
    // quadro puro é a forma base (os dois padrões não se sobrepõem — o regex exige serverId+nome).
    if (path === '/board') return { name: 'board', sessionName: null, serverId: null };
    // Overlay do card é ROTA (#/board/<serverId>/<nome>), não estado do shell: deep-link, botão
    // VOLTAR e reload saem de graça, e o servidor ativo vira função da rota (o $effect abaixo aponta
    // ele) em vez de exigir capture/restore manual ao abrir/fechar o overlay.
    const boardMatch = path.match(/^\/board\/([^/]+)\/(.+)$/);
    if (boardMatch) {
      const sessionName = decodeURIComponent(boardMatch[2]);
      // Mesma auto-cura do #/chat: um hash podre montaria o Chat com sessionName "undefined"/"null"
      // -> SSE em /sessions/undefined/events (404 em loop). Aqui degrada pro quadro sem overlay.
      if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
        return { name: 'board', sessionName, serverId: decodeURIComponent(boardMatch[1]) };
      }
      return { name: 'board', sessionName: null, serverId: null };
    }
    return { name: 'sessions' };
  }

  // Deep-link do push (feature #5): a notif abre '/?server=<id>&session=<name>' — o router so olha
  // window.location.hash, entao sem isto os query params eram ignorados e sempre caia na lista.
  // Roda ANTES do currentHash ler o hash (uma vez, no load do modulo).
  (function applyPushDeepLink() {
    const params = new URLSearchParams(window.location.search);
    const server = params.get('server');
    const session = params.get('session');
    // Servidor do push desconhecido localmente (re-pareado com id novo, storage limpo, push
    // antigo)? NÃO navega pro chat — montaria contra o servidor ativo errado (cross-wire calado
    // se houver sessão homônima). Cai na lista, que é sempre segura.
    const serverOk = server ? selectServer(server) : true;
    if (session && serverOk) {
      window.location.hash = '#/chat/'
        + (server ? encodeURIComponent(server) + '/' : '')
        + encodeURIComponent(session);
    }
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

  // Rota manda no servidor ativo: URL colada/back-button com #/chat/<server>/<nome> — ou
  // #/board/<server>/<nome>, o overlay do card — ativa o servidor certo mesmo sem passar por um
  // clique (que já chama selectServer antes de navegar). Servidor DESCONHECIDO (link de outra
  // máquina, id re-pareado) -> cai na tela-base da rota em vez de montar o chat contra o servidor
  // ativo errado (cross-wire calado com sessão homônima).
  // UMA regra pras duas rotas (em vez de um $effect por rota): as duas montam o MESMO Chat contra o
  // servidor ativo, então a condição é idêntica — só o destino do fallback muda. Duplicar seria
  // convidar as duas cópias a divergir.
  //
  // ESPIADA (#/board/<server>/<nome>): num quadro que mostra TODAS as máquinas, abrir um card não
  // muda onde você está — o ativo volta pro de antes ao fechar. A REGRA (e seus casos de borda:
  // deep-link frio, promoção pro chat, troca de card B->C) mora em lib/peek.ts, pura e testada; aqui
  // fica só a aplicação dela. Ela já foi apagada uma vez por refactor (62ee600 reverteu o fd79dda),
  // então o que a protege é o peek.test.ts ficar vermelho, não este comentário.
  // Aqui e não no DesktopShell: o ativo já é decidido NESTE effect (um lugar só), e o App nunca
  // desmonta — #/costs e #/archive casam ANTES do branch isDesktop e DESMONTAM o shell, que foi o
  // que obrigou o teardown do fd79dda. Sem componente pra desmontar, aquela classe de bug some.
  let peekMemo = initialPeek;
  $effect(() => {
    if (route.name === 'loading' || route.name === 'login') return;  // ainda não há rota de verdade
    const peek = route.name === 'board' ? route.serverId : null;
    const step = peekStep(peekMemo, route.name, peek, getActiveId());
    peekMemo = step.memo;
    if (step.restore) selectServer(step.restore);

    const routed = route.name === 'chat' || route.name === 'board' ? route.serverId : null;
    if (routed && routed !== getActiveId() && !selectServer(routed)) {
      navigateTo(route.name === 'board' ? '#/board' : '#/');
    }
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
    // O servidor vai NO hash: todos os pontos de entrada chamam selectServer() antes de navegar,
    // então o ativo aqui é o dono da sessão. Sessões homônimas em servidores diferentes ganham
    // hashes distintos -> hashchange dispara e o {#key} remonta o Chat (SSE reconecta no certo).
    const sid = getActiveId();
    navigateTo('#/chat/' + (sid ? encodeURIComponent(sid) + '/' : '') + encodeURIComponent(name));
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

  // Abrir um card do QUADRO: vira rota (overlay por cima do kanban), não estado do shell. Sem
  // selectServer aqui de propósito — o $effect da rota é quem aponta o ativo, e é o mesmo caminho
  // que o deep-link/reload percorre. Um único lugar decide o servidor.
  function navigateToBoardCard(name: string, serverId: string) {
    navigateTo('#/board/' + encodeURIComponent(serverId) + '/' + encodeURIComponent(name));
  }

  function onLogin() {
    authenticated = true;
    navigateTo('#/');
  }

  // Cloud-sync: chave em memoria (vive a sessao) + revisao do vault no hub. Nada disso toca o disco.
  let encKey: CryptoKey | null = null;
  let vaultRev = 0;
  let unsubSync: (() => void) | null = null;

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
    // Relogin sem reload chama establishSync de novo: solta o listener anterior pra nao acumular
    // dois pushes do mesmo vault (o slot unico mascarava isto sobrescrevendo).
    unsubSync?.();
    unsubSync = onServersChanged(async () => {
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
      // Solta o listener do push (quem registra TEM que chamar — auth.ts:41). Sem isto ele ficava no
      // Set pela vida da pagina e o unico freio era o `if (!encKey) return` la dentro: real, mas
      // implicito, e um relogin ja empilhava o proximo por cima.
      unsubSync?.();
      unsubSync = null;
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
      currentKey={route.name === 'chat' ? (route.serverId ?? '') + '::' + route.sessionName : null}
      view={route.name === 'board' ? 'board' : 'chat'}
      overlaySession={route.name === 'board' && route.sessionName && route.serverId
        ? { name: route.sessionName, serverId: route.serverId }
        : null}
      onOpenBoardSession={navigateToBoardCard}
      onCloseOverlay={() => navigateTo('#/board')}
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
    <!-- Remonta ao trocar de sessao (switcher): re-roda loadHistory + reconecta o SSE.
         Key inclui o SERVIDOR: homônimas em servidores diferentes precisam remontar. -->
    {#key (route.serverId ?? '') + '::' + route.sessionName}
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
