<script lang="ts">
  import { listServers, getActiveId, renameServer, updateServer, removeServer,
           onServersChanged, snapshotRemocao, removalStillMatches } from '../../lib/auth';
  import { checkPeer, getIdentificador, setIdentificador, listarPeers, removerPeer,
           type PeerView } from '../../lib/peers';
  import { registrarPeerDoisLados, type LadoState } from '../../lib/registrarPeerDoisLados';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import ServerManager from '../ServerManager.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import AdicionarMaquina from './AdicionarMaquina.svelte';
  import AcessoSettings from './AcessoSettings.svelte';
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

  // Remoção com confirmação REAL (ConfirmDialog). O ÚLTIMO servidor é removível de propósito:
  // remover tudo dispara o logout global (única saída pra deslogar o aparelho) — por isso o
  // ServerManager recebe `podeRemoverUltimo`.
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

  // Geração da carga em voo: a resposta de um alvo que a aba já não mostra não escreve na
  // tela. Sem isto, trocar de servidor com uma chamada pendente deixa o dado do anterior
  // na tela e a remoção clicada nele sai para a máquina errada.
  let geracao = 0;

  $effect(() => {
    const meu = ++geracao;
    // Troca de alvo apaga o que era do anterior: erro, carregamento e diálogo aberto
    // pertencem à máquina que saiu da tela.
    idErro = ''; peersErro = ''; mostrandoRegistro = false; removerPeerId = null;
    corrigeId = null; corrigeUrl = ''; corrigeToken = '';
    // Gravação em voo pertence ao alvo que saiu da tela: sem isto o campo fica `readonly`
    // e o Confirmar do diálogo nasce desabilitado, para sempre, no alvo novo.
    idSalvando = false; regSalvando = false;
    // Estados de checagem pertencem ao alvo que saiu da tela (Task 8).
    estados = {};
    if (!resolvedServer) {
      // Servidor indisponível (resolvedServer null): não há o que ler — sem este gate a seção
      // lia o servidor ATIVO com a aba dizendo que o escolhido não existe.
      peers = []; identificador = ''; idOriginal = '';
      peersCarregando = false; idCarregado = true;
      return;
    }
    void carregarIdentificador(meu);
    void carregarPeers(meu);
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
      // Task 8 (rodada 3): cada peer da lista ganha o estado REAL da ida ao montar — uma linha
      // nunca fica em "Testando as duas pontas…" quando ninguém está testando. Só a IDA: o token
      // do peer não volta da lista (PeerView.token é máscara), então a volta honesta é o selo
      // '·' ("não sei daqui"); a volta real só existe a partir do gesto de registrar.
      void checarLista(meu);
    } catch (e) {
      if (meu !== geracao) return;
      peersErro = msgErro(e);
    } finally {
      if (meu === geracao) peersCarregando = false;
    }
  }

  async function checarLista(meu: number) {
    const resultados = await Promise.all(peers.map(async (p) => {
      const ida = await checkPeer(apiTarget, p.base_url, p.id).catch((e) => ({
        estado: 'falhou' as const,
        motivo: String((e as Error)?.message ?? e),
      }));
      return [p.id, ida] as const;
    }));
    // Guard de geração: a resposta de um alvo que a aba já não mostra não escreve na tela.
    if (meu !== geracao) return;
    for (const [id, ida] of resultados) {
      estados = { ...estados, [id]: {
        lados: [{ lado: 'ida', ...ida }, { lado: 'volta', estado: 'nao_configurado' }],
        ok: false,
      } };
    }
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

  // Registrar um peer (Task 5 → 8): o gesto único agora registra os DOIS lados de uma vez
  // (A em B e B em A, com a credencial que o celular já guarda de cada um) e testa cada lado
  // com a primitiva da Task 3. A tela mostra o estado de cada lado e, quando um falha, abre o
  // bloco de correção de endereço (mock estado 3).
  let mostrandoRegistro = $state(false);
  let regId = $state('');
  let regUrl = $state('');
  let regToken = $state('');
  let regErro = $state('');
  let regSalvando = $state(false);
  // Estados de checagem por peer: id -> {lados, ok, endereco_alternativo} (Task 8).
  let estados = $state<Record<string, { lados: LadoState[]; ok: boolean; endereco_alternativo?: string }>>({});
  let corrigeId = $state<string | null>(null);
  let corrigeUrl = $state('');       // endereço digitado no bloco de correção (bind:value)
  let corrigeToken = $state('');     // credencial do gesto que abriu o bloco (não volta da lista)

  // Selo de UM lado derivado do estado (Task 8, rodada 3): ✓ passou, ✗ falhou, · não sei
  // (nao_configurado — nunca é nem uma coisa nem outra). O glifo não é mais texto chumbado.
  const selo = (l?: LadoState) =>
    l?.estado === 'ok' ? '✓' : l && l.estado !== 'nao_configurado' ? '✗' : '·';

  function abrirRegistro() {
    mostrandoRegistro = true;
    regId = ''; regUrl = ''; regToken = ''; regErro = '';
  }

  async function registrarPeer() {
    if (regSalvando) return;
    const id = regId.trim();
    const url = regUrl.trim();
    const token = regToken.trim();
    if (!ID_OK.test(id)) { regErro = ID_DICA(); return; }
    if (!/^https?:\/\//.test(url)) { regErro = m.url_invalida(); return; }
    regSalvando = true;
    regErro = '';
    const meu = geracao;
    try {
      // Task 8: um gesto registra os dois lados e testa os dois — a lista volta do backend.
      const r = await registrarPeerDoisLados(apiTarget, { id, base_url: url, token });
      if (meu !== geracao) return;
      // A lista nova vem da gravação no DONO (o backend devolve a lista atualizada).
      const lista = await listarPeers(apiTarget);
      if (meu !== geracao) return;
      peers = lista;
      // O estado dos dois lados fica na tela (mock estados 2 e 3).
      estados = { ...estados, [id]: { lados: r.lados, ok: r.ok, endereco_alternativo: r.endereco_alternativo } };
      if (!r.ok) {
        // Bloco de correção aberto no endereço que FALHOU — com a credencial do gesto guardada
        // (o token não volta da lista; é o que "Testar de novo" reusa no endereço digitado).
        corrigeId = id;
        corrigeUrl = url;
        corrigeToken = token;
      }
      mostrandoRegistro = false;
    } catch (e) {
      if (meu !== geracao) return;
      regErro = msgErro(e);
    } finally {
      if (meu === geracao) regSalvando = false;
    }
  }

  // Fecha o bloco de correção (o usuário escolheu "deixar só de ida" — aceita o estado parcial).
  function fecharCorrige() {
    corrigeId = null;
    corrigeUrl = '';
    corrigeToken = '';
  }

  // "Testar de novo": re-registra e re-testa o peer no ENDEREÇO DIGITADO (o bloco de correção
  // existe justamente para testar um endereço novo). Só fecha quando o par fecha; senão o estado
  // novo fica à vista. Risco do token guardado: regToken do gesto que abriu o bloco (não volta
  // da lista, que mascara) — mesmo contrato do registrarPeer.
  async function testarDeNovo(peer: PeerView) {
    const url = corrigeUrl.trim();
    if (!/^https?:\/\//.test(url)) { peersErro = m.url_invalida(); return; }
    try {
      const r = await registrarPeerDoisLados(apiTarget, { id: peer.id, base_url: url, token: corrigeToken });
      const lista = await listarPeers(apiTarget);
      peers = lista;
      estados = { ...estados, [peer.id]: { lados: r.lados, ok: r.ok, endereco_alternativo: r.endereco_alternativo } };
      if (r.ok) { corrigeId = null; corrigeUrl = ''; corrigeToken = ''; }
    } catch (e) {
      peersErro = msgErro(e);
    }
  }

  let removerPeerId = $state<string | null>(null);
  async function removerPeerConfirmado() {
    const id = removerPeerId;
    removerPeerId = null;
    if (!id) return;
    const meu = geracao;
    peersErro = '';
    try {
      const lista = await removerPeer(apiTarget, id);
      if (meu !== geracao) return;
      peers = lista;
    } catch (e) {
      if (meu !== geracao) return;
      peersErro = msgErro(e);
    }
  }
</script>

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

  <div class="ss-sep"></div>
  <!-- Bloco 2: máquinas que este servidor alcança (Task 5): a lista do peers.json. O estado das
       duas pontas (testar/liga) é da Task 8 — aqui se mostra e se edita o vínculo local. -->
  <p class="ss-secao">{m.peers_secao_alcance()}</p>
  {#if peers.length}
    <p class="ss-legenda">{m.peers_legenda_alcance()}</p>
    <div class="pr-cartao">
      {#each peers as peer (peer.id)}
        {@const st = estados[peer.id]}
        {@const ida = st?.lados.find((l) => l.lado === 'ida')}
        {@const volta = st?.lados.find((l) => l.lado === 'volta')}
        {@const ok = ida?.estado === 'ok' && volta?.estado === 'ok'}
        {@const meio = st && !ok}
        <div class="pr-linha">
          <span class="pr-farol" class:ok class:nao={meio} class:test={!st}>
            {st ? (ok ? '●' : '◌') : '◌'}
          </span>
          <span class="pr-txt">
            <span class="pr-nome">{peer.id}</span>
            <span class="pr-url">{peer.base_url}</span>
            {#if st}
              {#if ok}
                <span class="pr-estado ok">{m.peers_estado_ok()}</span>
              {:else}
                <span class="pr-estado nao">{m.peers_estado_parcial()}</span>
              {/if}
            {:else}
              <span class="pr-estado neutro">{m.peers_estado_testando()}</span>
            {/if}
          </span>
          {#if st}
            <span class="pr-lados">
          <span class="pr-lado" class:ok={ida?.estado === 'ok'} class:nao={ida && ida.estado !== 'ok' && ida.estado !== 'nao_configurado'}>{selo(ida)} {m.peers_lado_ida()}</span>
              <span class="pr-lado" class:ok={volta?.estado === 'ok'} class:nao={volta && volta.estado !== 'ok' && volta.estado !== 'nao_configurado'}>{selo(volta)} {m.peers_lado_volta()}</span>
            </span>
          {/if}
          <button class="pr-btn min" onclick={() => (removerPeerId = peer.id)}>{m.peers_remover()}</button>
        </div>
        {#if st && !ok && corrigeId === peer.id}
          <div class="corrige">
            <p>
              {m.peers_corrige_1({ nome: peer.id, endereco: peer.base_url })}
            </p>
            <p><b>{m.peers_corrige_pergunta({ nome: peer.id })}</b></p>
            <input class="corrige-input" bind:value={corrigeUrl} aria-label={m.peers_corrige_pergunta({ nome: peer.id })} />
            <div class="acoes">
              <button class="btn primaria" onclick={() => testarDeNovo(peer)}>{m.peers_testar_novamente()}</button>
              <button class="btn" onclick={() => fecharCorrige()}>{m.peers_so_ida()}</button>
            </div>
          </div>
        {/if}
      {/each}
    </div>
  {:else if peersCarregando}
    <p class="ss-legenda">{m.comum_carregando()}</p>
  {:else if identificador}
    <p class="ss-legenda">{m.peers_legenda_alcance()}</p>
  {:else}
    <p class="ss-legenda">{m.peers_vazio()}</p>
  {/if}
  {#if identificador}
    <div class="pr-acoes">
      <button class="pr-btn primaria" onclick={abrirRegistro}>{m.peers_registrar()}</button>
    </div>
  {/if}
  {#if peersErro}<p class="id-erro" role="status">{peersErro}</p>{/if}

  <div class="ss-sep"></div>
{/if}

<!-- Bloco 3: os servidores que ESTE navegador conhece. Fora do {#if}: é por aqui que se cadastra
     o primeiro, e é a única saída quando o ativo morreu. -->
<p class="ss-secao">{m.maquinas_este_aparelho()}</p>
<p class="ss-legenda">{m.maquinas_este_aparelho_legenda()}</p>
<ServerManager
  {servers}
  targetId={resolvedServer?.id ?? null}
  {onPickTarget}
  podeRemoverUltimo
  onRename={rename}
  onUpdateToken={updateToken}
  onRemove={abrirRemocao}
  onAdd={() => (showAdd = true)}
/>
<div class="ss-acoes">
  <button class="ss-btn" onclick={() => sessionsStore.reconnect()} disabled={logoutInFlight}>{m.config_servidores_reconectar()}</button>
  <button class="ss-btn ss-danger" onclick={() => (confirmLogout = true)} disabled={logoutInFlight}>{m.sessao_sair_curto()}</button>
</div>

{#if showAdd}
  <AdicionarMaquina {fallbackFocus} onFechar={() => (showAdd = false)} />
{/if}

{#if mostrandoRegistro}
  <ConfirmDialog title={m.peers_registrar()} aria={m.peers_registrar()} role="dialog"
    {fallbackFocus}
    onClose={() => (mostrandoRegistro = false)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (mostrandoRegistro = false) },
      { label: m.comum_confirmar(), kind: 'primary',
        disabled: regSalvando || !regId.trim() || !regUrl.trim() || !regToken.trim(),
        onClick: registrarPeer },
    ]}>
    <label class="pr-form-campo">
      <span class="pr-form-rot">{m.peers_identificador()}</span>
      <input class="pr-form-input" bind:value={regId}
             placeholder={m.peers_identificador_placeholder()}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             onkeydown={(e) => { regErro = ''; if (e.key === 'Enter' && regSalvando === false && regId.trim() && regUrl.trim() && regToken.trim()) registrarPeer(); }} />
    </label>
    <label class="pr-form-campo">
      <span class="pr-form-rot">{m.sessao_url_servidor()}</span>
      <input class="pr-form-input" bind:value={regUrl}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             onkeydown={(e) => { regErro = ''; if (e.key === 'Enter' && regSalvando === false && regId.trim() && regUrl.trim() && regToken.trim()) registrarPeer(); }} />
    </label>
    <label class="pr-form-campo">
      <span class="pr-form-rot">{m.sessao_token()}</span>
      <input class="pr-form-input" bind:value={regToken}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             onkeydown={(e) => { regErro = ''; if (e.key === 'Enter' && regSalvando === false && regId.trim() && regUrl.trim() && regToken.trim()) registrarPeer(); }} />
    </label>
    {#if regErro}<p class="pr-form-erro" role="alert">{regErro}</p>{/if}
  </ConfirmDialog>
{/if}

{#if removerPeerId}
  <ConfirmDialog title={m.config_servidores_remover({ nome: removerPeerId })} aria={m.config_servidores_remover_aria()}
    {fallbackFocus}
    onClose={() => (removerPeerId = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (removerPeerId = null) },
      { label: m.peers_remover(), kind: 'danger', onClick: removerPeerConfirmado },
    ]}>
    <p class="ss-dialog-copy">{m.config_servidores_token_removido()}</p>
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

  /* ── Seções Task 5: identificador + máquinas que este servidor alcança ──────────────────
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

  .pr-cartao { background: var(--surface-card); border: 1px solid var(--border-subtle);
               border-radius: var(--radius-md); overflow: hidden; }
  .pr-linha { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-3); }
  .pr-linha + .pr-linha { border-top: 1px solid var(--border-subtle); }
  .pr-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; }
  .pr-farol.ok { color: var(--success); }
  .pr-farol.nao { color: var(--error); }
  .pr-farol.test { color: var(--text-muted); }
  .pr-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  .pr-nome { font-size: var(--text-sm); color: var(--text-primary); }
  .pr-url { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
            word-break: break-all; }
  .pr-estado { font-size: var(--text-xs); line-height: 1.35; }
  .pr-estado.ok { color: var(--success); }
  .pr-estado.nao { color: var(--warning); }
  .pr-estado.neutro { color: var(--text-muted); }
  .pr-lados { display: flex; gap: var(--space-2); flex-shrink: 0; }
  .pr-lado { display: flex; align-items: center; gap: 4px; font-size: 11px; color: var(--text-muted);
             padding: 2px var(--space-2); border-radius: var(--radius-full);
             background: var(--surface-raised); border: 1px solid var(--border-subtle); }
  .pr-lado.ok { color: var(--success); }
  .pr-lado.nao { color: var(--error); }
  .corrige { margin-top: var(--space-3); padding: var(--space-3); background: var(--surface-card);
             border: 1px solid var(--border-default); border-left: 3px solid var(--warning);
             border-radius: var(--radius-md); }
  .corrige p { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--text-secondary);
               line-height: 1.45; }
  .corrige b { color: var(--text-primary); font-weight: 600; }
  .corrige input { width: 100%; height: 34px; padding: 0 var(--space-3);
                   background: var(--surface-inset); border: 1px solid var(--border-default);
                   border-radius: var(--radius-sm); color: var(--text-primary);
                   font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; }
  .acoes { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
  .btn { height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm);
         border: 1px solid var(--border-subtle); background: var(--surface-raised);
         color: var(--text-primary); font-size: var(--text-sm); font-family: inherit; }
  .btn.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }
  .pr-acoes { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
  .pr-btn.min { height: 30px; padding: 0 var(--space-3); font-size: var(--text-xs); }
  .pr-btn:disabled { opacity: 0.45; }

  .pr-form-campo { display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3); }
  .pr-form-rot { font-size: var(--text-xs); color: var(--text-secondary); }
  .pr-form-input {
    height: 40px; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); outline: none;
  }
  .pr-form-input:focus { border-color: var(--accent); }
  .pr-form-erro { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--error); }
</style>
