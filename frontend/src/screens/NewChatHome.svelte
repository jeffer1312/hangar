<script lang="ts">
  import { untrack } from 'svelte';
  import { basename, createSession, defaultBase, pickFolderRoot, providerName, type Provider, type SessionOpeningExtras, type WorktreeChoice } from '@hangar/core';
  import HomeUsage from '../components/HomeUsage.svelte';
  import FolderGitPill from '../components/FolderGitPill.svelte';
  import BottomSheet from '../components/BottomSheet.svelte';
  import FolderScanner from '../components/FolderScanner.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import IconFolder from '../components/icons/IconFolder.svelte';
  import IconMonitor from '../components/icons/IconMonitor.svelte';
  import IconWorktree from '../components/icons/IconWorktree.svelte';
  import QuietPill from '../components/newchat/QuietPill.svelte';
  import AccountPill from '../components/newchat/AccountPill.svelte';
  import ModelPill from '../components/newchat/ModelPill.svelte';
  import NewChatComposer from '../components/newchat/NewChatComposer.svelte';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { faixaDeCota } from '../lib/cota';
  import { createNewChatDraft } from '../lib/newChatDraft.svelte';
  import { selectServer, getActiveId, listOwnServers } from '../lib/auth';
  import { abrirConfig } from '../lib/configNav';
  import { keyboardInset } from '../lib/keyboardInset';
  import * as m from '../paraglide/messages';

  // Tela inicial do celular: nova conversa no desenho do nativo (`render_new_chat`). Máquina e pasta
  // acima do compositor, modelo dentro, conta e branch abaixo, e o aviso embaixo de tudo.
  let { onOpenList }: { onOpenList: () => void } = $props();

  const draft = createNewChatDraft();
  draft.init();

  // A pílula de conta mantém o feed vivo; aqui só reage a cada leitura nova.
  $effect(() => {
    const linha = faixaDeCota(quotaFeed.contas);
    untrack(() => draft.switchFromExhausted(linha));
  });

  type Menu = 'machine' | 'folder' | 'branch';
  let menu = $state<Menu | null>(null);
  let moreOpen = $state(false);
  let text = $state('');

  const folderLabel = $derived(draft.cwd ? basename(draft.cwd) : m.native_new_chat_folder());
  const machineLabel = $derived(draft.serverObj?.label ?? m.native_new_chat_machine());
  const branchLabel = $derived.by(() => {
    if (draft.branchesLoading) return m.native_create_checkout_loading();
    if (draft.newBranch) return `${draft.branchName || m.worktree_nome_branch()} · ${m.native_create_checkout_worktree()}`;
    if (draft.branch) return `${draft.branch} · ${m.native_create_checkout_worktree()}`;
    return draft.branches?.current ?? m.native_create_checkout_current();
  });
  const otherBranches = $derived(draft.branches
    ? [...draft.branches.branches, ...draft.branches.remotes].filter((b) => b !== draft.branches?.current)
    : []);

  async function send() {
    try {
      const r = await draft.send(text);
      text = '';
      selectServer(r.serverId);
      window.location.hash = `#/chat/${encodeURIComponent(r.serverId)}/${encodeURIComponent(r.name)}`;
    } catch {
      // O texto fica no campo; o erro já está em `draft.note`.
    }
  }

  function pickFolder(path: string) {
    menu = null;
    draft.setCwd(path);
  }

  // "Mais opções": a folha completa, que já faz `selectServer` no alvo antes de criar.
  async function createFromSheet(name: string, cwd?: string, configDir?: string | null, provider?: Provider,
                                 engine?: string | null, model?: string | null, effort?: string | null,
                                 permissionMode?: string | null, ompProfile?: string | null,
                                 headless?: boolean, subagentModel?: string | null, jev?: boolean,
                                 worktree?: WorktreeChoice | null, opening?: SessionOpeningExtras) {
    const info = await createSession(name, cwd, configDir, provider, engine, model, effort, permissionMode, ompProfile,
                                     null, headless, subagentModel, jev, worktree ?? undefined, opening);
    openChat(info.name);
  }
  function openChat(name: string) {
    const sid = getActiveId();
    window.location.hash = `#/chat/${sid ? encodeURIComponent(sid) + '/' : ''}${encodeURIComponent(name)}`;
  }
</script>

