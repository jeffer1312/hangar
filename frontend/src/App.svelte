<script lang="ts">
  import { isAuthenticated, setServers, listServers, mergeServers, onServersChanged, clearCredentials, selectServer, getActiveId, type Server } from './lib/auth';
  import { getVault, decryptList, encryptList, putVault, logout as syncLogout, syncStatus, stashKey, loadKey, clearKey } from './lib/sync';
  import { vaultPush } from './lib/vaultPush.svelte';
  import { encodeCompareIds, type CompareId } from './lib/format';
  import { peekStep, initialPeek } from './lib/peek';
  import { parseHash, type Route } from './lib/route';
  import { parseConfig, comConfig, TELAS_DE_SERVIDOR, type TelaConfig } from './lib/configRoute';
  import { abrirConfig, fecharConfig } from './lib/configNav';
  import Login from './screens/Login.svelte';
  import SessionList from './screens/SessionList.svelte';
  import Costs from './screens/Costs.svelte';
  import Archive from './screens/Archive.svelte';
  import Chat from './screens/Chat.svelte';
  import Compare from './screens/Compare.svelte';
  import DesktopShell from './components/DesktopShell.svelte';
  import SettingsModal from './components/settings/SettingsModal.svelte';
  import TtsBar from './components/TtsBar.svelte';

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

  // Chegada com o painel JA no endereco (link colado, reload, PWA reaberto): sem isto, a entrada do
  // historico que carrega `?config=` e a MESMA de onde o fechar quer voltar — o ✕ "voltava" pra ela e
  // o painel reabria sozinho. Reescreve a entrada atual pro caminho limpo e empilha o painel por cima.
  (function normalizarChegadaDoPainel() {
    const inicial = window.location.hash;
    if (!parseConfig(inicial)) return;
    // Terceiro argumento presente AQUI de proposito: e o unico ponto onde queremos MESMO trocar a URL
    // sem navegar. (Nos outros, ele apagaria o fragmento — ver lib/configNav.ts.)
    history.replaceState({ cpDepth: 0 }, '', comConfig(inicial, null));
    window.location.hash = inicial;
    history.replaceState({ ...history.state, cpDepth: 1 }, '');
  })();

  // Aponta o servidor ativo pro dono da rota, SINCRONAMENTE. Tem que rodar ANTES de currentHash
  // mudar: `route` é $derived dele, e o Chat monta no MESMO tick buscando o histórico via apiFetch
  // (= servidor ATIVO). Num $effect isto rodaria só DEPOIS do mount — o Chat já teria buscado no
  // servidor anterior e mostrado "não encontrei o transcript" (era o bug de abrir card do quadro
  // com >1 servidor; com 1 só, `routed === getActiveId()` e o furo não aparece).
  // Devolve false quando o servidor é desconhecido (link de outra máquina, id re-pareado) -> o
  // caller cai na tela-base em vez de montar o Chat contra o servidor errado.
  // A ESPIADA entra aqui (e não num effect próprio) porque a ordem importa: o peekStep precisa ver o
  // ativo de ANTES da rota nova, então tem que rodar antes do selectServer abaixo. A regra e seus
  // casos de borda (deep-link frio, promoção pro chat, troca de card B->C) moram em lib/peek.ts,
  // puros e testados — ela já foi apagada uma vez por refactor (62ee600 reverteu o fd79dda), e o que
  // a protege é o peek.test.ts ficar vermelho, não este comentário.
  let peekMemo = initialPeek;
  function applyRouteServer(next: string): boolean {
    const r = parseHash(next);
    const peek = r.name === 'board' || r.name === 'canvas' ? r.serverId : null;
    const step = peekStep(peekMemo, r.name, peek, getActiveId());
    peekMemo = step.memo;
    if (step.restore) selectServer(step.restore);

    const routed = r.name === 'chat' || r.name === 'board' || r.name === 'canvas' ? r.serverId : null;
    if (!routed || routed === getActiveId()) return true;
    return selectServer(routed);
  }
  if (!applyRouteServer(window.location.hash || '#/')) window.location.hash = '#/';

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

  // Painel de Configuracoes: segundo eixo do endereco (?config=&srv=, ver lib/configRoute.ts).
  const cfg = $derived(parseConfig(currentHash));

  // ⚠ listServers() e getActiveId() leem localStorage e NAO sao reativos: sem este contador, remover o
  // servidor com o painel aberto deixaria `alvoConfig` devolvendo o objeto congelado, e a tela seguiria
  // lendo e GRAVANDO nele. Pior: removeServer promove list[0] a ativo (auth.ts:271-275), entao remover
  // o servidor ATIVO muda o destino das funcoes globais enquanto targetConfig continua null.
  let versaoServidores = $state(0);
  $effect(() => onServersChanged(() => versaoServidores++));

  // SEMPRE `listServers().find`. NUNCA o caminho de activeServer(): auth.ts:151-155 faz `?? list[0]`,
  // entao um id desconhecido devolveria O PRIMEIRO SERVIDOR DA LISTA, silenciosamente.
  const alvoConfig = $derived.by(() => {
    versaoServidores;                                   // dependencia explicita, ver acima
    return cfg?.srv ? (listServers().find((s) => s.id === cfg.srv) ?? null) : null;
  });

  // Alvo que nao resolve (servidor removido, link velho, id re-pareado) NAO abre tela de servidor: cai
  // na Aparencia, que e do aparelho e vale sempre. `semServidor` sozinho nao cobre isto — ele so apaga
  // as LINHAS da raiz, e nao ajuda quem ja esta dentro da tela ou chegou por deep-link.
  const telaEfetiva = $derived(
    !cfg ? null
    : (TELAS_DE_SERVIDOR.includes(cfg.tela) && !alvoConfig) ? ('aparencia' as TelaConfig)
    : cfg.tela,
  );

  // `targetServer` null significa "e o servidor ativo, use as funcoes globais" — o contrato que
  // ServerSettings/EnginesSettings ja tem. Manter isso preserva o self-heal de 401 do caminho global
  // (apiFetchForServer NAO faz self-heal de proposito, AccountMenu.svelte:94-96).
  const targetConfig = $derived.by(() => {
    versaoServidores;
    return alvoConfig && alvoConfig.id !== getActiveId() ? alvoConfig : null;
  });

  function irParaConfig(tela: TelaConfig) {
    abrirConfig(tela, cfg?.srv ?? null);
  }

  // ‹ (botaoEsquerdo de sub-tela no SettingsModal) e "subir um nivel", nao "voltar no tempo": os dois
  // coincidem quando a sub-tela foi empilhada por cima da raiz (cpDepth >= 2, history.back sobe pra
  // ela). Mas um deep-link cai DIRETO na sub-tela (normalizarChegadaDoPainel carimba cpDepth 1) — nao
  // ha parada da raiz atras dela no historico, entao back() sairia do painel inteiro. Empilha a raiz
  // por cima (abrirConfig conta como nova parada, cpDepth vira 2) em vez de voltar.
  function voltarConfig() {
    const prof = (history.state?.cpDepth as number | undefined) ?? 0;
    if (prof <= 1) abrirConfig('root', cfg?.srv ?? null);
    else history.back();
  }

  // Fechar destroi o painel; o gatilho que o abriu pode nao existir mais (no celular ele vai junto
  // com o drawer), e o foco cai no <body> — leitor de tela mudo e Tab recomecando do zero. O
  // SettingsModal nao pode devolver isto sozinho: onFechar dispara history.go(), que e assincrono, e
  // o BottomSheet so restaura foco numa transicao de `open` pra falso (aqui `open` e fixo em `true`,
  // o componente e DESTRUIDO). Reagir aqui, a `cfg` sumir, e o unico jeito de pegar o momento certo.
  let painelEstavaAberto = false;
  $effect(() => {
    const aberto = !!cfg;
    if (painelEstavaAberto && !aberto && document.activeElement === document.body) {
      document
        .querySelector<HTMLElement>('[aria-label="Menu da conta"], [aria-label="Abrir menu"]')
        ?.focus();
    }
    painelEstavaAberto = aberto;
  });

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
      const next = window.location.hash || '#/';
      // Servidor ANTES do render (ver applyRouteServer): o Chat monta no mesmo tick.
      if (!applyRouteServer(next)) {
        window.location.hash = next.startsWith('#/board') ? '#/board' : next.startsWith('#/canvas') ? '#/canvas' : '#/';
        return; // o hash novo dispara este handler de novo, agora sem servidor a resolver
      }
      currentHash = next;
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
  // A aplicação da regra vive em applyRouteServer (acima), NÃO num $effect: efeito roda depois do
  // DOM, e o Chat monta no mesmo tick buscando pelo servidor ativo — tarde demais.

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

  // Abrir um card do CANVAS: espelha o navigateToBoardCard — vira rota (overlay por cima do canvas),
  // sem selectServer aqui (o $effect da rota aponta o ativo). Separado do board pra o fechar do
  // overlay voltar pro #/canvas, não pro #/board.
  function navigateToCanvasCard(name: string, serverId: string) {
    navigateTo('#/canvas/' + encodeURIComponent(serverId) + '/' + encodeURIComponent(name));
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
      // Publica o resultado no vaultPush: console.error sozinho nao chega no usuario, e numa troca
      // de TOKEN o silencio significa "os outros aparelhos seguem com a chave velha".
      if (!encKey) { vaultPush.locked(); return; }
      try {
        let res = await putVault(await encryptList(encKey, listServers()), vaultRev);
        if ('conflict' in res) {           // rev velha: adota a do hub e tenta de novo uma vez
          vaultRev = res.conflict.rev;
          res = await putVault(await encryptList(encKey, listServers()), vaultRev);
        }
        if ('rev' in res) vaultRev = res.rev;
        vaultPush.ok();
      } catch (e) {
        // Nao engole: um push que falha (rede/sessao) some sem isso. Loga pra ficar visivel.
        console.error('sync push falhou — server nao subiu pro hub:', e);
        vaultPush.error(e);
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
    <!-- Desktop cobre sessions/chat/board/canvas (as demais rotas já saíram nos branches acima). -->
    <DesktopShell
      currentSession={route.name === 'chat' ? route.sessionName : null}
      currentKey={route.name === 'chat' ? (route.serverId ?? '') + '::' + route.sessionName : null}
      view={route.name === 'board' ? 'board' : route.name === 'canvas' ? 'canvas' : 'chat'}
      overlaySession={(route.name === 'board' || route.name === 'canvas') && route.sessionName && route.serverId
        ? { name: route.sessionName, serverId: route.serverId }
        : null}
      onOpenBoardSession={navigateToBoardCard}
      onOpenCanvasSession={navigateToCanvasCard}
      onCloseOverlay={() => navigateTo(route.name === 'canvas' ? '#/canvas' : '#/board')}
      onToggleBoard={() => navigateTo(route.name === 'board' ? '#/' : '#/board')}
      onToggleCanvas={() => navigateTo(route.name === 'canvas' ? '#/' : '#/canvas')}
      onNavigateToChat={navigateToChat}
      onCompare={navigateToCompare}
      {onLogout}
    />
  {:else if route.name === 'sessions' || route.name === 'board' || route.name === 'canvas'}
    <!-- Quadro/canvas são só desktop: no mobile caem na lista normal (em vez de tela em branco). -->
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

  <!-- Painel de Configuracoes: montado UMA vez, aqui, e nao dentro dos dois AccountMenu
       (Sidebar.svelte:1177 e SessionList.svelte:800). E isso que o faz sobreviver a travessia dos
       820px: o App nunca desmonta, o branch `isDesktop` desmonta os dois shells.
       A guarda de rota nao e detalhe: loading/login/costs/archive/compare — as rotas que saem antes
       do branch `isDesktop` —, entao sem ela um #/?config=avancado desenharia o painel por cima da
       tela de boot e dispararia fetch sem credencial. -->
  <!-- Player de TTS: montado UMA vez aqui, pelo mesmo motivo do SettingsModal logo abaixo — o App
       nunca desmonta, enquanto o Chat e remontado a cada troca de sessao pelo {#key} acima. Dentro
       do Chat o audio morreria em toda troca, e o proprio elemento perderia o destravamento do
       gesto do iOS. -->
  <TtsBar />

  {#if cfg && telaEfetiva && route.name !== 'login' && route.name !== 'loading'}
    <SettingsModal
      tela={telaEfetiva}
      alvo={targetConfig}
      nomeAlvo={alvoConfig?.label ?? null}
      semServidor={!alvoConfig}
      onIrPara={irParaConfig}
      onVoltar={voltarConfig}
      onFechar={fecharConfig}
    />
  {/if}
</div>

<style>
  .app-root {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
</style>
