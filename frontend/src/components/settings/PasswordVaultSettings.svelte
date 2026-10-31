<script lang="ts">
  import { untrack } from 'svelte';
  import BottomSheet from '../BottomSheet.svelte';
  import ConfirmSheet from '../ConfirmSheet.svelte';
  import * as m from '../../paraglide/messages';
  import { copyText } from '../../lib/clipboard';
  import { listOwnServers } from '../../lib/auth';
  import {
    addVaultEntry, deleteVaultEntry, findOwnerServer, getVaultStatus, listVaultEntries, listVaultPeers, putVaultConfig, restoreVault,
    revealVaultPassword, setVaultRecovery, updateVaultEntry, vaultFailureText,
    type Server, type VaultEntry, type VaultPeer, type VaultPolicy, type VaultStatus,
  } from '@hangar/core';

  let { server }: { server: Server } = $props();

  let status = $state<VaultStatus | null>(null);
  let statusError = $state<string | null>(null);
  let entries = $state<VaultEntry[] | null>(null);
  let entriesError = $state<string | null>(null);
  let notice = $state<string | null>(null);
  // Senha revelada por entrada; mora só nesta tela e some ao esconder ou sair.
  let revealed = $state<Record<string, string>>({});
  let busyId = $state<string | null>(null);

  const reason = (err: unknown) => vaultFailureText(err) ?? (err instanceof Error ? err.message : String(err));

  // Só a leitura mais nova vale: resposta atrasada de outra rodada não sobrescreve a tela.
  let loadSeq = 0;
  async function load() {
    const seq = ++loadSeq;
    statusError = null;
    entriesError = null;
    // Lista das outras máquinas já em paralelo: serve às escolhas e, sem `owner_url`, a achar a dona.
    void loadPeers();
    const [st, list] = await Promise.allSettled([getVaultStatus(server), listVaultEntries(server)]);
    if (seq !== loadSeq) return;
    if (st.status === 'rejected') {
      status = null;
      clearRevealed();
      statusError = reason(st.reason);
      return;
    }
    status = st.value;
    if (status.state !== 'open') { entries = null; clearRevealed(); return; }
    if (list.status === 'rejected') { entries = null; entriesError = reason(list.reason); }
    else if (!Array.isArray(list.value)) { entries = null; entriesError = m.vault_web_bad_answer(); }
    else {
      // A senha pode ter mudado em outro aparelho: a revelada aqui ficaria velha.
      clearRevealed();
      entries = list.value;
    }
  }
  // `untrack`: o `load` lê e grava o estado dos peers, e o efeito se reagendaria sozinho.
  $effect(() => { untrack(() => void load()); });

  // A senha revelada não fica na tela de um app que foi para o fundo.
  $effect(() => {
    const onChange = () => { if (document.hidden) clearRevealed(); };
    document.addEventListener('visibilitychange', onChange);
    return () => document.removeEventListener('visibilitychange', onChange);
  });

  // Quem recebe as escritas e a leitura da senha: a dona. Numa máquina que aponta para outra, a
  // lista vem encaminhada, mas o resto só vai à dona, achada entre os servidores salvos neste aparelho.
  const writer = $derived.by((): Server | null => {
    if (status?.role === 'owner') return server;
    if (status?.role !== 'remote') return null;
    const owner = status.owner;
    const url = status.owner_url ?? peers?.find((p) => p.id === owner)?.base_url;
    return url ? findOwnerServer(url, listOwnServers()) : null;
  });
  // Sem `owner_url` a dona só aparece quando a lista de máquinas chega.
  const resolving = $derived.by(() => status?.role === 'remote' && !status.owner_url && peersLoading);

  // ── Onde fica o cofre ──────────────────────────────────────────────────────────────────────
  let configuring = $state(false);
  let configError = $state<string | null>(null);
  // Máquinas do `peers.json` desta: candidatas a dona e a guardar a cópia.
  let peers = $state<VaultPeer[] | null>(null);
  let peersFor = $state<'owner' | 'backup' | 'restore' | null>(null);
  let peersLoading = $state(false);
  let peersError = $state<string | null>(null);

  async function loadPeers() {
    if (peers || peersLoading) return;
    peersLoading = true;
    peersError = null;
    try {
      peers = await listVaultPeers(server);
    } catch (err) {
      peersError = m.vault_web_peers_failed({ error: reason(err) });
    } finally {
      peersLoading = false;
    }
  }

  function showPeers(purpose: 'owner' | 'backup' | 'restore') {
    peersFor = purpose;
    void loadPeers();
  }

  async function setConfig(change: { owner: string } | { backup_peer: string | null }) {
    configuring = true;
    configError = null;
    try {
      await putVaultConfig(server, change);
      peersFor = null;
      await load();
    } catch (err) {
      configError = reason(err);
    } finally {
      configuring = false;
    }
  }
  const setOwner = (owner: string) => setConfig({ owner });

  // ── Cópia de segurança e restauração ────────────────────────────────────────────────────────
  // A senha de recuperação nunca fica no estado depois do envio.
  const RECOVERY_MIN = 12;
  let rPassword = $state('');
  let rConfirm = $state('');
  let rVisible = $state(false);
  let rSaving = $state(false);
  let rError = $state<string | null>(null);
  let rDone = $state(false);
  const rMismatch = $derived(!!rConfirm && rPassword !== rConfirm);
  // Conta caracteres, não unidades UTF-16: o servidor mede o mesmo.
  const rLength = $derived([...rPassword].length);
  const rCanSave = $derived(rLength >= RECOVERY_MIN && rPassword === rConfirm);

  async function saveRecovery(e: SubmitEvent) {
    e.preventDefault();
    if (rSaving || !rCanSave) return;
    const typed = rPassword;
    rPassword = '';
    rConfirm = '';
    rSaving = true;
    rError = null;
    rDone = false;
    try {
      await setVaultRecovery(server, typed);
      rDone = true;
      await load();
    } catch (err) {
      rError = reason(err);
    } finally {
      rSaving = false;
      rVisible = false;
    }
  }

  let restorePeer = $state<string | null>(null);
  let restoreFrom = $state('');
  let restorePassword = $state('');
  let restoring = $state(false);
  let restoreError = $state<string | null>(null);

  async function restore(e: SubmitEvent) {
    e.preventDefault();
    if (restoring || !restorePeer || !restorePassword) return;
    const typed = restorePassword;
    restorePassword = '';
    const from = restoreFrom.trim();
    restoring = true;
    restoreError = null;
    try {
      await restoreVault(server, typed, restorePeer, from || undefined);
      restorePeer = null;
      await load();
    } catch (err) {
      restoreError = reason(err);
      // O cofre voltou, só a senha de recuperação faltou: a tela mostra o cofre aberto.
      if ((err as { code?: string })?.code === 'erro_cofre_recuperacao_nao_gravada') await load();
    } finally {
      restoring = false;
    }
  }

  const when = (at: number) => new Date(at * 1000).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
  // Chave perdida na dona também restaura: a cópia traz o cofre de volta com chave nova.
  const canRestore = $derived(!!status && status.role !== 'remote' &&
    (status.state === 'absent' || (status.role === 'owner' && status.state === 'key_lost')
      || (status.state === 'open' && entries?.length === 0)));

  // ── Revelar e copiar ────────────────────────────────────────────────────────────────────────
  // Uma marca por busca em voo: `forget` a apaga, e a resposta que chegar depois é descartada.
  const revealTokens = new Map<string, object>();
  // Senha já buscada à espera do segundo toque em Copiar; não aparece na tela.
  let copyReady: { id: string; password: string } | null = null;

  async function fetchPassword(id: string): Promise<string | null> {
    const w = writer;
    if (!w) return null;
    const token = {};
    revealTokens.set(id, token);
    busyId = id;
    try {
      const answer = await revealVaultPassword(w, id);
      if (revealTokens.get(id) !== token) return null;
      if (typeof answer?.password === 'string') return answer.password;
      notice = m.vault_web_reveal_failed({ error: m.vault_web_bad_answer() });
      return null;
    } catch (err) {
      if (revealTokens.get(id) === token) notice = m.vault_web_reveal_failed({ error: reason(err) });
      return null;
    } finally {
      if (revealTokens.get(id) === token) revealTokens.delete(id);
      busyId = null;
    }
  }

  function forget(id: string) {
    revealTokens.delete(id);
    if (copyReady?.id === id) copyReady = null;
    const next = { ...revealed };
    delete next[id];
    revealed = next;
  }

  function clearRevealed() {
    revealTokens.clear();
    copyReady = null;
    revealed = {};
  }

  async function toggleReveal(id: string) {
    if (revealed[id] !== undefined) { forget(id); return; }
    const pw = await fetchPassword(id);
    if (pw !== null) revealed = { ...revealed, [id]: pw };
  }

  async function copy(id: string) {
    notice = null;
    const ready = revealed[id] ?? (copyReady?.id === id ? copyReady.password : undefined);
    const pw = ready ?? await fetchPassword(id);
    if (pw === null) return;
    if (await copyText(pw)) { copyReady = null; notice = m.vault_web_copied(); return; }
    // Depois de buscar a senha o navegador pode recusar a cópia fora do toque: o segundo toque copia direto.
    if (ready === undefined) { copyReady = { id, password: pw }; notice = m.vault_web_copy_again(); return; }
    copyReady = null;
    notice = m.vault_web_copy_failed();
  }

  // ── Cadastrar e editar ──────────────────────────────────────────────────────────────────────
  let editing = $state<VaultEntry | 'new' | null>(null);
  let fName = $state('');
  let fUser = $state('');
  let fPassword = $state('');
  let fUrl = $state('');
  let fNotes = $state('');
  let fPolicy = $state<VaultPolicy>('ask');
  let fVisible = $state(false);
  let saving = $state(false);
  let formError = $state<string | null>(null);
  // Muda a cada abrir e fechar: o fim de um envio não mexe num formulário aberto depois dele.
  let formToken = 0;

  function openForm(e: VaultEntry | 'new') {
    formToken++;
    editing = e;
    const cur = e === 'new' ? null : e;
    fName = cur?.name ?? '';
    fUser = cur?.username ?? '';
    fUrl = cur?.url ?? '';
    fNotes = cur?.notes ?? '';
    fPolicy = cur?.policy ?? 'ask';
    fPassword = '';
    fVisible = false;
    formError = null;
  }
  function closeForm() { formToken++; editing = null; fPassword = ''; formError = null; }

  const canSave = $derived(!!fName.trim() && (editing !== 'new' || !!fPassword));

  async function saveForm(e: SubmitEvent) {
    e.preventDefault();
    const w = writer;
    if (!editing || saving || !canSave || !w) return;
    const fields = {
      name: fName.trim(),
      username: fUser.trim() || null,
      url: fUrl.trim() || null,
      notes: fNotes.trim() || null,
      policy: fPolicy,
    };
    const typed = fPassword;
    const target = editing;
    const token = formToken;
    saving = true;
    formError = null;
    try {
      if (target === 'new') await addVaultEntry(w, { ...fields, password: typed });
      else {
        await updateVaultEntry(w, target.id, typed ? { ...fields, password: typed } : fields);
        forget(target.id);
      }
      if (token === formToken) closeForm();
      await load();
    } catch (err) {
      // Formulário fechado durante o envio: a falha aparece na lista, nunca some.
      if (token === formToken) formError = reason(err);
      else notice = reason(err);
    } finally {
      saving = false;
    }
  }

  // ── Apagar ──────────────────────────────────────────────────────────────────────────────────
  let deleting = $state<VaultEntry | null>(null);
  async function confirmDelete() {
    const e = deleting;
    const w = writer;
    if (!e || !w) return;
    notice = null;
    try {
      await deleteVaultEntry(w, e.id);
      forget(e.id);
      await load();
    } catch (err) {
      notice = m.vault_web_delete_failed({ error: reason(err) });
    }
  }

  const where = $derived(!status ? '' :
    status.role === 'owner' ? m.vault_web_where_self() :
    status.role === 'remote' ? m.vault_web_where_remote({ owner: status.owner ?? '' }) :
    m.vault_web_where_none());