<div class="home" use:keyboardInset>
  <header class="top">
    <button type="button" class="icon-btn" aria-label={m.newchat_abrir_lista()} onclick={onOpenList}>
      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
        stroke-linecap="round" aria-hidden="true"><line x1="4" y1="7" x2="20" y2="7" /><line x1="4" y1="12" x2="20" y2="12" /><line x1="4" y1="17" x2="14" y2="17" /></svg>
    </button>
    <button type="button" class="icon-btn" aria-label={m.native_settings()} onclick={() => abrirConfig('root', null)}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
      </svg>
    </button>
  </header>

  {#if draft.servers.length === 0}
    <div class="empty">
      <p>{m.newchat_sem_servidor()}</p>
      <button type="button" class="link" onclick={() => abrirConfig('root', null)}>{m.native_settings()}</button>
    </div>
  {:else}
    <div class="greeting">
      {#if draft.serverObj}<HomeUsage server={draft.serverObj} />{/if}
      <h1>{m.native_new_chat_title()}</h1>
    </div>

    <div class="dock">
      <div class="top-pills">
        {#if draft.servers.length > 1}
          <QuietPill label={machineLabel} ariaLabel={m.native_new_chat_machine()} disabled={draft.sending}
            onclick={() => (menu = 'machine')}>
            {#snippet icon()}<IconMonitor size={14} />{/snippet}
          </QuietPill>
        {/if}
        <QuietPill label={folderLabel} ariaLabel={m.native_new_chat_folder()} loading={draft.roots === null && !draft.rootsError}
          disabled={draft.sending || draft.roots?.length === 0} onclick={() => (menu = 'folder')}>
          {#snippet icon()}<IconFolder size={14} />{/snippet}
        </QuietPill>
      </div>

      <NewChatComposer bind:value={text} placeholder={m.native_composer({ agent: providerName(draft.provider) })}
        busy={draft.sending} blocked={draft.loading || !!draft.proxyBlocked} note={draft.note} onsend={send}>
        {#snippet pills()}<ModelPill {draft} disabled={draft.sending} />{/snippet}
        {#snippet below()}
          <div class="bottom-pills">
            <AccountPill server={draft.server} provider={draft.provider} configs={draft.accountChoices}
              codexAccounts={draft.codexAccounts}
              selected={draft.provider === 'codex' ? draft.codexAccount : draft.configDir}
              loading={draft.configsLoading || draft.codexLoading} disabled={draft.sending}
              onchange={(id) => (draft.provider === 'codex' ? draft.setCodexAccount(id) : draft.setConfig(id))} />
            {#if draft.branchesLoading || draft.branches}
              <QuietPill label={branchLabel} ariaLabel={m.native_create_checkout_branch()} loading={draft.branchesLoading}
                disabled={draft.sending} onclick={() => (menu = 'branch')}>
                {#snippet icon()}<IconWorktree size={14} />{/snippet}
              </QuietPill>
            {/if}
            {#if draft.serverObj && draft.cwd}
              <FolderGitPill server={draft.serverObj} cwd={draft.cwd} disabled={draft.sending}
                root={draft.roots ? (pickFolderRoot(draft.roots, draft.cwd) ?? undefined) : undefined} />
            {/if}
            <button type="button" class="more" disabled={draft.sending} onclick={() => (moreOpen = true)}>{m.native_create_more()}</button>
          </div>
        {/snippet}
      </NewChatComposer>
    </div>
  {/if}
</div>

<BottomSheet open={menu === 'machine'} onClose={() => (menu = null)} ariaLabel={m.native_new_chat_machine()}>
  <div class="sheet">
    <h2 class="title">{m.native_new_chat_machine()}</h2>
    <ul class="list">
      {#each draft.servers as s (s.id)}
        <li>
          <button type="button" class="row" class:on={s.id === draft.server} aria-pressed={s.id === draft.server}
            onclick={() => { menu = null; if (s.id !== draft.server) draft.pickServer(s.id); }}>
            <span>{s.label}</span>
            {#if s.id === draft.server}<span class="muted">{m.native_create_current()}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  </div>
</BottomSheet>

<BottomSheet open={menu === 'folder'} onClose={() => (menu = null)} ariaLabel={m.native_new_chat_folder()}>
  <div class="sheet">
    <!-- O scanner lê o servidor ativo: remonta quando a máquina muda. -->
    {#key draft.server}
      {#if menu === 'folder'}<FolderScanner onPick={pickFolder} selected={draft.cwd} />{/if}
    {/key}
  </div>
</BottomSheet>

<BottomSheet open={menu === 'branch'} onClose={() => (menu = null)} ariaLabel={m.native_create_checkout_branch()}>
  <div class="sheet">
    <h2 class="title">{m.native_create_checkout_branch()}</h2>
    <ul class="list">
      <li>
        <button type="button" class="row" class:on={draft.branch === '' && !draft.newBranch}
          aria-pressed={draft.branch === '' && !draft.newBranch}
          onclick={() => { menu = null; draft.branch = ''; draft.newBranch = false; }}>
          <span>{m.native_create_checkout_current()}{draft.branches?.current ? ` · ${draft.branches.current}` : ''}</span>
        </button>
      </li>
      <li>
        <button type="button" class="row" class:on={draft.newBranch} aria-pressed={draft.newBranch}
          onclick={() => { draft.newBranch = true; draft.branch = ''; draft.base = draft.base || (draft.branches ? defaultBase(draft.branches) : ''); }}>
          <span>{m.worktree_nova_branch({ base: draft.base || draft.branches?.current || '' })}</span>
        </button>
      </li>
      {#if draft.newBranch}
        <li class="campos">
          <label class="campo">{m.worktree_nome_branch()}
            <input class="field-input" type="text" bind:value={draft.branchName} autocapitalize="off" spellcheck="false" />
          </label>
          <label class="campo">{m.worktree_base()}
            <select class="field-input" bind:value={draft.base}>
              {#each [draft.branches?.current, ...otherBranches].filter(Boolean) as b (b)}
                <option value={b}>{b}</option>
              {/each}
            </select>
          </label>
        </li>
      {/if}
      {#each otherBranches as b (b)}
        <li>
          <button type="button" class="row" class:on={draft.branch === b} aria-pressed={draft.branch === b}
            onclick={() => { menu = null; draft.branch = b; draft.newBranch = false; }}>
            <span>{b}</span><span class="muted">{m.native_create_checkout_worktree()}</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if draft.branch || draft.newBranch}
      <p class="help"><strong>{m.worktree_modo()}</strong> — {m.worktree_modo_ajuda()}</p>
    {:else}
      <p class="help">{m.native_create_checkout_current_help()}</p>
    {/if}
    {#if draft.branches?.dirty}<p class="help">{m.native_create_checkout_dirty()}</p>{/if}
  </div>
</BottomSheet>

<CreateSessionSheet open={moreOpen} servers={listOwnServers()} onClose={() => (moreOpen = false)}
  onCreate={createFromSheet} onOpenSession={openChat} />

<style>
  .home {
    flex: 0 1 auto; height: 100%; min-height: 0; display: flex; flex-direction: column; background: transparent; position: relative;
    padding: env(safe-area-inset-top) var(--space-3) calc(env(safe-area-inset-bottom) + var(--space-3));
  }
  .top { display: flex; justify-content: space-between; align-items: center; min-height: 52px; }
  .icon-btn {
    width: 44px; height: 44px; display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-full); background: transparent; color: var(--text-secondary); cursor: pointer;
  }
  .icon-btn:active { background: var(--fill-subtle); }
  .greeting { flex: 1; min-height: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; }
  .greeting h1 { margin: 0; font-size: var(--text-xl); font-weight: var(--fw-semibold); color: var(--text-primary); }
  .dock { display: flex; flex-direction: column; gap: 2px; }
  .top-pills { display: flex; justify-content: flex-end; align-items: center; gap: 2px; padding-bottom: 4px; }
  .bottom-pills { display: flex; align-items: center; flex-wrap: wrap; gap: 2px; padding: 2px 0 0 4px; }
  .more {
    margin-left: auto; min-height: 32px; padding: 4px 10px; border: 0; background: transparent;
    color: var(--text-muted); font-size: var(--text-sm); cursor: pointer;
  }
  .more:disabled { opacity: 0.6; cursor: default; }
  .empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: var(--space-3); color: var(--text-secondary); text-align: center; }
  .link { border: 0; background: transparent; color: var(--accent); font-size: var(--text-base); cursor: pointer; }
  .sheet { padding: var(--space-2) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-4)); }
  .title { margin: 0 0 var(--space-2); font-size: var(--text-base); font-weight: var(--fw-semibold); color: var(--text-primary); }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; max-height: 50vh; overflow-y: auto; }
  .row {
    width: 100%; min-height: 44px; display: flex; justify-content: space-between; align-items: center; gap: 8px;
    padding: 8px 12px; border: 0; border-radius: var(--radius-md); background: transparent;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .row.on { background: var(--accent-dim); }
  .muted { font-size: var(--text-xs); color: var(--text-muted); }
  .help { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  /* .field-input é global (app.css), o mesmo campo da folha completa. */
  .campos { display: flex; flex-direction: column; gap: 6px; padding: 4px 12px 8px; }
  .campo { display: flex; flex-direction: column; gap: 6px; font-size: var(--text-xs); color: var(--text-muted); }
</style>
