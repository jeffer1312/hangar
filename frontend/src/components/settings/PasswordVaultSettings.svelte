<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import ConfirmSheet from '../ConfirmSheet.svelte';
  import * as m from '../../paraglide/messages';
  import { copyText } from '../../lib/clipboard';
  import { listOwnServers } from '../../lib/auth';
  import {
    addVaultEntry, deleteVaultEntry, getVaultStatus, listVaultEntries, putVaultConfig, revealVaultPassword, updateVaultEntry,
    type Server, type VaultEntry, type VaultPolicy, type VaultStatus,
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

  const reason = (err: unknown) => err instanceof Error ? err.message : String(err);
  const codeOf = (err: unknown) => (err as { code?: string })?.code;

  async function load() {
    statusError = null;
    entriesError = null;
    try {
      status = await getVaultStatus(server);
    } catch (err) {
      status = null;
      statusError = reason(err);
      return;
    }
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
  // Outras máquinas do dono que guardam um cofre, lidas só quando a pessoa pede para escolher.
  let candidates = $state<{ server: Server; id: string }[] | null>(null);
  let probing = $state(false);

  async function setOwner(owner: string) {
    configuring = true;
    configError = null;
    try {
      await putVaultConfig(server, { owner });
      candidates = null;
      await load();
    } catch (err) {
      configError = reason(err);
    } finally {
      configuring = false;
    }
  }

  async function findOwners() {
    probing = true;
    configError = null;
    const others = listOwnServers().filter((s) => s.id !== server.id);
    const found = await Promise.all(others.map(async (s) => {
      try {
        const st = await getVaultStatus(s);
        return st.role === 'owner' && st.server_id ? { server: s, id: st.server_id } : null;
      } catch { return null; }
    }));
    candidates = found.filter((c): c is { server: Server; id: string } => !!c);
    probing = false;
  }

  // ── Revelar e copiar ────────────────────────────────────────────────────────────────────────
  async function fetchPassword(id: string): Promise<string | null> {
    if (revealed[id] !== undefined) return revealed[id];
    busyId = id;
    try {
      const { password } = await revealVaultPassword(server, id);
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
    if (!editing || saving || !canSave) return;
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
      if (target === 'new') await addVaultEntry(server, { ...fields, password: typed });
      else {
        await updateVaultEntry(server, target.id, typed ? { ...fields, password: typed } : fields);
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
    if (!e) return;
    notice = null;
    try {
      await deleteVaultEntry(server, e.id);
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
          <button type="button" class="pv-btn" disabled={configuring || probing} onclick={findOwners}>
            {probing ? m.vault_web_finding_owners() : m.vault_web_choose_owner()}
          </button>
        {/if}
        {#if status.state !== 'open'}
          <button type="button" class="pv-btn" onclick={load}>{m.vault_web_retry()}</button>
        {/if}
      </div>
      {#if candidates}
        {#if candidates.length === 0}
          <p class="pv-muted">{m.vault_web_no_owners()}</p>
        {:else}
          <div class="pv-acoes">
            {#each candidates as c (c.server.id)}
              <button type="button" class="pv-btn" disabled={configuring} onclick={() => setOwner(c.id)}>
                {m.vault_web_use_owner({ label: c.server.label })}
              </button>
            {/each}
          </div>
        {/if}
      {/if}
      {#if configError}<p class="pv-erro-txt" role="alert">{configError}</p>{/if}
    </div>

    {#if status.state === 'open'}
      <div class="pv-topo">
        <p class="pv-secao">{m.vault_web_entries_title()}</p>
        <button type="button" class="pv-btn pv-primario" onclick={() => openForm('new')}>{m.vault_web_add()}</button>
      </div>
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
              <div class="pv-item-acoes">
                <button type="button" class="pv-btn" disabled={busyId === e.id} onclick={() => toggleReveal(e.id)}>
                  {revealed[e.id] !== undefined ? m.vault_web_hide() : m.vault_web_reveal()}
                </button>
                <button type="button" class="pv-btn" disabled={busyId === e.id} onclick={() => copy(e.id)}>{m.vault_web_copy()}</button>
                <button type="button" class="pv-btn" onclick={() => openForm(e)}>{m.vault_web_edit()}</button>
                <button type="button" class="pv-btn pv-perigo" onclick={() => (deleting = e)}>{m.vault_web_delete()}</button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  {/if}
</div>

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
