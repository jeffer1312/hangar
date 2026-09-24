<script lang="ts">
  import { listServers, getActiveId, renameServer, updateServer, removeServer,
           onServersChanged, snapshotRemocao, removalStillMatches } from '../../lib/auth';
  import { checkPeer, descobrirMaquinas, getIdentificador, setIdentificador, listarPeers, removerPeerDoisLados,
           setPeerEnabled, type MaquinaDescoberta, type PeerView } from '../../lib/peers';
  import { registrarPeerDoisLados, type LadoState } from '../../lib/registrarPeerDoisLados';
  import { unirMaquinas, type LinhaMaquina, type MotivoSemId } from '../../lib/maquinas';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import ModalDialog from '../ModalDialog.svelte';
  import { alcanceDoServidor, type AlcanceDoServidor, type TipoEndereco } from '../../lib/alcance';
  import { getAtualizacaoEm, reiniciarServidorEm } from '@hangar/core';
  import AdicionarMaquina from './AdicionarMaquina.svelte';
  import AcessoSettings from './AcessoSettings.svelte';
  import ListaMaquinas from './ListaMaquinas.svelte';
  import ServerEditSheet from '../ServerEditSheet.svelte';
  import LinhaConfig from './LinhaConfig.svelte';
  import EscopoChip from './EscopoChip.svelte';
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import type { RemovalSnapshot, Server } from '../../lib/auth';
  import * as m from '../../paraglide/messages';

  // Tela Máquinas das Configurações (item C): controller LOCAL do CRUD de servidores, do alvo de
  // edição e do logout global. O App continua dono do roteamento, do servidor resolvido e do
  // logout/sync — nada disso é duplicado aqui.
  //
  // Contrato central (plano 4b):
  // - resolvedServer = objeto real escolhido por ?srv= (aparece na UI);
  // - apiTarget = null quando resolvedServer é o ativo (API global com self-heal) ou Server
  //   explícito quando não é — `null` NÃO significa "sem alvo";
  // - só resolvedServer === null significa indisponível.
  interface Props {
    resolvedServer: Server | null;
    apiTarget: Server | null;
    // Fallback de foco das confirmações: o botão FECHAR do modal é o controle que sempre sobra
    // acessível, mesmo quando o gatilho (linha de servidor, Sair) sai da a11y tree.
    fallbackFocus?: HTMLElement | null;
    onLogout: () => void | Promise<void>;
    // Config do servidor, só pra linha das origens do terminal. Opcional porque a tela existe (e
    // é útil) mesmo com o servidor fora do ar, quando não há config nenhuma pra ler.
    store?: ConfigServidorStore;
  }
  let { resolvedServer, apiTarget, fallbackFocus = null, onLogout, store }: Props = $props();

  const CAMPO_TERM_ORIGINS = {
    chave: 'term_origins', tipo: 'texto' as const,
    rotulo: m.config_term_origins(), ajuda: m.config_term_origins_ajuda(),
  };
  // A origem DESTE app, quando o servidor a recusaria. Vem do próprio backend (ele responde a
  // mesma pergunta que o handshake do terminal faz), e não de um palpite do navegador.
  const origemRecusada = $derived(
    store?.leitura?.terminal_origem_ok === false ? window.location.origin : '',
  );

  // lista reativa local: listServers() lê localStorage e não é reativo; o contador sobe pelo mesmo
  // onServersChanged que o App usa (o sync cross-aparelho também passa por ele).
  let serverVersion = $state(0);
  $effect(() => onServersChanged(() => serverVersion++));
  const servers = $derived.by(() => {
    serverVersion;   // dependência explícita do contador (listServers não é reativo)
    return listServers();
  });
  function rename(id: string, label: string) {
    renameServer(id, label);
    sessionsStore.refreshServers();
  }
  function updateToken(id: string, token: string): boolean {
    const ok = updateServer(id, { token });
    if (!ok) return false;
    sessionsStore.refreshServers();
    sessionsStore.reconnect();
    return true;
  }
  // Não há mais "tornar ativo" AQUI de propósito. O servidor ativo é derivado da SESSÃO ABERTA:
  // `App.applyRouteServer` chama `selectServer` a cada mudança de rota, síncrono, antes do chat
  // montar — abrir qualquer sessão o sobrescreve. Um botão nesta tela era um controle que mente.
  // O que se escolhe aqui é o ALVO das telas de config de servidor, e só.

  let showAdd = $state(false);
  let addEndereco = $state('');   // enderecoInicial do AdicionarMaquina, pré-preenchido por Acompanhar

  // Remoção com confirmação REAL (ConfirmDialog). O ÚLTIMO servidor é removível de propósito:
  // remover tudo dispara o logout global (única saída pra deslogar o aparelho) — por isso o
  // diálogo ganha o aviso extra quando é o último.
  let pendingRemoval = $state<(RemovalSnapshot & { label: string }) | null>(null);
  let avisoRemocao = $state('');
  function abrirRemocao(id: string) {
    if (logoutInFlight) return;   // logout andando: portas de saída bloqueadas
    const s = servers.find((x) => x.id === id);
    const snap = snapshotRemocao(s, serverVersion);
    if (!snap) return;
    pendingRemoval = { ...snap, label: s!.label };
  }
  function confirmRemoval() {
    if (!pendingRemoval) return;
    const snap = pendingRemoval;
    pendingRemoval = null;
    // Revalida por FINGERPRINT + REVISION (não só ID, round 4): o sync pode ter apagado, alterado
    // OU reintroduzido este servidor entre o diálogo e o clique — ou a lista inteira ter mudado
    // (removido noutro aparelho, ativo trocado). Remover calado uma entidade que mudou é mentira:
    // mostra o motivo (role=status) e não faz nada.
    const motivo = removalStillMatches(snap, listServers(), serverVersion);
    if (motivo) { avisoRemocao = motivo; return; }
    avisoRemocao = '';
    esteAberto = false;
    const wasActive = snap.id === getActiveId();
    removeServer(snap.id);   // auth notifica onServersChanged -> contador local e store reagem
    if (listServers().length === 0) { void logout(); return; }
    if (wasActive) { window.location.reload(); return; }
    sessionsStore.refreshServers();
  }

  // Sair também pede confirmação: recuperação exige o token/QR de novo.
  let confirmLogout = $state(false);
  let logoutMsg = $state('');
  // Logout idempotente: Sair e remover-último podem cair aqui ao mesmo tempo; o App é o dono
  // único do clear de credenciais (lib/logout.ts) — este guard só impede o disparo duplicado, e
  // enquanto a Promise anda as portas de saída ficam bloqueadas. Rejeição capturada no limite do
  // evento: nada de unhandled/hang, e o erro aparece recuperável na tela.
  let logoutInFlight = $state(false);
  async function logout() {
    if (logoutInFlight) return;
    logoutInFlight = true;
    logoutMsg = '';
    try {
      await onLogout();
    } catch {
      logoutMsg = m.config_servidores_sair_erro();
    } finally {
      logoutInFlight = false;
    }
  }

  // ── Seções da Task 5: identificador desta máquina + máquinas que este servidor alcança ───────
  // O backend/peers.json (lido também pelo hangar-send) guarda id -> {base_url, token}; a rota
  // devolve a credencial MASCARADA — este front só exibe o que já chegou mascarado.
  const ID_OK = /^[a-z0-9][a-z0-9_-]{0,31}$/;   // espelho da regra do backend (fullmatch)
  const ID_DICA = () => m.peers_identificador_dica({ exemplos: 'casa, notebook' });

  // Erro de rede/servidor com nome, no idioma da tela: mensagem crua ("Failed to fetch") é o
  // erro genérico que a régua do projeto proíbe, e `''` fazia a falha sumir da tela.
  // Mesmo formato das linhas 96 e 120, que são o precedente desta própria aba.
  function msgErro(e: unknown): string {
    return e instanceof Error ? `${m.falha_conexao()}: ${e.message}` : m.erro_desconhecido();
  }

  let identificador = $state('');
  let idOriginal = $state('');          // último valor commitado: blur sem mudança não re-PUTa
  let idCarregado = $state(false);
  let idSalvando = $state(false);
  let idErro = $state('');

  let peers = $state<PeerView[]>([]);
  let peersCarregando = $state(false);
  let peersErro = $state('');

  // Server.id + token → identificador: busca o id que o NAVEGADOR conhece de cada
  // máquina, pra casar com o id que o SERVIDOR conhece (peers) e desenhar uma linha só por
  // máquina. Não depende do alvo escolhido: sobrevive à troca de servidor no cabeçalho, só
  // máquina nova (ou token trocado) volta a ser perguntada.
  const cacheIds = new Map<string, string | null>();
  let idsNavegador = $state<Record<string, string | null>>({});   // Server.id → identificador
  // POR QUE faltou, quando faltou. "Não responde" e "responde sem identificador" pedem ações
  // opostas (esperar a máquina voltar / preencher um campo) e saíam na mesma frase ambígua.
  let motivosId = $state<Record<string, MotivoSemId>>({});
  let idsCarregando = $state(false);
  let emEdicao = $state<Server | null>(null);        // ServerEditSheet
  let removerLadoDeLaFalhou = $state(false);         // aviso depois de remover peer
  const linhas = $derived(unirMaquinas(servers, idsNavegador, peers, resolvedServer?.id ?? null, motivosId));

  // Geração da carga em voo: a resposta de um alvo que a aba já não mostra não escreve na
  // tela. Sem isto, trocar de servidor com uma chamada pendente deixa o dado do anterior
  // na tela e a remoção clicada nele sai para a máquina errada.
  let geracao = 0;

  // Máquina adicionada sem reload: a carga abaixo roda de novo pra ela entrar com identificador,
  // peer e medição, como se a tela tivesse acabado de abrir.
  let recarga = $state(0);
  $effect(() => {
    recarga;
    const meu = ++geracao;
    // Troca de alvo apaga o que era do anterior: erro, carregamento e diálogo aberto
    // pertencem à máquina que saiu da tela. idsNavegador NÃO zera — o cache é por máquina do
    // navegador, não por alvo escolhido.
    idErro = ''; peersErro = ''; removerPeerId = null;
    corrigeId = null; corrigeUrl = '';
    removerLadoDeLaFalhou = false;
    emEdicao = null; avisoRemocao = ''; logoutMsg = '';
    idRemotoErro = {}; idRemotoSalvando = '';
    // O reinício e o "salvo" também pertencem à máquina que saiu da tela: sem isto o sucesso (ou
    // o erro) de reiniciar a anterior ficava à vista no detalhe da nova.
    reiniciando = false; reinicioErro = ''; reinicioFeito = false; reinicioRecusado = false; idSalvo = false;
    reinicioAguardando = false; reinicioHora = '';
    // Gravação em voo pertence ao alvo que saiu da tela: sem isto o campo fica `readonly`
    // e o Confirmar do diálogo nasce desabilitado, para sempre, no alvo novo.
    idSalvando = false;
    // Estados de checagem pertencem ao alvo que saiu da tela (Task 8).
    estados = {};
    descobertas = null; descobrindo = false; descobertasErro = '';
    esteAberto = false; parearAberto = false;
    resumo = null; resumoErro = '';
    if (!resolvedServer) {
      // Servidor indisponível (resolvedServer null): não há o que ler — sem este gate a seção
      // lia o servidor ATIVO com a aba dizendo que o escolhido não existe.
      peers = []; identificador = ''; idOriginal = '';
      peersCarregando = false; idsCarregando = false; idCarregado = true;
      return;
    }
    const alvoResumo = resolvedServer;
    alcanceDoServidor(alvoResumo)
      .then((r) => { if (meu === geracao) resumo = r; })
      .catch((e) => { if (meu === geracao) resumoErro = msgErro(e); });
    void (async () => {
      // Ordem de carga: espera os identificadores do navegador ANTES de medir. Sem
      // isso `checarLista` lê `linhas` com `idsNavegador` vazio, nenhuma linha casa navegador
      // com peer, e a volta cai em `nao_configurado` sempre — a feature central não roda.
      await Promise.all([carregarIdentificador(meu), carregarPeers(meu), carregarIdsNavegador(meu)]);
      if (meu !== geracao) return;
      await checarLista(meu);
    })();
  });

  async function carregarIdentificador(meu: number) {
    idErro = '';
    try {
      const r = await getIdentificador(apiTarget);
      if (meu !== geracao) return;
      identificador = r.identificador;
      idOriginal = r.identificador;
    } catch (e) {
      if (meu !== geracao) return;
      idErro = msgErro(e);
    } finally {
      if (meu === geracao) idCarregado = true;
    }
  }

  async function carregarPeers(meu: number) {
    peersCarregando = true;
    peersErro = '';
    try {
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return;
      peers = lista;
    } catch (e) {
      if (meu !== geracao) return;
      peersErro = msgErro(e);
    } finally {
      if (meu === geracao) peersCarregando = false;
    }
  }

  async function carregarIdsNavegador(meu: number) {
    idsCarregando = true;
    const pares = await Promise.all(servers.map(async (s) => {
      const k = `${s.id}:${s.token}`;
      if (cacheIds.has(k)) return [s.id, cacheIds.get(k)!, 'vazio' as MotivoSemId] as const;
      let id: string | null = null;
      let motivo: MotivoSemId = 'vazio';
      try {
        id = (await getIdentificador(s)).identificador || null;
      } catch (e) {
        // 401 é o token deste aparelho recusado, não a máquina fora do ar — a mesma distinção que
        // a volta do peer já faz. Qualquer outra falha é rede: ela não respondeu.
        id = null;
        motivo = (e as Error & { status?: number }).status === 401 ? 'token' : 'sem_resposta';
      }
      if (id) cacheIds.set(k, id);   // só sucesso entra no cache — fracasso não trava sem identificador pra sempre
      return [s.id, id, motivo] as const;
    }));
    if (meu !== geracao) return;
    idsNavegador = Object.fromEntries(pares.map(([id, valor]) => [id, valor]));
    motivosId = Object.fromEntries(pares.map(([id, , motivo]) => [id, motivo]));
    idsCarregando = false;
  }

  // Identificador de OUTRA máquina, gravado daqui (PUT no .env dela). Sem isto, o aviso "está sem
  // identificador" não tinha campo nenhum: só trocando o servidor da tela inteira. O erro é POR
  // linha — uma gravação recusada não pode apagar o erro de outra.
  let idRemotoSalvando = $state('');   // Server.id em voo ('' = nenhum)
  let idRemotoErro = $state<Record<string, string>>({});
  async function salvarIdentificadorRemoto(linha: LinhaMaquina, valor: string) {
    const alvo = linha.navegador;
    if (!alvo || idRemotoSalvando) return;
    if (valor && !ID_OK.test(valor)) { idRemotoErro = { ...idRemotoErro, [alvo.id]: ID_DICA() }; return; }
    const meu = geracao;
    idRemotoSalvando = alvo.id;
    idRemotoErro = { ...idRemotoErro, [alvo.id]: '' };
    try {
      const r = await setIdentificador(alvo, valor);
      if (meu !== geracao) return;
      // O cache é por (servidor, token): sem invalidar, a tela seguiria mostrando o nome antigo
      // até o token mudar. E o identificador é a chave que casa navegador com peer, então a
      // medição dos dois lados roda de novo com o nome novo.
      cacheIds.delete(`${alvo.id}:${alvo.token}`);
      if (r.identificador) cacheIds.set(`${alvo.id}:${alvo.token}`, r.identificador);
      idsNavegador = { ...idsNavegador, [alvo.id]: r.identificador || null };
      motivosId = { ...motivosId, [alvo.id]: 'vazio' };
      await checarLista(meu);
    } catch (e) {
      if (meu !== geracao) return;
      idRemotoErro = { ...idRemotoErro, [alvo.id]: msgErro(e) };
    } finally {
      if (meu === geracao) idRemotoSalvando = '';
    }
  }

  // Mede os dois lados de cada peer: a IDA (este servidor -> peer) e a VOLTA pelo endereço que o
  // LADO DE LÁ guardou para esta máquina (decisão 3 da spec: aqui é LAN, lá pode ser Tailscale —
  // medir a volta pelo endereço deste navegador daria `falhou` num par que funciona).
  async function checarLista(meu: number) {
    const meuId = identificador;
    // Peer desligado no servidor é máquina que a pessoa sabe estar fora: medir só pinta vermelho.
    await Promise.all(peers.filter((p) => p.enabled !== false).map(async (p) => {
      const linha = linhas.find((l) => l.peer?.id === p.id);
      const idaP = checkPeer(apiTarget, p.base_url, p.id)
        .then((r) => ({ lado: 'ida', ...r }) as LadoState)
        .catch((e) => ({ lado: 'ida', estado: 'falhou', motivo: e instanceof Error ? e.message : String(e) } as LadoState));
      let voltaP: Promise<LadoState>;
      if (!linha?.navegador || !meuId) {
        voltaP = Promise.resolve({ lado: 'volta', estado: 'nao_configurado', motivo: 'token' } as LadoState);
      } else {
        const nav = linha.navegador;
        voltaP = listarPeers(nav)
          .then((deLa) => {
            const eu = deLa.find((x) => x.id === meuId);
            if (!eu) return { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' } as LadoState;
            return checkPeer(nav, eu.base_url, meuId).then((r) => ({ lado: 'volta', ...r, url: eu.base_url }) as LadoState);
          })
          .catch((e) => {
            // 401 é o token DESTE aparelho para aquela máquina recusado, não a máquina fora do ar —
            // não é o mesmo caso de "falhou" que abre a correção de endereço.
            const st = (e as Error & { status?: number }).status;
            return { lado: 'volta', estado: st === 401 ? 'recusou' : 'falhou', motivo: st === 401 ? 'credencial' : (e instanceof Error ? e.message : String(e)) } as LadoState;
          });
      }
      const [ida, volta] = await Promise.all([idaP, voltaP]);
      if (meu !== geracao) return;
      estados[p.id] = { lados: [ida, volta], ok: ida.estado === 'ok' && volta.estado === 'ok' };
      // Decisão 5 da spec: a correção de endereço abre também na montagem, quando a volta falhou
      // de verdade e este navegador tem o token para re-registrar.
      // `estranho` entra junto de `falhou`: o endereço guardado lá responde como OUTRA máquina,
      // e é exatamente o caso que este bloco conserta — sem isto ele só dizia "só de ida".
      if ((volta.estado === 'falhou' || volta.estado === 'estranho') && linha?.navegador && !corrigeId) {
        corrigeId = p.id;
        corrigeUrl = volta.url ?? p.base_url;
      }
    }));
  }

  // Salvar é um BOTÃO, não o blur: este campo grava o CP_SERVER_ID no .env, e o nome é o que as
  // outras máquinas usam pra chegar aqui. Gravar por sair do campo não avisa nem deixa desfazer.
  const idMudou = $derived(identificador.trim() !== idOriginal);
  let idSalvo = $state(false);
  function desfazerIdentificador() { identificador = idOriginal; idErro = ''; }
  function salvarIdentificador() {
    if (idSalvando) return;   // gravação em voo: Enter e botão não abrem uma segunda (linha 82)
    const valor = identificador.trim();
    if (valor === idOriginal) return;
    if (valor && !ID_OK.test(valor)) {
      idErro = ID_DICA();        // a dica É a regra: minúsculas, números, hífen
      return;
    }
    idSalvando = true;
    idErro = '';
    const meu = geracao;
    void setIdentificador(apiTarget, valor)
      .then((r) => { if (meu !== geracao) return; identificador = r.identificador; idOriginal = r.identificador; idSalvo = true; })
      .catch((e) => { if (meu !== geracao) return; idErro = msgErro(e); })
      .finally(() => { if (meu === geracao) idSalvando = false; });
  }

  // Reinício do serviço: o que faz valer Identificador, Escuta em e as outras chaves do .env. O
  // pedido passa pelo PRÓPRIO serviço; travado, ele não chega, e aí a saída é o app do computador
  // (o shell reinicia por fora). Fora do systemd o backend recusa com 409 e a tela diz isso.
  let reiniciando = $state(false);
  let reinicioErro = $state('');
  let reinicioFeito = $state(false);
  let reinicioRecusado = $state(false);   // 409: o servidor respondeu dizendo que não faz
  // Pedir não é reiniciar: quem diz que o serviço voltou é o estado que o motor grava
  // (`fase: pronto`), casado pelo pid do motor que ESTE pedido lançou.
  let reinicioAguardando = $state(false);
  let reinicioHora = $state('');
  // Ponte do shell Electron (shell/preload.cjs). Lida na hora, não no import: o preload injeta
  // `window.hangar` antes da página, mas uma const de topo congelaria `undefined` nos testes.
  type ReinicioShell = () => Promise<{ ok: boolean; motivo?: string; detalhe?: string }>;
  const reinicioPeloShell = (): ReinicioShell | undefined =>
    (window as { hangar?: { reiniciarServico?: ReinicioShell } }).hangar?.reiniciarServico;
  // Só faz sentido no computador que RODA o serviço: o systemd do shell é o desta máquina, e
  // mandar reiniciar daqui um servidor remoto reiniciaria o errado — o daqui, que está de pé.
  // "Servidor ativo" NÃO basta: o ativo pode ser a máquina de outro canto, alcançada por
  // Tailscale. A prova de que é o daqui é o endereço ser loopback.
  const EH_LOOPBACK = /^(localhost|127(\.\d+){3}|\[?::1\]?)$/i;
  const alvoEhLocal = $derived.by(() => {
    try { return EH_LOOPBACK.test(new URL(resolvedServer?.baseUrl ?? window.location.origin).hostname); }
    catch { return false; }
  });
  const podeReiniciarPorFora = $derived(alvoEhLocal && !!reinicioPeloShell());

  async function reiniciarServico() {
    if (reiniciando) return;
    const meu = geracao;   // resposta tardia não escreve na máquina que entrou na tela depois
    reiniciando = true;
    reinicioErro = '';
    reinicioFeito = false;
    reinicioRecusado = false;
    reinicioHora = '';
    try {
      const { pid } = await reiniciarServidorEm(apiTarget);
      if (meu !== geracao) return;
      reinicioAguardando = true;
      reiniciando = false;
      void confirmarReinicio(pid, meu);
    } catch (e) {
      if (meu !== geracao) return;
      // 409 é o servidor RECUSANDO com motivo (topologia que não reinicia sozinha, atualização já
      // rodando) — ele respondeu. Chamar isso de "falha na conexão" e sugerir que está travado
      // manda a pessoa pro caminho errado; aqui vale a frase do próprio backend.
      reinicioRecusado = (e as { status?: number }).status === 409;
      reinicioErro = reinicioRecusado && e instanceof Error
        ? e.message.replace(/^\d{3}:\s*/, '')
        : msgErro(e);
    } finally {
      if (meu === geracao) reiniciando = false;
    }
  }

  async function confirmarReinicio(pid: number, meu: number) {
    const limite = Date.now() + 120_000;
    try {
      while (Date.now() < limite) {
        await new Promise((r) => setTimeout(r, 2000));
        if (meu !== geracao) return;
        let estado;
        try {
          estado = (await getAtualizacaoEm(apiTarget)).estado;
        } catch {
          continue;   // servidor caído no meio do reinício: é o esperado, pergunta de novo
        }
        if (meu !== geracao) return;
        if (estado.pid !== pid || estado.fase !== 'pronto') continue;
        if (estado.ok) reinicioHora = new Date(estado.ts ?? Date.now()).toLocaleTimeString();
        else reinicioErro = estado.reinicio_erro || estado.erro || m.maquinas_servico_falhou();
        return;
      }
      if (meu === geracao) reinicioErro = m.maquinas_servico_sem_confirmacao();
    } finally {
      if (meu === geracao) reinicioAguardando = false;
    }
  }

  // Serviço travado: o pedido HTTP não chega nele. Aqui quem manda é o systemd, pelo shell.
  async function reiniciarPorFora() {
    const ponte = reinicioPeloShell();
    if (!ponte || reiniciando) return;
    const meu = geracao;
    reiniciando = true;
    reinicioErro = '';
    reinicioFeito = false;
    reinicioRecusado = false;
    try {
      const r = await ponte();
      if (meu !== geracao) return;
      if (r.ok) reinicioFeito = true;
      else reinicioErro = r.motivo === 'sem_systemd' || r.motivo === 'plataforma'
        ? m.maquinas_servico_sem_systemd()
        : `${m.maquinas_servico_shell_falhou()} ${r.detalhe ?? ''}`.trim();
    } catch (e) {
      if (meu !== geracao) return;
      reinicioErro = msgErro(e);
    } finally {
      if (meu === geracao) reiniciando = false;
    }
  }

  // Estados de checagem por peer: id -> {lados, ok, testando?} — testando é o gesto de registrar
  // em voo (farol cinza + "Testando…" em ListaMaquinas, e trava clique duplo em onFalar).
  let estados = $state<Record<string, { lados: LadoState[]; ok: boolean; testando?: boolean }>>({});
  let corrigeId = $state<string | null>(null);
  let corrigeUrl = $state('');       // endereço digitado no bloco de correção (bind:value)

  // Ações da lista unificada: Acompanhar reusa a remoção/adição de servidor de hoje;
  // Servidores se falam registra ou remove os dois lados de uma vez.
  function onAcompanhar(linha: LinhaMaquina, ligar: boolean) {
    if (!ligar && linha.navegador) { abrirRemocao(linha.navegador.id); return; }   // confirmação de hoje
    if (ligar && linha.peer) { addEndereco = linha.peer.base_url; showAdd = true; } // pede só o token
  }

  async function onToggleScan(linha: LinhaMaquina, ligar: boolean) {
    if (!linha.peer) return;
    const meu = geracao;
    peersErro = '';
    try {
      const lista = await setPeerEnabled(apiTarget, linha.peer.id, ligar);
      if (meu === geracao) peers = lista;
    } catch (e) {
      if (meu === geracao) peersErro = msgErro(e);
    }
  }

  async function onFalar(linha: LinhaMaquina, ligar: boolean) {
    if (!ligar && linha.peer) { removerPeerId = linha.peer.id; return; }            // confirmação de hoje
    if (!ligar || !linha.navegador || !linha.identificador) return;
    const id = linha.identificador;
    if (estados[id]?.testando) return;   // clique duplo: já tem um registro em voo
    const meu = geracao;
    peersErro = '';
    estados[id] = { lados: [], ok: false, testando: true };   // gesto em voo: farol cinza + "Testando…"
    try {
      const r = await registrarPeerDoisLados(apiTarget, { id, base_url: linha.navegador.baseUrl, token: linha.navegador.token });
      if (meu !== geracao) return;
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return;
      peers = lista;
      estados[r.id] = { lados: r.lados, ok: r.ok };
      // O bloco edita o endereço DESTA máquina (o que a volta bate), não o do peer.
      if (!r.ok) { corrigeId = r.id; corrigeUrl = r.meu_endereco; }
    } catch (e) {
      if (meu !== geracao) return;
      estados[id] = { lados: [], ok: false };   // sai do "testando": a listagem que falhou não deixa estado preso
      peersErro = msgErro(e);
    }
  }

  // "Testar de novo": re-registra e re-testa com o ENDEREÇO DIGITADO no lugar certo. A pergunta
  // do bloco é "qual endereço o X deve usar para chegar aqui?", então o que se digita é o
  // endereço DESTA máquina, para gravar LÁ — era passado como base_url do PEER, e o botão mexia
  // no lado oposto ao que a frase promete. Só fecha quando o par fecha; senão o estado novo fica
  // à vista. O token vem do NAVEGADOR: só há bloco de correção em linha com navegador.
  async function testarDeNovo(linha: LinhaMaquina) {
    const meu = geracao;
    const url = corrigeUrl.trim();
    if (!/^https?:\/\//.test(url)) { peersErro = m.url_invalida(); return; }
    peersErro = '';
    try {
      const alvo = { id: linha.identificador!, base_url: linha.peer?.base_url ?? linha.navegador!.baseUrl, token: linha.navegador!.token };
      const r = await registrarPeerDoisLados(apiTarget, alvo, url);
      if (meu !== geracao) return;
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return;
      peers = lista;
      estados[r.id] = { lados: r.lados, ok: r.ok };
      if (r.ok) { corrigeId = null; corrigeUrl = ''; }
    } catch (e) {
      if (meu !== geracao) return;
      peersErro = msgErro(e);
    }
  }

  // Fecha o bloco de correção (o usuário escolheu "deixar só de ida" — aceita o estado parcial).
  function fecharCorrige() {
    corrigeId = null;
    corrigeUrl = '';
  }

  // Busca no Tailscale, sob demanda (decisão do usuário: botão, não na abertura da tela — cada
  // busca bate em todos os peers online). null = nunca buscou; [] = buscou e não achou.
  let descobertas = $state<MaquinaDescoberta[] | null>(null);
  let descobrindo = $state(false);
  let descobertasErro = $state('');   // slot próprio: não apaga nem é apagado pelo erro dos peers
  const hostnameDe = (u: string) => { try { return new URL(u).hostname.toLowerCase(); } catch { return u.trim().toLowerCase(); } };
  // Máquina que já tem linha (neste aparelho ou no servidor) não é "nova". Casa por QUALQUER
  // nome dela (IP ou nome do Tailscale): registrada pelo nome e achada pelo IP é a mesma máquina.
  const novasDescobertas = $derived.by(() => {
    const conhecidas = new Set(linhas.flatMap((l) => [l.navegador?.baseUrl, l.peer?.base_url].filter((u): u is string => !!u).map(hostnameDe)));
    return (descobertas ?? []).filter((d) => ![hostnameDe(d.base_url), ...d.hosts.map((h) => h.toLowerCase())].some((h) => conhecidas.has(h)));
  });
  async function buscarNoTailscale() {
    if (descobrindo) return;
    const meu = geracao;
    descobrindo = true;
    descobertasErro = '';
    try {
      const lista = await descobrirMaquinas(apiTarget);
      if (meu !== geracao) return;
      descobertas = lista;
    } catch (e) {
      if (meu !== geracao) return;
      descobertas = null;   // resultado da busca anterior não fica à vista como se fosse desta
      descobertasErro = msgErro(e);
    } finally {
      if (meu === geracao) descobrindo = false;
    }
  }

  // Cartão deste servidor: o endereço que responde mais rápido resume a medição inteira, que
  // aparece completa no detalhe.
  let esteAberto = $state(false);
  let parearAberto = $state(false);
  let resumo = $state<AlcanceDoServidor | null>(null);
  let resumoErro = $state('');
  const NOME_TIPO: Record<TipoEndereco, () => string> = {
    nesta_maquina: m.acesso_nesta_maquina, rede_local: m.acesso_rede_local,
    tailscale: m.acesso_tailscale, publico: m.acesso_publico,
  };
  const resumoTexto = $derived.by(() => {
    if (resumoErro) return { texto: resumoErro, farol: 'nao' as const };
    if (!resumo) return { texto: m.acesso_testando(), farol: 'test' as const };
    const melhor = resumo.enderecos
      .filter((e) => e.estado === 'ok' && e.tipo !== 'nesta_maquina')
      .sort((a, b) => (a.tempo_ms ?? 0) - (b.tempo_ms ?? 0))[0];
    if (!melhor) return { texto: m.servidores_loopback_curto(), farol: 'nao' as const };
    return { texto: `${NOME_TIPO[melhor.tipo]()} · ${melhor.tempo_ms ?? 0} ms`, farol: 'ok' as const };
  });
  const esteNoAparelho = $derived(!!resolvedServer && servers.some((s) => s.id === resolvedServer.id));

  let removerPeerId = $state<string | null>(null);
  function removerPeerConfirmado() { const id = removerPeerId; removerPeerId = null; if (id) void removerPeer(id); }

  // Largura do PAINEL, não da janela: esta tela mora dentro do modal de config, e é a coluna dele
  // que decide se cabe lista + detalhe lado a lado. Medida por observador porque o template
  // precisa ramificar — a consulta de contêiner sozinha só muda CSS, e aqui muda quem desenha o
  // detalhe (coluna, no largo; folha, no estreito).
  // ✕ da linha: tira a máquina dos DOIS lados de uma vez (peer do servidor e entrada deste
  // navegador). Cada caixa desligada já fazia um lado; isto é os dois com uma confirmação.
  let removerLinha = $state<LinhaMaquina | null>(null);
  async function removerLinhaConfirmado() {
    const linha = removerLinha;
    removerLinha = null;
    if (!linha) return;
    // O lado do servidor primeiro: se ele falhar, a entrada deste navegador FICA — é o token dela
    // que permite tentar de novo. Apagar os dois com o peer ainda lá deixaria a máquina sem
    // linha pra remover e sem como refazer.
    if (linha.peer && !(await removerPeer(linha.peer.id))) return;
    if (linha.navegador && !linha.estaMaquina) {
      removeServer(linha.navegador.id);
      sessionsStore.refreshServers();
    }
  }

  // true = o peer saiu deste servidor (o lado de lá pode ter ficado, e a tela avisa).
  async function removerPeer(id: string): Promise<boolean> {
    const meu = geracao;
    peersErro = '';
    // O navegador conhece o peer que sai (mesmo motivo de removerPeerDoisLados existir): sem o
    // token dele, o lado de lá não dá pra desfazer, e a tela avisa em vez de fingir que desfez.
    const remoto = linhas.find((l) => l.peer?.id === id)?.navegador ?? null;
    try {
      const resultado = await removerPeerDoisLados(apiTarget, id, remoto);
      if (meu !== geracao) return false;
      removerLadoDeLaFalhou = resultado === false;
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return false;
      peers = lista;
      // Peer que saiu não pode deixar farol/estado ou correção presos a um id que não existe mais.
      const { [id]: _descartado, ...resto } = estados;
      estados = resto;
      if (corrigeId === id) fecharCorrige();
      return true;
    } catch (e) {
      if (meu !== geracao) return false;
      peersErro = msgErro(e);
      removerLadoDeLaFalhou = false;
      return false;
    }
  }
</script>

{#snippet detalheEste(comFechar: boolean)}
  {#if resolvedServer}
  <div class="sd-rolagem">
  <div class="sv-cab">
    <div class="sv-cab-txt">
      <h2 class="sv-cab-nome">{resolvedServer.label}</h2>
      <span class="sv-cab-sub">{m.peers_esta_maquina()}</span>
    </div>
    {#if comFechar}
      <button type="button" class="sv-fechar" onclick={() => (esteAberto = false)} aria-label={m.sessao_fechar()}>✕</button>
    {/if}
  </div>
  <!-- Identificador (Task 5): é o CP_SERVER_ID, gravado no .env — o mesmo que o hangar-send usa
       no endereço de resposta srv::sessao. Vazio = pareamento entre servidores recusado. -->
  {#if !identificador}
    <p class="ss-legenda">{m.peers_legenda_identificador()}</p>
    <p class="id-aviso">{m.peers_aviso_nao_definido()}</p>
  {/if}
  <div class="id-linha">
    <span class="id-rot">{m.peers_identificador()} <EscopoChip escopo="env" />
      {#if identificador}
        <small>{m.peers_identificador_definido({ nome: identificador })}</small>
      {:else}
        <small>{ID_DICA()}</small>
      {/if}
    </span>
    <input class="id-campo" class:vazio={!identificador} bind:value={identificador}
           placeholder={m.peers_identificador_placeholder()}
           aria-label={m.peers_identificador()}
           autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
           disabled={!idCarregado}
           readonly={idSalvando}
           oninput={() => { idErro = ''; idSalvo = false; }}
           onkeydown={(e) => { if (e.key === 'Enter' && !idSalvando) salvarIdentificador(); }} />
  </div>
  {#if idMudou || idSalvando}
    <div class="id-acoes">
      <button type="button" class="btn primario" onclick={salvarIdentificador} disabled={idSalvando}>
        {idSalvando ? m.config_motores_salvando() : m.ctx_salvar()}
      </button>
      <button type="button" class="btn" onclick={desfazerIdentificador} disabled={idSalvando}>{m.comum_desfazer()}</button>
    </div>
  {:else if idSalvo}
    <p class="id-ok" role="status">{m.maquinas_id_salvo()}</p>
  {/if}
  {#if idErro}<p class="id-erro" role="alert">{idErro}</p>{/if}

  <AcessoSettings alvo={resolvedServer} parte="detalhe">
    {#snippet antesAvancado()}
      <!-- Reiniciar NÃO é avançado: é o gesto que faz valer o identificador, o endereço de escuta e as
           outras chaves do .env, e até agora o único caminho pra ele no app estava escondido no fluxo
           de atualização. Por isso fica no corpo do detalhe, não dentro do Avançado. -->
      <p class="ss-secao">{m.maquinas_servico()}</p>
      <p class="ss-legenda">{m.maquinas_servico_ajuda()}</p>
      <div class="id-acoes">
        <button type="button" class="btn primario" onclick={reiniciarServico} disabled={reiniciando || reinicioAguardando}>
          {reiniciando || reinicioAguardando ? m.maquinas_servico_reiniciando() : m.maquinas_servico_reiniciar()}
        </button>
        {#if reinicioAguardando}<span class="id-ok" role="status">{m.maquinas_servico_aguardando()}</span>
        {:else if reinicioHora}<span class="id-ok" role="status">{m.maquinas_servico_reiniciado({ hora: reinicioHora })}</span>
        {:else if reinicioFeito}<span class="id-ok" role="status">{m.maquinas_servico_pedido()}</span>{/if}
      </div>
      {#if reinicioErro}
        <p class="id-erro" role="alert">{reinicioErro}</p>
        {#if !reinicioRecusado}<p class="ss-legenda">{m.maquinas_servico_travado()}</p>{/if}
        {#if podeReiniciarPorFora && !reinicioRecusado}
          <div class="id-acoes">
            <button type="button" class="btn" onclick={reiniciarPorFora} disabled={reiniciando}>{m.maquinas_servico_pelo_app()}</button>
          </div>
        {/if}
      {/if}
    {/snippet}
    {#snippet avancado()}
      <!-- Origens do terminal: mora junto dos endereços porque a pergunta é "de qual endereço o
           app pode abrir um terminal neste servidor". Quem tem um servidor só nunca precisa disso
           (mesma-origem já passa); quem tem dois descobria a recusa como uma tela preta. -->
      {#if store}
        <LinhaConfig campo={CAMPO_TERM_ORIGINS} store={store} />
        {#if origemRecusada}
          <p class="id-aviso" role="status">{m.config_term_origins_recusada({ origem: origemRecusada })}</p>
        {/if}
        <!-- O campo escreve num rascunho; sem este botão não havia como gravar a origem aqui. -->
        {#if store.erro}<p class="id-erro" role="alert">{store.erro}</p>{/if}
        {#if store.temMudanca || store.salvando || store.salvo}
          <div class="id-linha">
            {#if store.salvo}<span class="id-ok">{m.config_server_salvo()}</span>{/if}
            {#if store.temMudanca || store.salvando}
              <button type="button" class="btn primario" onclick={store.salvar} disabled={store.salvando}>
                {store.salvando ? m.config_motores_salvando() : m.ctx_salvar()}
              </button>
            {/if}
          </div>
        {/if}
      {/if}

    {/snippet}
  </AcessoSettings>

  <!-- Tirar o servidor escolhido deste aparelho é a mesma remoção de sempre; sendo o último, o
       diálogo avisa que isso desloga — e é o ÚNICO caminho pra tirar a última máquina. -->
  {#if esteNoAparelho}
    <div class="sv-rodape">
      <button type="button" class="sv-remover-este" onclick={() => abrirRemocao(resolvedServer?.id ?? '')} disabled={logoutInFlight}>{m.servidores_remover_deste_aparelho()}</button>
    </div>
  {/if}
  </div>
  {/if}
{/snippet}

<div class="sv-topo">
  <button type="button" class="sv-btn" onclick={() => { addEndereco = ''; showAdd = true; }}>+ {m.maquinas_adicionar()}</button>
  <button type="button" class="sv-btn primario" onclick={() => (parearAberto = true)} disabled={!resolvedServer}>{m.servidores_parear()}</button>
</div>
{#if avisoRemocao}<p class="ss-aviso" role="status">{avisoRemocao}</p>{/if}
{#if logoutMsg}<p class="ss-aviso" role="status">{logoutMsg}</p>{/if}

{#if resolvedServer}
  <!-- O servidor escolhido no seletor vira um cartão só; o detalhe dele traz identificador,
       endereços e o avançado. Ele não se repete na lista de baixo. -->
  <button type="button" class="sv-este" onclick={() => (esteAberto = true)}
          aria-label={m.servidores_abrir_aria({ nome: resolvedServer.label })}>
    <span class="sv-farol" class:ok={resumoTexto.farol === 'ok'} class:nao={resumoTexto.farol === 'nao'} aria-hidden="true">
      {resumoTexto.farol === 'test' ? '◌' : '●'}
    </span>
    <span class="sv-txt">
      <span class="sv-nome">{resolvedServer.label}{#if identificador}<span class="sv-id">{identificador}</span>{/if}</span>
      <span class="sv-estado">{m.peers_esta_maquina()} · {resumoTexto.texto}</span>
      {#if idCarregado && !identificador && !idErro}<span class="sv-estado aviso">{m.servidores_sem_identificador_curto()}</span>{/if}
      {#if origemRecusada}<span class="sv-estado aviso">{m.servidores_origem_recusada_curto()}</span>{/if}
    </span>
    <span class="sv-chev" aria-hidden="true">›</span>
  </button>
  {#if idErro && !esteAberto}<p class="id-erro" role="alert">{idErro}</p>{/if}
{/if}


{#if resolvedServer && esteAberto}
  <ModalDialog open={true} ariaLabel={resolvedServer.label} onClose={() => (esteAberto = false)} className="sd-dialogo">
    {@render detalheEste(true)}
  </ModalDialog>
{/if}

{#if resolvedServer && parearAberto}
  <ModalDialog open={true} ariaLabel={m.servidores_parear()} onClose={() => (parearAberto = false)} className="sd-dialogo">
    <div class="sd-rolagem">
    <div class="sv-cab">
      <div class="sv-cab-txt">
        <h2 class="sv-cab-nome">{m.servidores_parear()}</h2>
        <span class="sv-cab-sub">{resolvedServer.label}</span>
      </div>
      <button type="button" class="sv-fechar" onclick={() => (parearAberto = false)} aria-label={m.sessao_fechar()}>✕</button>
    </div>
    <AcessoSettings alvo={resolvedServer} parte="parear" />
    </div>
  </ModalDialog>
{/if}

<p class="ss-secao">{m.maquinas_secao()}</p>
<ListaMaquinas
  linhas={linhas.filter((l) => !l.estaMaquina)} {estados} meuIdentificador={identificador}
  carregando={idsCarregando || peersCarregando}
  corrige={corrigeId ? { id: corrigeId, url: corrigeUrl } : null}
  {onAcompanhar} {onFalar} {onToggleScan}
  idSalvando={idRemotoSalvando} idErro={idRemotoErro}
  onSalvarIdentificador={(l, v) => void salvarIdentificadorRemoto(l, v)}
  onEditar={(l) => (emEdicao = l.navegador)}
  onCorrige={(u) => { if (u === null) fecharCorrige(); else corrigeUrl = u; }}
  onTestarDeNovo={testarDeNovo}
  onRemover={(l) => (removerLinha = l)} />
{#if peersErro}<p class="id-erro" role="status">{peersErro}</p>{/if}
{#if removerLadoDeLaFalhou}<p class="ss-aviso" role="status">{m.maquinas_remover_peer_lado_de_la_falhou()}</p>{/if}
<div class="ss-acoes">
  <button class="ss-btn" onclick={() => sessionsStore.reconnect()} disabled={logoutInFlight}>{m.maquinas_reconectar()}</button>
  <button class="ss-btn ss-danger" onclick={() => (confirmLogout = true)} disabled={logoutInFlight}>{m.sessao_sair_curto()}</button>
</div>

<ServerEditSheet open={!!emEdicao} server={emEdicao} onClose={() => (emEdicao = null)} onRename={rename} onUpdateToken={updateToken} />
{#if showAdd}
  <AdicionarMaquina {fallbackFocus} onFechar={() => (showAdd = false)} onAdicionada={() => recarga++}
    {apiTarget} podeFalar={!!resolvedServer && !!identificador} enderecoInicial={addEndereco}
    busca={{ itens: descobertas === null ? null : novasDescobertas, buscando: descobrindo, erro: descobertasErro,
             podeBuscar: !!resolvedServer, onBuscar: buscarNoTailscale }} />
{/if}

{#if removerPeerId}
  {@const linhaRem = linhas.find((l) => l.peer?.id === removerPeerId)}
  <ConfirmDialog title={m.config_servidores_remover({ nome: removerPeerId })} aria={m.config_servidores_remover_aria()}
    {fallbackFocus}
    onClose={() => (removerPeerId = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (removerPeerId = null) },
      { label: m.peers_remover(), kind: 'danger', onClick: removerPeerConfirmado },
    ]}>
    <p class="ss-dialog-copy">{linhaRem?.navegador ? m.maquinas_remover_peer_lados() : m.maquinas_remover_peer_so_aqui()}</p>
  </ConfirmDialog>
{/if}

{#if removerLinha}
  <ConfirmDialog title={m.config_servidores_remover({ nome: removerLinha.nome })} aria={m.config_servidores_remover_aria()}
    {fallbackFocus}
    onClose={() => (removerLinha = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (removerLinha = null) },
      { label: m.lista_remover(), kind: 'danger', onClick: () => void removerLinhaConfirmado() },
    ]}>
    <!-- Os mesmos dois campos que a ação lê, e esta é a última tela antes de apagar: sem peer a
         remoção nem chega ao servidor; sem navegador ela não sai daqui e o lado de lá nem é
         tentado (`removerPeerDoisLados` para no `if (!remoto)`), que é a mesma razão do ternário
         do diálogo irmão, logo acima. -->
    <p class="ss-dialog-copy">{!removerLinha.peer ? m.maquinas_remover_linha_local() : removerLinha.navegador ? m.maquinas_remover_linha() : m.maquinas_remover_peer_so_aqui()}</p>
  </ConfirmDialog>
{/if}

{#if pendingRemoval}
  <ConfirmDialog title={m.config_servidores_remover({ nome: pendingRemoval.label })} aria={m.config_servidores_remover_aria()}
    {fallbackFocus}
    onClose={() => (pendingRemoval = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (pendingRemoval = null) },
      { label: m.lista_remover(), kind: 'danger', onClick: confirmRemoval },
    ]}>
    <!-- Com peer na linha só o lado deste aparelho sai, e o texto tem de dizer que o recado (e o
         token dele no servidor) fica — senão parece que apagou tudo, ou que não apagou nada. -->
    <p class="ss-dialog-copy">{linhas.some((l) => l.navegador?.id === pendingRemoval?.id && l.peer) ? m.config_servidores_token_removido_recado_fica() : m.config_servidores_token_removido()}</p>
    {#if servers.length === 1}<p class="ss-dialog-copy">{m.config_servidores_voltar()}</p>{/if}
  </ConfirmDialog>
{/if}

{#if confirmLogout}
  <ConfirmDialog title={m.config_servidores_sair_titulo()} aria={m.config_servidores_sair_aria()}
    {fallbackFocus}
    onClose={() => (confirmLogout = false)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (confirmLogout = false) },
      { label: m.sessao_sair_curto(), kind: 'danger', onClick: () => { confirmLogout = false; void logout(); } },
    ]}>
    <p class="ss-dialog-copy">{m.config_servidores_voltar()}</p>
  </ConfirmDialog>
{/if}

<style>
  .ss-aviso { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--warning); }

  .ss-secao {
    margin: var(--space-4) 0 var(--space-2) var(--space-2);
    color: var(--text-muted); font-size: var(--text-xs);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  .ss-legenda {
    margin: 0 var(--space-2) var(--space-2);
    color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4;
  }

  .sv-topo { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-2); margin-bottom: var(--space-3); }
  .sv-btn { min-height: 40px; padding: 0 var(--space-4); border-radius: var(--radius-md); border: 1px solid var(--border-default);
            color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .sv-btn:hover { background: var(--bg-hover); }
  .sv-btn.primario { background: var(--accent); border-color: var(--accent); color: #fff; }
  .sv-btn:disabled { opacity: 0.45; }

  /* Painel largo: lista fixa à esquerda, detalhe à direita. Estreito: uma coluna só, como o
     celular sempre foi — as duas regras moram aqui porque é a MESMA tela nos dois tamanhos. */
  .sv-este { display: flex; align-items: center; gap: var(--space-3); width: 100%; min-height: 60px; padding: var(--space-3);
             text-align: left; color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-md); }
  .sv-este:hover { background: var(--bg-hover); }
  .sv-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; color: var(--text-muted); }
  .sv-farol.ok { color: var(--success); }
  .sv-farol.nao { color: var(--error); }
  .sv-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sv-nome { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-base); font-weight: 600; }
  .sv-id { font-family: var(--font-mono); font-size: 11px; font-weight: 400; color: var(--text-muted);
           padding: 1px var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-full); }
  .sv-estado { font-size: var(--text-xs); color: var(--text-secondary); }
  .sv-estado.aviso { color: var(--warning); }
  .sv-chev { flex-shrink: 0; color: var(--text-muted); font-size: 18px; }

  .sv-cab { display: flex; align-items: flex-start; gap: var(--space-3); margin-bottom: var(--space-3); }
  .sv-cab-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sv-cab-nome { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .sv-cab-sub { font-size: var(--text-xs); color: var(--text-muted); }
  .sv-fechar { width: 36px; height: 36px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm); color: var(--text-muted); }
  .sv-fechar:hover { background: var(--bg-hover); color: var(--text-primary); }
  .sv-rodape { display: flex; justify-content: flex-end; margin-top: var(--space-4); }
  .sv-remover-este { min-height: 40px; padding: 0 var(--space-3); border-radius: var(--radius-sm); color: var(--error); font-size: var(--text-sm); }
  .sv-remover-este:hover { background: rgba(255, 69, 58, 0.1); }

  .ss-acoes { display: flex; justify-content: space-between; gap: var(--space-2); margin-top: var(--space-4); }
  .ss-btn {
    display: flex; align-items: center;
    min-height: 44px; padding: var(--space-2) var(--space-3);
    color: var(--accent); font-size: var(--text-sm); border-radius: var(--radius-sm);
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .ss-btn:hover { background: var(--bg-hover); }
  .ss-danger { color: var(--error); }
  .ss-danger:hover { background: rgba(255, 69, 58, 0.1); }

  .ss-dialog-copy { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }

  /* ── Seção Task 5: identificador desta máquina ─────────────────────────────────────────
     Classes espelhando o mock de servidores.html (que por sua vez veio destas mesmas telas e
     dos tokens de app.css) — o pedaço novo tem que ser do mesmo peso do resto da aba. */
  .id-linha { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-2) 0; }
  .id-rot { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); }
  .id-rot small { display: block; color: var(--text-muted); font-size: var(--text-xs);
                  line-height: 1.35; margin-top: 2px; }
  .id-campo {
    width: 200px; height: 34px; flex-shrink: 0; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; outline: none;
  }
  .id-campo.vazio { border-color: var(--warning); }
  .id-campo:focus { border-color: var(--accent); }
  .id-campo:disabled { opacity: 0.6; }
  .id-campo:read-only { opacity: 0.6; }
  .id-aviso { margin: 0 0 var(--space-2) var(--space-2); font-size: var(--text-xs); color: var(--warning); }
  .id-erro { margin: 0 0 var(--space-2) var(--space-2); font-size: var(--text-xs); color: var(--error); }
  .id-linha { display: flex; align-items: center; justify-content: flex-end; gap: var(--space-3); margin: 0 0 var(--space-3); }
  .id-ok { font-size: var(--text-xs); color: var(--text-secondary); }
  .id-acoes { display: flex; align-items: center; gap: var(--space-2); margin: var(--space-2) 0 var(--space-3); }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
    transition: transform 160ms ease-out;
  }
  .btn:not(:disabled):active { transform: scale(0.97); }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; }
</style>
