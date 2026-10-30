<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import ConfirmSheet from '../ConfirmSheet.svelte';
  import * as m from '../../paraglide/messages';
  import { copyText } from '../../lib/clipboard';
  import { listOwnServers } from '../../lib/auth';
  import {
    addVaultEntry, deleteVaultEntry, getVaultStatus, listVaultEntries, listVaultPeers, putVaultConfig, restoreVault,
    revealVaultPassword, setVaultRecovery, updateVaultEntry,
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

  const codeOf = (err: unknown) => (err as { code?: string })?.code;
  const reason = (err: unknown) => {
    const code = codeOf(err);
    if (code === 'erro_cofre_ausente') return m.vault_web_error_absent();
    if (code === 'erro_cofre_dona_remota') return m.vault_web_error_owner_remote();
    return err instanceof Error ? err.message : String(err);
  };

  // Quem recebe as escritas e a leitura da senha: a dona. Numa máquina que aponta para outra, a
  // lista vem encaminhada, mas o resto só vai à dona, achada entre os servidores salvos neste aparelho.
  let writer = $state<Server | null>(null);

  function hostKey(url: string): string {
    try {
      const u = new URL(url.trim());
      return `${u.host}${u.pathname.replace(/\/+$/, '')}`.toLowerCase();
    } catch {
      return url.trim().replace(/\/+$/, '').toLowerCase();
    }
  }

  let resolving = $state(false);
  async function resolveWriter(owner: string | undefined) {
    if (!owner) return;
    resolving = true;
    const ok = await loadPeers();
    resolving = false;
    const peer = ok ? peers?.find((p) => p.id === owner) : undefined;
    if (!peer) return;
    const want = hostKey(peer.base_url);
    writer = listOwnServers().find((s) => hostKey(s.baseUrl) === want) ?? null;
  }

  async function load() {
    statusError = null;
    entriesError = null;
    try {
      status = await getVaultStatus(server);
    } catch (err) {
      status = null;
      writer = null;
      statusError = reason(err);
      return;
    }
    writer = status.role === 'owner' ? server : null;
    if (status.role === 'remote') void resolveWriter(status.owner);
    if (status.state !== 'open') { entries = null; return; }
    try {
      entries = await listVaultEntries(server);
    } catch (err) {
      entries = null;
      entriesError = reason(err);
    }
  }
  $effect(() => { void load(); });

  // ── Onde fica o cofre ──────────────────────────────────────────────────────────────────────
  let configuring = $state(false);
  let configError = $state<string | null>(null);
  // Máquinas do `peers.json` desta: candidatas a dona e a guardar a cópia. Lidas só quando pedidas.
  let peers = $state<VaultPeer[] | null>(null);
  let peersFor = $state<'owner' | 'backup' | 'restore' | null>(null);
  let peersLoading = $state(false);
  let peersError = $state<string | null>(null);

  async function loadPeers(): Promise<boolean> {
    if (peers) return true;
    peersLoading = true;
    peersError = null;
    try {
      peers = await listVaultPeers(server);
      return true;
    } catch (err) {
      peersError = m.vault_web_peers_failed({ error: reason(err) });
      return false;
    } finally {
      peersLoading = false;
    }
  }

  function showPeers(purpose: 'owner' | 'backup' | 'restore') {
    peersFor = purpose;
    void loadPeers();
  }

  async function setConfig(change: { owner: string } | { backup_peer: string }) {
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
      status = await setVaultRecovery(server, typed);
      rDone = true;
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
  const restoreCodes: Record<string, () => string> = {
    erro_cofre_recuperacao_errada: m.vault_web_restore_wrong,
    erro_cofre_nao_vazio: m.vault_web_restore_not_empty,
    erro_cofre_backup_ausente: m.vault_web_restore_no_copy,
    erro_cofre_backup_ilegivel: m.vault_web_restore_unreadable,
    erro_cofre_backup_peer_fora: m.vault_web_restore_peer_down,
    erro_cofre_recuperacao_nao_gravada: m.vault_web_restore_recovery_not_saved,
  };

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
      const code = codeOf(err);
      restoreError = (code && restoreCodes[code]?.()) || reason(err);
      // O cofre voltou, só a senha de recuperação faltou: a tela mostra o cofre aberto.
      if (code === 'erro_cofre_recuperacao_nao_gravada') await load();
    } finally {
      restoring = false;
    }
  }

  const when = (at: number) => new Date(at * 1000).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
  const canRestore = $derived(!!status && status.role !== 'remote' &&
    (status.state === 'absent' || (status.state === 'open' && entries?.length === 0)));

  // ── Revelar e copiar ────────────────────────────────────────────────────────────────────────
  async function fetchPassword(id: string): Promise<string | null> {
    if (revealed[id] !== undefined) return revealed[id];
    const w = writer;
    if (!w) return null;
    busyId = id;
    try {
      const { password } = await revealVaultPassword(w, id);
      return password;
    } catch (err) {
      notice = m.vault_web_reveal_failed({ error: reason(err) });
      return null;
    } finally {
      busyId = null;
    }
  }

  function forget(id: string) {
    const next = { ...revealed };
    delete next[id];
    revealed = next;
  }

  async function toggleReveal(id: string) {
    if (revealed[id] !== undefined) { forget(id); return; }
    const pw = await fetchPassword(id);
    if (pw !== null) revealed = { ...revealed, [id]: pw };
  }

  async function copy(id: string) {
    notice = null;
    const had = revealed[id];
    const pw = had ?? await fetchPassword(id);
    if (pw === null) return;
    if (await copyText(pw)) { notice = m.vault_web_copied(); return; }
    // Depois de buscar a senha o navegador pode recusar a cópia fora do toque: o segundo toque copia direto.
    revealed = { ...revealed, [id]: pw };
    notice = m.vault_web_copy_again();
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

  function openForm(e: VaultEntry | 'new') {
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
  function closeForm() { editing = null; fPassword = ''; formError = null; }

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
    saving = true;
    formError = null;
    try {
      if (target === 'new') await addVaultEntry(w, { ...fields, password: typed });
      else {
        await updateVaultEntry(w, target.id, typed ? { ...fields, password: typed } : fields);
        forget(target.id);
      }
      closeForm();
      await load();
    } catch (err) {
      formError = codeOf(err) === 'erro_cofre_nome_repetido' ? m.vault_web_name_taken() : reason(err);
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
      {:else if status.state === 'offline'}
        <p class="pv-aviso" role="status">{m.vault_web_state_offline({ owner: status.owner ?? '', detail: status.detail ?? '' })}</p>
      {:else if status.state === 'absent' && status.role !== 'none'}
        <p class="pv-aviso" role="status">{status.detail ?? m.vault_web_state_absent()}</p>
      {/if}
      <div class="pv-acoes">
        {#if status.role !== 'owner'}
          <button type="button" class="pv-btn" disabled={configuring} onclick={() => setOwner('self')}>{m.vault_web_make_owner()}</button>
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
        {#if peersFor === 'backup'}{@render peerChoice((id) => setConfig({ backup_peer: id }), status.backup?.peer ?? null)}{/if}
        {#if configError && peersFor === 'backup'}<p class="pv-erro-txt" role="alert">{configError}</p>{/if}

        <form class="pv-form-inline" onsubmit={saveRecovery}>
          <p class="pv-campo-tit">{m.vault_web_recovery_title()}</p>
          <p class="pv-aviso">{m.vault_web_recovery_warning()}</p>
          <label class="pv-campo"><span>{m.vault_web_recovery_password()}</span>
            <span class="pv-pw">
              <input type={rVisible ? 'text' : 'password'} autocomplete="new-password" autocapitalize="off" spellcheck="false"
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
            <input type={rVisible ? 'text' : 'password'} autocomplete="new-password" autocapitalize="off" spellcheck="false"
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
          <input type="password" autocomplete="off" autocapitalize="off" spellcheck="false"
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
      <label class="pv-campo"><span>{m.vault_web_name()}</span>
        <input type="text" autocomplete="off" bind:value={fName} disabled={saving} required /></label>
      <label class="pv-campo"><span>{m.vault_web_username()}</span>
        <input type="text" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={fUser} disabled={saving} /></label>
      <label class="pv-campo"><span>{editing === 'new' ? m.vault_web_password() : m.vault_web_password_keep()}</span>
        <span class="pv-pw">
          <input type={fVisible ? 'text' : 'password'} autocomplete="new-password" autocapitalize="off" spellcheck="false"
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
        <input type="url" inputmode="url" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={fUrl} disabled={saving} /></label>
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
