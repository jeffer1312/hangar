<script lang="ts">
  import { listServers, getActiveId, selectServer, renameServer, updateServer, removeServer,
           addServer, validarPareamento, onServersChanged } from '../../lib/auth';
  import { pushSupported } from '../../lib/push';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import ServerManager from '../ServerManager.svelte';
  import PushQuiet from '../PushQuiet.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import QrScanner from '../QrScanner.svelte';
  import type { Server } from '../../lib/auth';

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
    onPickTarget: (id: string) => void;
    onLogout: () => void | Promise<void>;
  }
  let { resolvedServer, apiTarget, onPickTarget, onLogout }: Props = $props();

  // lista reativa local: listServers() lê localStorage e não é reativo; o contador sobe pelo mesmo
  // onServersChanged que o App usa (o sync cross-aparelho também passa por ele).
  let serverVersion = $state(0);
  $effect(() => onServersChanged(() => serverVersion++));
  const servers = $derived.by(() => {
    serverVersion;   // dependência explícita do contador (listServers não é reativo)
    return listServers();
  });
  const activeId = $derived.by(() => {
    serverVersion;
    return getActiveId();
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
  function switchActive(id: string) {
    if (id === getActiveId()) return;
    selectServer(id);
    window.location.reload();
  }

  // Adicionar servidor: colar URL de pareamento (com token) ou QR — mesmo fluxo do Sidebar
  // (mesma rota de parse), com CSS local. O reload no sucesso é deliberado: a lista nova muda
  // ativo/token e o SSE do store precisa renascer limpo.
  //
  // Validação ESTRITA compartilhada (auth.validarPareamento, manual E QR — mesma função testável):
  // base absoluta http/https com hostname, api válida quando presente, token não vazio, URL sem
  // ?token= recusada — tudo ANTES de tocar storage. Gravar URL quebrada como credencial vira um
  // 401 sem pista depois.
  function erroPareamento(cru: string): string {
    if (!cru.includes('://')) return 'Cole a URL de pareamento (com o token).';
    if (cru.includes('?token=')) return 'URL de pareamento inválida — use http/https com token.';
    return 'essa URL não tem ?token= — cole a URL de pareamento completa.';
  }
  let showAdd = $state(false);
  let addUrlText = $state('');
  let addError = $state('');
  let scanning = $state(false);
  function autofocus(node: HTMLInputElement) { node.focus(); }
  function submitPasteServer() {
    const cru = addUrlText.trim();
    const parsed = validarPareamento(cru);
    if (!parsed) { addError = erroPareamento(cru); return; }
    addServer(parsed.base, parsed.token);
    window.location.reload();
  }
  function handleScan(text: string) {
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
    addServer(parsed.base, parsed.token);
    window.location.reload();
  }

  // Remoção com confirmação REAL (ConfirmDialog). O ÚLTIMO servidor é removível de propósito:
  // remover tudo dispara o logout global (única saída pra deslogar o aparelho) — por isso o
  // ServerManager recebe `podeRemoverUltimo`.
  let pendingRemoval = $state<{ id: string; label: string } | null>(null);
  let avisoRemocao = $state('');
  function confirmRemoval() {
    if (!pendingRemoval) return;
    const id = pendingRemoval.id;
    pendingRemoval = null;
    // Revalida o ID: o sync pode ter apagado este servidor entre o diálogo e o clique. Remover
    // calado um servidor que já não existe é mentira — mostra o estado e não faz nada.
    if (!servers.some((s) => s.id === id)) {
      avisoRemocao = 'Este servidor já foi removido em outro aparelho.';
      return;
    }
    avisoRemocao = '';
    const wasActive = id === getActiveId();
    removeServer(id);   // auth notifica onServersChanged -> contador local e store reagem
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
      logoutMsg = 'Não foi possível sair — tente de novo.';
    } finally {
      logoutInFlight = false;
    }
  }

  const pushTarget = $derived(
    !resolvedServer ? { mode: 'unavailable' } as const
    : apiTarget ? { mode: 'server', server: apiTarget } as const
    : { mode: 'global' } as const,
  );
</script>

{#if resolvedServer}
  <p class="ss-editando">Editando: <strong>{resolvedServer.label}</strong></p>
{:else}
  <p class="ss-editando ss-muted">Escolha “Editar” em um servidor para configurá-lo.</p>
{/if}
{#if avisoRemocao}<p class="ss-aviso" role="status">{avisoRemocao}</p>{/if}
{#if logoutMsg}<p class="ss-aviso" role="status">{logoutMsg}</p>{/if}

<ServerManager
  {servers}
  {activeId}
  targetId={resolvedServer?.id ?? null}
  {onPickTarget}
  onSwitchActive={switchActive}
  podeRemoverUltimo
  onRename={rename}
  onUpdateToken={updateToken}
  onRemove={(id) => {
    if (logoutInFlight) return;   // logout andando: portas de saída bloqueadas
    const s = servers.find((x) => x.id === id);
    pendingRemoval = { id, label: s?.label ?? id };
  }}
  onAdd={() => { showAdd = true; addUrlText = ''; addError = ''; }}
/>

{#if pushSupported()}
  <div class="ss-sep"></div>
  <PushQuiet target={pushTarget} open={true} />
{:else}
  <div class="ss-sep"></div>
  <p class="ss-muted">Notificações não estão disponíveis neste navegador.</p>
{/if}

<div class="ss-sep"></div>
<div class="ss-acoes">
  <button class="ss-btn" onclick={() => sessionsStore.reconnect()} disabled={logoutInFlight}>Reconectar</button>
  <button class="ss-btn ss-danger" onclick={() => (confirmLogout = true)} disabled={logoutInFlight}>Sair</button>
</div>

{#if showAdd}
  <ConfirmDialog title="Adicionar servidor" aria="Adicionar servidor" role="dialog"
    onClose={() => (showAdd = false)}
    actions={[
      { label: 'Escanear QR', onClick: () => { showAdd = false; scanning = true; } },
      { label: 'Adicionar', kind: 'primary', disabled: !addUrlText.trim(), onClick: submitPasteServer },
    ]}>
    <input
      type="url"
      class="ss-add-input"
      bind:value={addUrlText}
      placeholder="Colar URL do servidor (com token)"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      use:autofocus
      onkeydown={(e) => { addError = ''; if (e.key === 'Enter') submitPasteServer(); }}
      aria-label="URL de pareamento do servidor"
    />
    {#if addError}<p class="ss-add-err" role="alert">{addError}</p>{/if}
  </ConfirmDialog>
{/if}

{#if scanning}
  <QrScanner onScan={handleScan} onClose={() => (scanning = false)} />
{/if}

{#if pendingRemoval}
  <ConfirmDialog title={`Remover ${pendingRemoval.label}?`} aria="Confirmar remoção do servidor"
    onClose={() => (pendingRemoval = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (pendingRemoval = null) },
      { label: 'Remover', kind: 'danger', onClick: confirmRemoval },
    ]}>
    <p class="ss-dialog-copy">O token salvo neste aparelho será removido.</p>
  </ConfirmDialog>
{/if}

{#if confirmLogout}
  <ConfirmDialog title="Sair do Hangar?" aria="Confirmar saída"
    onClose={() => (confirmLogout = false)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmLogout = false) },
      { label: 'Sair', kind: 'danger', onClick: () => { confirmLogout = false; void logout(); } },
    ]}>
    <p class="ss-dialog-copy">Você precisará do token ou QR de pareamento para voltar.</p>
  </ConfirmDialog>
{/if}

<style>
  .ss-editando { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); }
  .ss-editando strong { color: var(--text-primary); font-weight: 600; }
  .ss-muted { color: var(--text-muted); }
  .ss-aviso { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--warning); }

  .ss-sep { height: 1px; background: var(--border-subtle); margin: var(--space-3) 0; }

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
</style>
