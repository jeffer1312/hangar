<script lang="ts">
  import { listServers, getActiveId, renameServer, updateServer, removeServer,
           addServerWithRollback, validarPareamento, onServersChanged, snapshotRemocao, removalStillMatches } from '../../lib/auth';
  import { getSessions } from '../../lib/api';
  import { getIdentificador, setIdentificador, listarPeers, gravarPeer, removerPeer,
           type PeerView } from '../../lib/peers';
  import { pushSupported } from '../../lib/push';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import ServerManager from '../ServerManager.svelte';
  import PushQuiet from '../PushQuiet.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import QrScanner from '../QrScanner.svelte';
  import type { RemovalSnapshot, Server } from '../../lib/auth';
  import * as m from '../../paraglide/messages';

  // Tela Servidores das Configurações (item C): controller LOCAL do CRUD de servidores, do alvo de
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

  // Adicionar servidor: colar URL de pareamento (com token) ou QR — mesmo fluxo do Sidebar
  // (mesma rota de parse), com CSS local. O reload no sucesso é deliberado: a lista nova muda
  // ativo/token e o SSE do store precisa renascer limpo.
  //
  // Validação ESTRITA compartilhada (auth.validarPareamento, manual E QR — mesma função testável):
  // base absoluta http/https com hostname, api válida quando presente, token não vazio, URL sem
  // ?token= recusada — tudo ANTES de tocar storage. Gravar URL quebrada como credencial vira um
  // 401 sem pista depois.
  function erroPareamento(cru: string): string {
    if (!cru.includes('://')) return m.config_servidores_erro_cole();
    if (cru.includes('?token=')) return m.config_servidores_erro_invalida();
    return m.config_servidores_erro_token();
  }
  let showAdd = $state(false);
  let addUrlText = $state('');
  let addError = $state('');
  // Transação em voo: o botão Add e o QR ficam desabilitados até o settle (sucesso ou rollback) —
  // o helper é serializado FIFO, mas um segundo clique não pode abrir outra tentativa por cima.
  let addBusy = $state(false);
  let scanning = $state(false);
  function autofocus(node: HTMLInputElement) { node.focus(); }
  async function submitPasteServer() {
    if (addBusy) return;   // guard: o botão é disabled, mas Enter chama o handler direto (round 6)
    const cru = addUrlText.trim();
    const parsed = validarPareamento(cru);
    if (!parsed) { addError = erroPareamento(cru); return; }
    // Add transacional: probe rejeitado não recarrega — erro visível e o diálogo fica pra retry
    // (o rollback escopado já rodou dentro do helper).
    addBusy = true;
    try {
      await addServerWithRollback(parsed.base, parsed.token, () => getSessions());
      window.location.reload();
    } catch (err) {
      // Erro TARDIO não some: o usuário pode ter fechado o diálogo enquanto a transação rodava —
      // reabre com a mensagem visível (mesmo caminho do QR logo abaixo).
      showAdd = true;
      addError = err instanceof Error ? `${m.falha_conexao()}: ${err.message}` : m.erro_desconhecido();
    } finally {
      addBusy = false;
    }
  }
  async function handleScan(text: string) {
    if (addBusy) return;   // guard: callback QR não pode iniciar transação por cima de outra (round 6)
    const cru = text.trim();
    const parsed = validarPareamento(cru);
    if (!parsed) {
      // QR inválido NÃO fecha silencioso: volta pro diálogo com o erro visível (role=alert) e o
      // usuário pode escanear de novo ou colar à mão.
      scanning = false;
      showAdd = true;
      addError = erroPareamento(cru);
      return;
    }
    scanning = false;
    addBusy = true;
    try {
      await addServerWithRollback(parsed.base, parsed.token, () => getSessions());
      window.location.reload();
    } catch (err) {
      showAdd = true;
      addError = err instanceof Error ? `${m.falha_conexao()}: ${err.message}` : m.erro_desconhecido();
    } finally {
      addBusy = false;
    }
  }

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

  const pushTarget = $derived(
    !resolvedServer ? { mode: 'unavailable' } as const
    : apiTarget ? { mode: 'server', server: apiTarget } as const
    : { mode: 'global' } as const,
  );

  // ── Seções da Task 5: identificador desta máquina + máquinas que este servidor alcança ───────
  // O backend/peers.json (lido também pelo cp-send) guarda id -> {base_url, token}; a rota
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

  $effect(() => {
    // Servidor indisponível (resolvedServer null): não há o que ler — sem este gate a seção lia
    // o servidor ATIVO com a aba dizendo que o escolhido não existe.
    if (!resolvedServer) {
      peers = []; identificador = ''; idOriginal = ''; idCarregado = true;
      return;
    }
    void carregarIdentificador();
    void carregarPeers();
  });

  async function carregarIdentificador() {
    idErro = '';
    try {
      const r = await getIdentificador(apiTarget);
      identificador = r.identificador;
      idOriginal = r.identificador;
    } catch (e) {
      idErro = msgErro(e);
    } finally {
      idCarregado = true;
    }
  }

  async function carregarPeers() {
    peersCarregando = true;
    peersErro = '';
    try {
      peers = await listarPeers(apiTarget);
    } catch (e) {
      peersErro = msgErro(e);
    } finally {
      peersCarregando = false;
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
    void setIdentificador(apiTarget, valor)
      .then((r) => { identificador = r.identificador; idOriginal = r.identificador; })
      .catch((e) => { idErro = msgErro(e); })
      .finally(() => { idSalvando = false; });
  }

  // Registrar um peer: id + endereço + token. A legenda do mock promete "as duas pontas" — o
  // lado remoto (testar/ligar) é da Task 8; aqui se grava o vínculo local, que é a fundação.
  let mostrandoRegistro = $state(false);
  let regId = $state('');
  let regUrl = $state('');
  let regToken = $state('');
  let regErro = $state('');
  let regSalvando = $state(false);

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
    try {
      peers = await gravarPeer(apiTarget, { id, base_url: url, token });
      mostrandoRegistro = false;
    } catch (e) {
      regErro = msgErro(e);
    } finally {
      regSalvando = false;
    }
  }

  let removerPeerId = $state<string | null>(null);
  async function removerPeerConfirmado() {
    const id = removerPeerId;
    removerPeerId = null;
    if (!id) return;
    peersErro = '';
    try {
      peers = await removerPeer(apiTarget, id);
    } catch (e) {
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

<ServerManager
  {servers}
  targetId={resolvedServer?.id ?? null}
  {onPickTarget}
  podeRemoverUltimo
  onRename={rename}
  onUpdateToken={updateToken}
  onRemove={abrirRemocao}
  onAdd={() => { showAdd = true; addUrlText = ''; addError = ''; }}
/>

{#if pushSupported()}
  <div class="ss-sep"></div>
  <!-- Bloco de natureza MISTA, e por isso a legenda existe: ativar o push é do aparelho (assina o
       navegador e registra em todos os servidores de uma vez), as horas silenciosas são do servidor
       escolhido acima. Sem dizer isso, a tela deixava as duas parecendo a mesma coisa. -->
  <p class="ss-secao">{m.config_modal_notificacoes()}</p>
  <p class="ss-legenda">
    {m.config_servidores_push_1()} {resolvedServer ? m.config_modal_em({ nome: resolvedServer.label }) : m.config_servidores_push_global()}{m.comum_ponto()}
  </p>
  <PushQuiet target={pushTarget} open={true} />
{:else}
  <div class="ss-sep"></div>
  <p class="ss-muted">{m.config_servidores_sem_push()}</p>
{/if}

<div class="ss-sep"></div>
<!-- Fronteira explícita: daqui pra baixo NADA vai pro servidor escolhido acima. Reconectar refaz as
     conexões deste aparelho e Sair apaga os tokens guardados AQUI — em todos os servidores. Era o
     que mais confundia: a tela misturava as duas coisas sem dizer qual era qual. -->
<p class="ss-secao">{m.config_servidores_neste_aparelho()}</p>
<div class="ss-acoes">
  <button class="ss-btn" onclick={() => sessionsStore.reconnect()} disabled={logoutInFlight}>{m.config_servidores_reconectar()}</button>
  <button class="ss-btn ss-danger" onclick={() => (confirmLogout = true)} disabled={logoutInFlight}>{m.sessao_sair_curto()}</button>
</div>

<div class="ss-sep"></div>
<!-- Identificador desta máquina (Task 5): é o CP_SERVER_ID, gravado no .env — o mesmo que o
     cp-send usa no endereço de resposta srv::sessao. Vazio = pareamento entre servidores recusado. -->
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

<div class="ss-sep"></div>
<!-- Máquinas que este servidor alcança (Task 5): a lista do peers.json. O estado das duas pontas
     (testar/liga) é da Task 8 — aqui se mostra e se edita o vínculo local. -->
<p class="ss-secao">{m.peers_secao_alcance()}</p>
{#if peers.length}
  <p class="ss-legenda">{m.peers_legenda_alcance()}</p>
  <div class="pr-cartao">
    {#each peers as peer (peer.id)}
      <div class="pr-linha">
        <span class="pr-txt">
          <span class="pr-nome">{peer.id}</span>
          <span class="pr-url">{peer.base_url}</span>
        </span>
        <button class="pr-btn min" onclick={() => (removerPeerId = peer.id)}>{m.peers_remover()}</button>
      </div>
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

{#if showAdd}
  <ConfirmDialog title={m.sessao_adicionar_servidor()} aria={m.sessao_adicionar_servidor()} role="dialog"
    {fallbackFocus}
    onClose={() => (showAdd = false)}
    actions={[
      { label: m.sessao_escanear_qr(), disabled: addBusy, onClick: () => { showAdd = false; scanning = true; } },
      { label: m.config_servidores_adicionar(), kind: 'primary', disabled: !addUrlText.trim() || addBusy, onClick: submitPasteServer },
    ]}>
    <input
      type="url"
      class="ss-add-input"
      bind:value={addUrlText}
      placeholder={m.config_servidores_colar_url()}
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      use:autofocus
      onkeydown={(e) => { addError = ''; if (e.key === 'Enter' && !addBusy) submitPasteServer(); }}
      disabled={addBusy}
      aria-label={m.config_servidores_url_pareamento()}
      aria-invalid={!!addError}
      aria-describedby={addError ? 'ss-add-err' : undefined}
    />
    {#if addError}<p id="ss-add-err" class="ss-add-err" role="alert">{addError}</p>{/if}
  </ConfirmDialog>
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

{#if scanning}
  <QrScanner onScan={handleScan} onClose={() => (scanning = false)} />
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

  /* Input do modal "Adicionar servidor" (mesmo desenho do Sidebar, CSS local). */
  .ss-add-input {
    width: 100%; height: 44px; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-primary); font-family: var(--font-ui); font-size: var(--text-sm); outline: none;
  }
  .ss-add-input::placeholder { color: var(--text-muted); }
  .ss-add-input:focus { border-color: var(--accent); }
  .ss-add-err { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--error); }
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
  .pr-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  .pr-nome { font-size: var(--text-sm); color: var(--text-primary); }
  .pr-url { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
            word-break: break-all; }
  .pr-acoes { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
  .pr-btn {
    height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-primary); font-size: var(--text-sm); font-family: inherit;
  }
  .pr-btn.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }
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