</script>

<div class="pv">
  {#if !status && !statusError}
    <p class="pv-muted" role="status">{m.vault_web_loading()}</p>
  {:else if statusError}
    <div class="pv-box pv-erro" role="alert">
      <p>{m.vault_web_status_failed({ error: statusError })}</p>
      <button type="button" class="pv-btn" onclick={load}>{m.vault_web_retry()}</button>
    </div>
  {:else if status}
    <p class="pv-secao">{m.vault_web_where_title()}</p>
    <div class="pv-box">
      <p class="pv-where">{where}</p>
      {#if status.state === 'locked'}
        <p class="pv-aviso" role="status">{m.vault_web_state_locked()}</p>
      {:else if status.state === 'key_lost'}
        <p class="pv-erro-txt" role="alert">{m.vault_web_state_key_lost()}</p>
      {:else if status.state === 'unavailable'}
        <p class="pv-erro-txt" role="alert">{status.detail ?? m.vault_err_chaveiro_indisponivel()}</p>
      {:else if status.state === 'offline'}
        <p class="pv-aviso" role="status">{m.vault_web_state_offline({ owner: status.owner ?? '', detail: status.detail ?? '' })}</p>
      {:else if status.state === 'absent' && status.role !== 'none'}
        <p class="pv-aviso" role="status">{status.detail ?? m.vault_web_state_absent()}</p>
      {/if}
      <div class="pv-acoes">
        {#if status.role !== 'owner'}
          <!-- Com dona remota, virar dona deixaria as senhas dela fora do alcance desta máquina. -->
          {#if status.role === 'none'}
            <button type="button" class="pv-btn" disabled={configuring} onclick={() => setOwner('self')}>{m.vault_web_make_owner()}</button>
          {/if}
          <button type="button" class="pv-btn" disabled={configuring || peersLoading} onclick={() => showPeers('owner')}>
            {m.vault_web_choose_owner()}
          </button>
        {/if}
        {#if status.state !== 'open'}
          <button type="button" class="pv-btn" onclick={load}>{m.vault_web_retry()}</button>
        {/if}
      </div>
      {#if peersFor === 'owner'}{@render peerChoice((id) => setOwner(id), status.owner ?? null)}{/if}
      {#if configError && peersFor !== 'backup'}<p class="pv-erro-txt" role="alert">{configError}</p>{/if}
    </div>

    {#if status.role === 'owner' && status.state === 'open'}
      <p class="pv-secao">{m.vault_web_backup_title()}</p>
      <div class="pv-box">
        <p class="pv-where">
          {status.backup?.peer ? m.vault_web_backup_peer({ peer: status.backup.peer }) : m.vault_web_backup_none()}
        </p>
        {#if status.backup?.last_ok_at}
          <p class="pv-muted">{m.vault_web_backup_last_ok({ when: when(status.backup.last_ok_at) })}</p>
        {:else if status.backup?.peer}
          <p class="pv-muted">{m.vault_web_backup_never()}</p>
        {/if}
        {#if status.backup?.last_error}
          <p class="pv-erro-txt" role="alert">{m.vault_web_backup_last_error({ error: status.backup.last_error })}</p>
        {/if}
        <div class="pv-acoes">
          <button type="button" class="pv-btn" disabled={configuring || peersLoading} onclick={() => showPeers('backup')}>
            {status.backup?.peer ? m.vault_web_backup_change() : m.vault_web_backup_choose()}
          </button>
        </div>
        {#if peersFor === 'backup'}
          {@render peerChoice((id) => setConfig({ backup_peer: id }), status.backup?.peer ?? null)}
          {#if status.backup?.peer}
            <div class="pv-acoes">
              <button type="button" class="pv-btn" disabled={configuring} onclick={() => setConfig({ backup_peer: null })}>{m.vault_web_backup_clear()}</button>
            </div>
          {/if}
        {/if}
        {#if configError && peersFor === 'backup'}<p class="pv-erro-txt" role="alert">{configError}</p>{/if}

        <form class="pv-form-inline" onsubmit={saveRecovery}>
          <p class="pv-campo-tit">{m.vault_web_recovery_title()}</p>
          <p class="pv-aviso">{m.vault_web_recovery_warning()}</p>
          <label class="pv-campo"><span>{m.vault_web_recovery_password()}</span>
            <span class="pv-pw">
              <input type={rVisible ? 'text' : 'password'} autocomplete="off" data-1p-ignore data-lpignore="true" autocapitalize="off" spellcheck="false"
                bind:value={rPassword} oninput={() => { rError = null; rDone = false; }} disabled={rSaving} />
              <button type="button" class="pv-reveal" onclick={() => (rVisible = !rVisible)} disabled={rSaving}
                aria-label={rVisible ? m.vault_web_hide_password() : m.vault_web_show_password()} aria-pressed={rVisible}>
                <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
                  <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z" /><circle cx="12" cy="12" r="3" />
                  {#if rVisible}<path d="M3 3l18 18" />{/if}
                </svg>
              </button>
            </span></label>
          <label class="pv-campo"><span>{m.vault_web_recovery_confirm()}</span>
            <input type={rVisible ? 'text' : 'password'} autocomplete="off" data-1p-ignore data-lpignore="true" autocapitalize="off" spellcheck="false"
              bind:value={rConfirm} disabled={rSaving} aria-invalid={rMismatch} /></label>
          {#if rPassword && rLength < RECOVERY_MIN}
            <p class="pv-muted">{m.vault_web_recovery_short({ n: RECOVERY_MIN })}</p>
          {:else if rMismatch}
            <p class="pv-erro-txt">{m.vault_web_recovery_mismatch()}</p>
          {/if}
          {#if rError}<p class="pv-erro-txt" role="alert">{m.vault_web_recovery_failed({ error: rError })}</p>{/if}
          {#if rDone}<p class="pv-muted" role="status">{m.vault_web_recovery_saved()}</p>{/if}
          <div class="pv-acoes">
            <button type="submit" class="pv-btn pv-primario" disabled={rSaving || !rCanSave}>
              {rSaving ? m.vault_web_saving() : m.vault_web_recovery_save()}
            </button>
          </div>
        </form>
      </div>
    {/if}

    {#if canRestore}
      <p class="pv-secao">{m.vault_web_restore_title()}</p>
      <form class="pv-box" onsubmit={restore}>
        <p class="pv-muted">{m.vault_web_restore_hint()}</p>
        <div class="pv-acoes">
          <button type="button" class="pv-btn" disabled={restoring || peersLoading} onclick={() => showPeers('restore')}>
            {restorePeer ? m.vault_web_restore_from({ peer: restorePeer }) : m.vault_web_restore_choose()}
          </button>
        </div>
        {#if peersFor === 'restore'}{@render peerChoice((id) => { restorePeer = id; peersFor = null; }, restorePeer)}{/if}
        <label class="pv-campo"><span>{m.vault_web_restore_server_id()}</span>
          <input type="text" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={restoreFrom}
            placeholder={status.server_id ?? ''} disabled={restoring} /></label>
        <label class="pv-campo"><span>{m.vault_web_recovery_password()}</span>
          <input type="password" autocomplete="off" data-1p-ignore data-lpignore="true" autocapitalize="off" spellcheck="false"
            bind:value={restorePassword} oninput={() => (restoreError = null)} disabled={restoring} /></label>
        {#if restoreError}<p class="pv-erro-txt" role="alert">{restoreError}</p>{/if}
        <div class="pv-acoes">
          <button type="submit" class="pv-btn pv-primario" disabled={restoring || !restorePeer || !restorePassword}>
            {restoring ? m.vault_web_restoring() : m.vault_web_restore()}
          </button>
        </div>
      </form>
    {/if}

    {#if status.state === 'open'}
      <div class="pv-topo">
        <p class="pv-secao">{m.vault_web_entries_title()}</p>
        {#if writer}<button type="button" class="pv-btn pv-primario" onclick={() => openForm('new')}>{m.vault_web_add()}</button>{/if}
      </div>
      {#if !writer && !resolving}<p class="pv-muted" role="status">{m.vault_web_read_only({ owner: status.owner ?? '' })}</p>{/if}
      {#if notice}<p class="pv-muted" role="status">{notice}</p>{/if}
      {#if entriesError}
        <div class="pv-box pv-erro" role="alert">
          <p>{m.vault_web_entries_failed({ error: entriesError })}</p>
          <button type="button" class="pv-btn" onclick={load}>{m.vault_web_retry()}</button>
        </div>
      {:else if !entries}
        <p class="pv-muted" role="status">{m.vault_web_loading()}</p>
      {:else if entries.length === 0}
        <p class="pv-muted">{m.vault_web_empty()}</p>
      {:else}
        <ul class="pv-lista">
          {#each entries as e (e.id)}
            <li class="pv-item">
              <div class="pv-item-txt">
                <span class="pv-nome">{e.name}</span>
                {#if e.username || e.url}
                  <span class="pv-desc">{[e.username, e.url].filter(Boolean).join(' · ')}</span>
                {/if}
                <span class="pv-tag">{e.policy === 'free' ? m.vault_web_policy_free() : m.vault_web_policy_ask()}</span>
                {#if revealed[e.id] !== undefined}<code class="pv-senha">{revealed[e.id]}</code>{/if}
              </div>
              {#if writer}<div class="pv-item-acoes">
                <button type="button" class="pv-btn" disabled={busyId === e.id} onclick={() => toggleReveal(e.id)}>
                  {revealed[e.id] !== undefined ? m.vault_web_hide() : m.vault_web_reveal()}
                </button>
                <button type="button" class="pv-btn" disabled={busyId === e.id} onclick={() => copy(e.id)}>{m.vault_web_copy()}</button>
                <button type="button" class="pv-btn" onclick={() => openForm(e)}>{m.vault_web_edit()}</button>
                <button type="button" class="pv-btn pv-perigo" onclick={() => (deleting = e)}>{m.vault_web_delete()}</button>
              </div>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  {/if}
</div>

{#snippet peerChoice(pick: (id: string) => void, current: string | null)}
  {#if peersLoading}
    <p class="pv-muted" role="status">{m.vault_web_loading()}</p>
  {:else if peersError}
    <p class="pv-erro-txt" role="alert">{peersError}</p>
  {:else if peers && peers.length === 0}
    <p class="pv-muted">{m.vault_web_no_peers()}</p>
  {:else if peers}
    <div class="pv-acoes">
      {#each peers as p (p.id)}
        <button type="button" class="pv-btn" class:pv-atual={p.id === current} aria-pressed={p.id === current}
          disabled={configuring} onclick={() => pick(p.id)}>{p.id}</button>
      {/each}
    </div>
  {/if}
{/snippet}

<BottomSheet open={!!editing} onClose={closeForm} ariaLabel={editing === 'new' ? m.vault_web_add_title() : m.vault_web_edit_title()}>
  {#if editing}
    <form class="pv-form" onsubmit={saveForm}>
      <h2 class="pv-form-titulo">{editing === 'new' ? m.vault_web_add_title() : m.vault_web_edit_title()}</h2>
      <p class="pv-muted">{m.vault_token_hint()}</p>
      <label class="pv-campo"><span>{m.vault_web_name()}</span>
        <input type="text" autocomplete="off" bind:value={fName} disabled={saving} required /></label>
      <label class="pv-campo"><span>{m.vault_web_username()}</span>
        <input type="text" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={fUser} disabled={saving} /></label>
      <label class="pv-campo"><span>{editing === 'new' ? m.vault_web_password() : m.vault_web_password_keep()}</span>
        <span class="pv-pw">
          <input type={fVisible ? 'text' : 'password'} autocomplete="off" data-1p-ignore data-lpignore="true" autocapitalize="off" spellcheck="false"
            bind:value={fPassword} disabled={saving} />
          <button type="button" class="pv-reveal" onclick={() => (fVisible = !fVisible)} disabled={saving}
            aria-label={fVisible ? m.vault_web_hide_password() : m.vault_web_show_password()} aria-pressed={fVisible}>
            <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
              <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z" /><circle cx="12" cy="12" r="3" />
              {#if fVisible}<path d="M3 3l18 18" />{/if}
            </svg>
          </button>
        </span></label>
      <label class="pv-campo"><span>{m.vault_web_site()}</span>
        <!-- `type="text"`: `type="url"` faria o navegador recusar `auth.exemplo.com` sem esquema. -->
        <input type="text" inputmode="url" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={fUrl} disabled={saving} /></label>
      <label class="pv-campo"><span>{m.vault_web_notes()}</span>
        <textarea rows="3" bind:value={fNotes} disabled={saving}></textarea></label>
      <fieldset class="pv-politica" disabled={saving}>
        <legend>{m.vault_web_policy()}</legend>
        <label><input type="radio" bind:group={fPolicy} value="ask" />{m.vault_web_policy_ask()}</label>
        <label><input type="radio" bind:group={fPolicy} value="free" />{m.vault_web_policy_free()}</label>
      </fieldset>
      {#if formError}<p class="pv-erro-txt" role="alert">{formError}</p>{/if}
      <div class="pv-rodape">
        <button type="button" class="pv-btn" onclick={closeForm} disabled={saving}>{m.comum_cancelar()}</button>
        <button type="submit" class="pv-btn pv-primario" disabled={saving || !canSave}>{saving ? m.vault_web_saving() : m.vault_web_save()}</button>
      </div>
    </form>
  {/if}
</BottomSheet>

<ConfirmSheet open={!!deleting} danger
  title={m.vault_web_delete_title({ name: deleting?.name ?? '' })}
  message={m.vault_web_delete_message()}
  confirmLabel={m.vault_web_delete()}
  onConfirm={confirmDelete} onClose={() => (deleting = null)} />

<style>
  /* Container query: quem aperta a linha é a largura do painel, não a da janela. */
  .pv { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-2); }
  .pv-secao {
    margin: var(--space-3) 0 0 var(--space-2); color: var(--text-muted);
    font-size: var(--label-size); font-weight: var(--label-weight);
    text-transform: uppercase; letter-spacing: var(--label-tracking);
  }
  .pv-topo { display: flex; align-items: flex-end; justify-content: space-between; gap: var(--space-2); }
  .pv-box {
    display: flex; flex-direction: column; gap: var(--space-2);
    padding: var(--space-3); background: var(--surface-card);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .pv-box p { margin: 0; }
  .pv-erro { border-left: 3px solid var(--error); }
  .pv-where { font-size: var(--text-sm); color: var(--text-primary); }
  .pv-muted { margin: 0; color: var(--text-muted); font-size: var(--text-xs); }
  .pv-aviso { color: var(--warning); font-size: var(--text-xs); }
  .pv-erro-txt { margin: 0; color: var(--error); font-size: var(--text-xs); }
  .pv-acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .pv-btn {
    min-height: 36px; padding: 6px 12px; border-radius: 8px;
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    color: var(--text-primary); font: inherit; font-size: var(--text-xs); cursor: pointer;
  }
  .pv-btn:disabled { opacity: 0.5; cursor: default; }
  .pv-primario { background: var(--accent); border-color: var(--accent); color: #0b0e13; }
  .pv-perigo { color: var(--error); }
  .pv-atual { border-color: var(--accent); }
  .pv-form-inline { display: flex; flex-direction: column; gap: var(--space-2); margin-top: var(--space-2); }
  .pv-campo-tit { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .pv-lista {
    list-style: none; margin: 0; padding: 0;
    background: var(--surface-card); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); overflow: hidden;
  }
  .pv-item {
    display: flex; align-items: center; gap: var(--space-3);
    padding: var(--space-3); border-bottom: 1px solid var(--border-subtle);
  }
  .pv-item:last-child { border-bottom: none; }
  .pv-item-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .pv-nome { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); overflow-wrap: anywhere; }
  .pv-desc { font-size: var(--text-xs); color: var(--text-muted); overflow-wrap: anywhere; }
  .pv-tag { font-size: 11px; color: var(--text-secondary); }
  .pv-senha {
    margin-top: 2px; padding: 4px 6px; border-radius: 6px; background: var(--surface-inset);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-primary);
    overflow-wrap: anywhere; user-select: all;
  }
  .pv-item-acoes { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-1); }
  @container (max-width: 460px) {
    .pv-item { flex-direction: column; align-items: stretch; }
    .pv-item-acoes { justify-content: flex-start; }
    .pv-btn { min-height: 44px; }
  }

  .pv-form { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0 var(--space-3); }
  .pv-form-titulo { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .pv-campo { display: flex; flex-direction: column; gap: 4px; }
  .pv-campo > span:first-child { font-size: 11.5px; color: var(--text-secondary); }
  .pv-campo > input, .pv-campo > textarea {
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    border-radius: 8px; color: var(--text-primary); padding: 9px;
    font: inherit; font-size: 16px; width: 100%;
  }
  .pv-pw {
    display: flex; align-items: center;
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: 8px;
  }
  .pv-pw:focus-within { border-color: var(--accent); }
  .pv-pw input {
    flex: 1; min-width: 0; height: 44px; border: 0; background: transparent;
    color: var(--text-primary); font: inherit; font-size: 16px; padding: 0 9px; outline: none;
  }
  .pv-reveal {
    width: 44px; height: 44px; flex-shrink: 0; display: grid; place-items: center;
    border: 0; background: transparent; color: var(--text-muted); cursor: pointer;
  }
  .pv-reveal svg { fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .pv-politica { margin: 0; padding: 0; border: 0; display: flex; flex-wrap: wrap; gap: var(--space-3); font-size: var(--text-sm); color: var(--text-primary); }
  .pv-politica legend { padding: 0; margin-bottom: 4px; font-size: 11.5px; color: var(--text-secondary); }
  .pv-politica label { display: flex; align-items: center; gap: var(--space-1); min-height: 44px; }
  .pv-politica input { width: 20px; height: 20px; accent-color: var(--accent); }
  .pv-rodape { display: flex; gap: var(--space-2); justify-content: flex-end; }
  .pv-rodape .pv-btn { min-height: 44px; font-size: var(--text-sm); }
</style>
