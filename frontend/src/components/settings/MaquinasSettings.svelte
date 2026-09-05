<script lang="ts">
  import { listServers, getActiveId, renameServer, updateServer, removeServer,
           onServersChanged, snapshotRemocao, removalStillMatches } from '../../lib/auth';
  import { checkPeer, getIdentificador, setIdentificador, listarPeers, removerPeerDoisLados,
           type PeerView } from '../../lib/peers';
  import { registrarPeerDoisLados, type LadoState } from '../../lib/registrarPeerDoisLados';
  import { unirMaquinas, type LinhaMaquina } from '../../lib/maquinas';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import AdicionarMaquina from './AdicionarMaquina.svelte';
  import AcessoSettings from './AcessoSettings.svelte';
  import ListaMaquinas from './ListaMaquinas.svelte';
  import ServerEditSheet from '../ServerEditSheet.svelte';
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
    onPickTarget: (id: string) => void;
    onLogout: () => void | Promise<void>;
  }
  let { resolvedServer, apiTarget, fallbackFocus = null, onPickTarget, onLogout }: Props = $props();

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

  // Server.id + token → identificador (Task 4): busca o id que o NAVEGADOR conhece de cada
  // máquina, pra casar com o id que o SERVIDOR conhece (peers) e desenhar uma linha só por
  // máquina. Não depende do alvo escolhido: sobrevive à troca de servidor no cabeçalho, só
  // máquina nova (ou token trocado) volta a ser perguntada.
  const cacheIds = new Map<string, string | null>();
  let idsNavegador = $state<Record<string, string | null>>({});   // Server.id → identificador
  let idsCarregando = $state(false);
  let emEdicao = $state<Server | null>(null);        // ServerEditSheet
  let removerLadoDeLaFalhou = $state(false);         // aviso depois de remover peer
  const linhas = $derived(unirMaquinas(servers, idsNavegador, peers, resolvedServer?.id ?? null));

  // Geração da carga em voo: a resposta de um alvo que a aba já não mostra não escreve na
  // tela. Sem isto, trocar de servidor com uma chamada pendente deixa o dado do anterior
  // na tela e a remoção clicada nele sai para a máquina errada.
  let geracao = 0;

  $effect(() => {
    const meu = ++geracao;
    // Troca de alvo apaga o que era do anterior: erro, carregamento e diálogo aberto
    // pertencem à máquina que saiu da tela. idsNavegador NÃO zera — o cache é por máquina do
    // navegador, não por alvo escolhido.
    idErro = ''; peersErro = ''; removerPeerId = null;
    corrigeId = null; corrigeUrl = '';
    removerLadoDeLaFalhou = false;
    // Gravação em voo pertence ao alvo que saiu da tela: sem isto o campo fica `readonly`
    // e o Confirmar do diálogo nasce desabilitado, para sempre, no alvo novo.
    idSalvando = false;
    // Estados de checagem pertencem ao alvo que saiu da tela (Task 8).
    estados = {};
    if (!resolvedServer) {
      // Servidor indisponível (resolvedServer null): não há o que ler — sem este gate a seção
      // lia o servidor ATIVO com a aba dizendo que o escolhido não existe.
      peers = []; identificador = ''; idOriginal = '';
      peersCarregando = false; idCarregado = true;
      return;
    }
    void (async () => {
      // Ordem de carga (Task 4): espera os identificadores do navegador ANTES de medir. Sem
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
      if (cacheIds.has(k)) return [s.id, cacheIds.get(k)!] as const;
      let id: string | null = null;
      try { id = (await getIdentificador(s)).identificador || null; } catch { id = null; }
      cacheIds.set(k, id);
      return [s.id, id] as const;
    }));
    if (meu !== geracao) return;
    idsNavegador = Object.fromEntries(pares);
    idsCarregando = false;
  }

  // Mede os dois lados de cada peer: a IDA (este servidor -> peer) e a VOLTA pelo endereço que o
  // LADO DE LÁ guardou para esta máquina (decisão 3 da spec: aqui é LAN, lá pode ser Tailscale —
  // medir a volta pelo endereço deste navegador daria `falhou` num par que funciona).
  async function checarLista(meu: number) {
    const meuId = identificador;
    await Promise.all(peers.map(async (p) => {
      const linha = linhas.find((l) => l.peer?.id === p.id);
      const idaP = checkPeer(apiTarget, p.base_url, p.id).then((r) => ({ lado: 'ida', ...r }) as LadoState);
      let voltaP: Promise<LadoState>;
      if (!linha?.navegador || !meuId) {
        voltaP = Promise.resolve({ lado: 'volta', estado: 'nao_configurado', motivo: 'token' } as LadoState);
      } else {
        const nav = linha.navegador;
        voltaP = listarPeers(nav)
          .then((deLa) => {
            const eu = deLa.find((x) => x.id === meuId);
            if (!eu) return { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' } as LadoState;
            return checkPeer(nav, eu.base_url, meuId).then((r) => ({ lado: 'volta', ...r, url: eu.base_url }) as LadoState & { url: string });
          })
          .catch((e) => ({ lado: 'volta', estado: 'falhou', motivo: e instanceof Error ? e.message : String(e) } as LadoState));
      }
      const [ida, volta] = await Promise.all([idaP, voltaP]);
      if (meu !== geracao) return;
      estados[p.id] = { lados: [ida, volta], ok: ida.estado === 'ok' && volta.estado === 'ok' };
      // Decisão 5 da spec: a correção de endereço abre também na montagem, quando a volta falhou
      // de verdade e este navegador tem o token para re-registrar.
      if (volta.estado === 'falhou' && linha?.navegador && !corrigeId) {
        corrigeId = p.id;
        corrigeUrl = (volta as { url?: string }).url ?? p.base_url;
      }
    }));
  }

  // Enter salva; blur salva SÓ se mudou (sair do campo sem tocar não re-PUTa o mesmo valor).
  function salvarIdentificador() {
    if (idSalvando) return;   // gravação em voo: Enter e blur não abrem uma segunda (linha 82)
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
      .then((r) => { if (meu !== geracao) return; identificador = r.identificador; idOriginal = r.identificador; })
      .catch((e) => { if (meu !== geracao) return; idErro = msgErro(e); })
      .finally(() => { if (meu === geracao) idSalvando = false; });
  }

  // Estados de checagem por peer: id -> {lados, ok} (Task 8).
  let estados = $state<Record<string, { lados: LadoState[]; ok: boolean }>>({});
  let corrigeId = $state<string | null>(null);
  let corrigeUrl = $state('');       // endereço digitado no bloco de correção (bind:value)

  // Ações da lista unificada (Task 4): Acompanhar reusa a remoção/adição de servidor de hoje;
  // Servidores se falam registra ou remove os dois lados de uma vez.
  function onAcompanhar(linha: LinhaMaquina, ligar: boolean) {
    if (!ligar && linha.navegador) { abrirRemocao(linha.navegador.id); return; }   // confirmação de hoje
    if (ligar && linha.peer) { addEndereco = linha.peer.base_url; showAdd = true; } // pede só o token
  }

  async function onFalar(linha: LinhaMaquina, ligar: boolean) {
    if (!ligar && linha.peer) { removerPeerId = linha.peer.id; return; }            // confirmação de hoje
    if (ligar && linha.navegador && linha.identificador) {
      const meu = geracao;
      peersErro = '';
      try {
        const r = await registrarPeerDoisLados(apiTarget, { id: linha.identificador, base_url: linha.navegador.baseUrl, token: linha.navegador.token });
        if (meu !== geracao) return;
        peers = await listarPeers(apiTarget);
        estados[r.id] = { lados: r.lados, ok: r.ok };
        if (!r.ok) { corrigeId = r.id; corrigeUrl = r.base_url; }
      } catch (e) {
        if (meu === geracao) peersErro = msgErro(e);
      }
    }
  }

  // "Testar de novo": re-registra e re-testa o peer no ENDEREÇO DIGITADO (o bloco de correção
  // existe justamente para testar um endereço novo). Só fecha quando o par fecha; senão o estado
  // novo fica à vista. O token vem do NAVEGADOR: só há bloco de correção em linha com navegador.
  async function testarDeNovo(linha: LinhaMaquina) {
    const url = corrigeUrl.trim();
    if (!/^https?:\/\//.test(url)) { peersErro = m.url_invalida(); return; }
    try {
      const r = await registrarPeerDoisLados(apiTarget, { id: linha.identificador!, base_url: url, token: linha.navegador!.token });
      const lista = await listarPeers(apiTarget);
      peers = lista;
      estados[r.id] = { lados: r.lados, ok: r.ok };
      if (r.ok) { corrigeId = null; corrigeUrl = ''; }
    } catch (e) {
      peersErro = msgErro(e);
    }
  }

  // Fecha o bloco de correção (o usuário escolheu "deixar só de ida" — aceita o estado parcial).
  function fecharCorrige() {
    corrigeId = null;
    corrigeUrl = '';
  }

  let removerPeerId = $state<string | null>(null);
  async function removerPeerConfirmado() {
    const id = removerPeerId;
    removerPeerId = null;
    if (!id) return;
    const meu = geracao;
    peersErro = '';
    // O navegador conhece o peer que sai (mesmo motivo de removerPeerDoisLados existir): sem o
    // token dele, o lado de lá não dá pra desfazer, e a tela avisa em vez de fingir que desfez.
    const remoto = linhas.find((l) => l.peer?.id === id)?.navegador ?? null;
    try {
      const resultado = await removerPeerDoisLados(apiTarget, id, remoto);
      if (meu !== geracao) return;
      removerLadoDeLaFalhou = resultado === false;
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return;
      peers = lista;
    } catch (e) {
      if (meu !== geracao) return;
      peersErro = msgErro(e);
    }
  }
</script>

<p class="ss-legenda">{m.maquinas_intro()}</p>
{#if resolvedServer}
  <p class="ss-editando">
    {m.config_servidores_editando_1()} <strong>{resolvedServer.label}</strong>{m.config_servidores_editando_2()}
  </p>
{:else}
  <p class="ss-editando ss-muted">{m.config_servidores_escolha()}</p>
{/if}
{#if avisoRemocao}<p class="ss-aviso" role="status">{avisoRemocao}</p>{/if}
{#if logoutMsg}<p class="ss-aviso" role="status">{logoutMsg}</p>{/if}

{#if resolvedServer}
  <!-- Bloco 1: esta máquina — como ela se chama para as outras, por onde responde, QR.
       Identificador (Task 5): é o CP_SERVER_ID, gravado no .env — o mesmo que o hangar-send usa
       no endereço de resposta srv::sessao. Vazio = pareamento entre servidores recusado. -->
  <p class="ss-secao">{m.peers_esta_maquina()}</p>
  {#if !identificador}
    <p class="ss-legenda">{m.peers_legenda_identificador()}</p>
    <p class="id-aviso">{m.peers_aviso_nao_definido()}</p>
  {/if}
  <div class="id-linha">
    <span class="id-rot">{m.peers_identificador()}
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
           onkeydown={(e) => { idErro = ''; if (e.key === 'Enter' && !idSalvando) salvarIdentificador(); }}
           onblur={salvarIdentificador} />
  </div>
  {#if idErro}<p class="id-erro" role="alert">{idErro}</p>{/if}

  <AcessoSettings alvo={resolvedServer} />
{/if}

<div class="ss-sep"></div>
<p class="ss-secao">{m.maquinas_secao()}</p>
<p class="ss-legenda">{m.maquinas_secao_legenda()}</p>
<ListaMaquinas
  {linhas} {estados} meuIdentificador={identificador}
  carregando={idsCarregando || peersCarregando}
  corrige={corrigeId ? { id: corrigeId, url: corrigeUrl } : null}
  {onAcompanhar} {onFalar}
  onEditar={(l) => (emEdicao = l.navegador)}
  onCorrige={(u) => { if (u === null) fecharCorrige(); else corrigeUrl = u; }}
  onTestarDeNovo={testarDeNovo}
  onAdicionar={() => { addEndereco = ''; showAdd = true; }} />
{#if peersErro}<p class="id-erro" role="status">{peersErro}</p>{/if}
{#if removerLadoDeLaFalhou}<p class="ss-aviso" role="status">{m.maquinas_remover_peer_lado_de_la_falhou()}</p>{/if}
<div class="ss-acoes">
  <button class="ss-btn" onclick={() => sessionsStore.reconnect()} disabled={logoutInFlight}>{m.config_servidores_reconectar()}</button>
  <button class="ss-btn ss-danger" onclick={() => (confirmLogout = true)} disabled={logoutInFlight}>{m.sessao_sair_curto()}</button>
</div>

<ServerEditSheet open={!!emEdicao} server={emEdicao} onClose={() => (emEdicao = null)} onRename={rename} onUpdateToken={updateToken} />
{#if showAdd}
  <AdicionarMaquina {fallbackFocus} onFechar={() => (showAdd = false)}
    apiTarget={apiTarget} podeFalar={!!resolvedServer && !!identificador} enderecoInicial={addEndereco} />
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

{#if pendingRemoval}
  <ConfirmDialog title={m.config_servidores_remover({ nome: pendingRemoval.label })} aria={m.config_servidores_remover_aria()}
    {fallbackFocus}
    onClose={() => (pendingRemoval = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (pendingRemoval = null) },
      { label: m.lista_remover(), kind: 'danger', onClick: confirmRemoval },
    ]}>
    <p class="ss-dialog-copy">{m.config_servidores_token_removido()}</p>
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
  .ss-editando { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); }
  .ss-editando strong { color: var(--text-primary); font-weight: 600; }
  .ss-muted { color: var(--text-muted); }
  .ss-aviso { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--warning); }

  .ss-sep { height: 1px; background: var(--border-subtle); margin: var(--space-3) 0; }
  .ss-secao {
    margin: 0 0 var(--space-1) var(--space-2);
    color: var(--text-muted); font-size: var(--text-xs);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  .ss-legenda {
    margin: 0 var(--space-2) var(--space-2);
    color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4;
  }

  .ss-acoes { display: flex; flex-direction: column; gap: var(--space-1); }
  .ss-btn {
    display: flex; align-items: center; justify-content: flex-start;
    width: 100%; min-height: 44px; padding: var(--space-2) var(--space-4);
    text-align: left;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: 0;
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
</style>
