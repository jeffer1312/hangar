<script lang="ts">
  import { untrack } from 'svelte';
  import {
    folderGitActionForServer, folderGitBranchForServer, folderGitSwitchForServer, getFolderBranchesForServer,
    getFolderGitForServer, type FolderBranches, type FolderGit, type Server,
  } from '@hangar/core';
  import BottomSheet from './BottomSheet.svelte';
  import ConfirmSheet from './ConfirmSheet.svelte';
  import QuietPill from './newchat/QuietPill.svelte';
  import * as m from '../paraglide/messages';

  // Pílula de git da pasta (porte de `create/folder_git.rs`): some fora de repositório; o toque abre
  // a folha com o estado, Fetch e Pull, e as abas de trocar e criar branch. A primeira abertura de
  // cada pasta faz um fetch, como o `git_opened` do nativo.
  interface Props { server: Server; cwd: string; root?: string; disabled?: boolean; onChanged?: () => void }
  let { server, cwd, root, disabled = false, onChanged }: Props = $props();
  let tab = $state<'state' | 'switch' | 'create'>('state');
  let branches = $state<FolderBranches | null>(null);
  let newName = $state('');
  let newBase = $state('');
  let fetchedFor = '';
  let confirmSwitch = $state<{ text: string; run: () => void } | null>(null);

  let git = $state<FolderGit | null>(null);
  let loading = $state(false);
  let failed = $state(false);
  let open = $state(false);
  let busy = $state<'fetch' | 'pull' | null>(null);
  let note = $state<{ text: string; kind: 'ok' | 'err' } | null>(null);
  let seq = 0;

  const errText = (e: unknown) => (e instanceof Error && e.message ? e.message : m.home_usage_load_failed());

  async function load() {
    const mine = ++seq;
    loading = true;
    busy = null;
    failed = false;
    git = null;
    note = null;
    try {
      const g = await getFolderGitForServer(server, cwd, undefined, root);
      if (mine === seq) git = g;
    } catch {
      if (mine === seq) failed = true;
    } finally {
      if (mine === seq) loading = false;
    }
  }

  // Pasta ou máquina nova: o estado anterior não vale.
  $effect(() => {
    void [server.id, cwd, root];
    untrack(() => { tab = 'state'; branches = null; void load(); });
  });

  $effect(() => {
    if (!open) return;
    untrack(() => {
      const key = `${server.id}:${cwd}`;
      if (fetchedFor !== key && git?.repo) { fetchedFor = key; void act('fetch', true); }
      if (!branches) {
        getFolderBranchesForServer(server, cwd, undefined, root).then((b) => { branches = b; }).catch(() => {});
      }
    });
  });

  // O que a troca e a criação devolvem é o estado já relido; quem lista branches da pasta relê.
  function changed(g: FolderGit, text: string) {
    git = g;
    note = { kind: 'ok', text };
    branches = null;
    getFolderBranchesForServer(server, cwd, undefined, root).then((b) => { branches = b; }).catch(() => {});
    onChanged?.();
  }

  async function runGit(kind: 'switch' | 'create', target: string, confirmSessions = false) {
    if (busy) return;
    busy = 'pull';
    note = null;
    try {
      const g = kind === 'switch'
        ? await folderGitSwitchForServer(server, cwd, target, confirmSessions, root)
        : await folderGitBranchForServer(server, cwd, target, { checkout: true, base: newBase || undefined, confirmSessions }, root);
      changed(g, kind === 'switch' ? m.native_folder_git_switched({ branch: target }) : m.native_folder_git_created({ branch: target }));
      if (kind === 'create') newName = '';
      tab = 'state';
    } catch (e) {
      const err = e as { code?: string; envelope?: { params?: { sessoes?: string } } };
      if (err.code === 'erro_git_folder_sessions') {
        confirmSwitch = {
          text: m.native_folder_git_sessions({ sessoes: err.envelope?.params?.sessoes ?? '', branch: target }),
          run: () => void runGit(kind, target, true),
        };
      } else {
        note = { kind: 'err', text: errText(e) };
      }
    } finally {
      busy = null;
    }
  }

  const diverged = $derived(!!git && (git.ahead ?? 0) > 0 && (git.behind ?? 0) > 0);
  const pullBlock = $derived.by(() => {
    if (!git) return '';
    if ((git.dirty ?? 0) > 0) return m.native_folder_git_dirty({ n: String(git.dirty) });
    if (!git.upstream) return m.native_folder_git_no_upstream_pull();
    if (diverged) {
      return m.native_folder_git_diverged({
        upstream: git.upstream, ahead: String(git.ahead ?? 0), behind: String(git.behind ?? 0),
      });
    }
    return '';
  });

  const label = $derived.by(() => {
    if (!git) return m.native_folder_git_label();
    const parts = [
      (git.behind ?? 0) > 0 ? `↓${git.behind}` : '',
      (git.ahead ?? 0) > 0 ? `↑${git.ahead}` : '',
      (git.dirty ?? 0) > 0 ? m.native_folder_git_changes({ n: String(git.dirty) }) : '',
    ].filter(Boolean);
    if (parts.length) return parts.join(' · ');
    return git.upstream ? m.native_folder_git_up_to_date() : m.native_folder_git_label();
  });

  function ago(epoch: number): string {
    const s = Date.now() / 1000 - epoch;
    if (s < 60) return m.native_ago_now();
    if (s < 3600) return m.native_ago_min({ n: String(Math.floor(s / 60)) });
    if (s < 86400) return m.native_ago_h({ n: String(Math.floor(s / 3600)) });
    return m.native_ago_d({ n: String(Math.floor(s / 86400)) });
  }

  const statusLine = $derived.by(() => {
    if (!git) return '';
    const parts = [git.upstream ?? m.native_folder_git_no_upstream()];
    if (git.upstream) {
      const behind = git.behind ?? 0;
      const ahead = git.ahead ?? 0;
      if (!behind && !ahead) parts.push(m.native_folder_git_up_to_date());
      if (behind) parts.push(m.native_folder_git_behind({ n: String(behind) }));
      if (ahead) parts.push(m.native_folder_git_ahead({ n: String(ahead) }));
    }
    if ((git.dirty ?? 0) > 0) parts.push(m.native_folder_git_changes({ n: String(git.dirty) }));
    parts.push(git.last_fetch ? m.native_folder_git_fetched({ quando: ago(git.last_fetch) }) : m.native_folder_git_never_fetched());
    return parts.join(' · ');
  });

  async function act(action: 'fetch' | 'pull', quiet = false) {
    if (busy) return;
    busy = action;
    note = null;
    const mine = ++seq;
    try {
      const g = await folderGitActionForServer(server, cwd, action, root);
      if (mine !== seq) return;
      git = g;
      if (quiet) return;
      note = {
        kind: 'ok',
        text: action === 'fetch'
          ? m.native_folder_git_fetch_done()
          : m.native_folder_git_pull_done({ upstream: g.upstream ?? '' }),
      };
    } catch (e) {
      if (mine === seq) note = { kind: 'err', text: errText(e) };
    } finally {
      if (mine === seq) busy = null;
    }
  }
</script>

{#if git?.repo || (failed && !git) || (loading && !git)}
  <QuietPill label={label} ariaLabel={m.native_folder_git_title()} loading={loading && !git}
    disabled={disabled} onclick={() => (open = true)}>
    {#snippet icon()}
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M21 12a9 9 0 1 1-3-6.7" /><polyline points="21 3 21 9 15 9" />
      </svg>
    {/snippet}
  </QuietPill>
{/if}

<ConfirmSheet open={!!confirmSwitch} title={m.native_folder_git_title()} message={confirmSwitch?.text ?? ''}
  confirmLabel={m.native_folder_git_switch_anyway()}
  onConfirm={() => { const run = confirmSwitch?.run; confirmSwitch = null; run?.(); }} onClose={() => (confirmSwitch = null)} />

<BottomSheet {open} onClose={() => (open = false)} ariaLabel={m.native_folder_git_title()}>
  <div class="sheet">
    <h2 class="title">{m.native_folder_git_title()}</h2>
    {#if failed && !git}
      <p class="help err" role="alert">{m.home_usage_load_failed()}</p>
      <button type="button" class="btn" onclick={() => load()}>{m.sync_retry()}</button>
    {:else if git?.repo}
      <div class="tabs" role="tablist">
        <button type="button" role="tab" class="tab" class:on={tab === 'state'} aria-selected={tab === 'state'}
          onclick={() => (tab = 'state')}>{m.native_folder_git_label()}</button>
        <button type="button" role="tab" class="tab" class:on={tab === 'switch'} aria-selected={tab === 'switch'}
          onclick={() => (tab = 'switch')}>{m.native_folder_git_switch_tab()}</button>
        <button type="button" role="tab" class="tab" class:on={tab === 'create'} aria-selected={tab === 'create'}
          onclick={() => (tab = 'create')}>{m.native_folder_git_create_tab()}</button>
      </div>
      <p class="branch">{git.current ?? 'HEAD'}</p>
      <p class="help" role="status">{statusLine}</p>
      {#if tab === 'switch'}
        {#if !branches}
          <p class="help" role="status">{m.comum_carregando()}</p>
        {:else}
          <ul class="list">
            {#each [...branches.branches.map((b) => ({ name: b, remote: false })), ...branches.remotes.map((b) => ({ name: b, remote: true }))] as b (b.name + b.remote)}
              <li>
                <button type="button" class="row" disabled={!!busy || b.name === git.current} onclick={() => runGit('switch', b.name)}>
                  <span>{b.name}</span>
                  <span class="muted">{b.name === git.current ? m.native_folder_git_current() : b.remote ? m.native_folder_git_remote() : ''}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {:else if tab === 'create'}
        <label class="field"><span class="muted">{m.native_folder_git_name()}</span>
          <input type="text" bind:value={newName} autocapitalize="off" autocomplete="off" spellcheck="false" /></label>
        <label class="field"><span class="muted">{m.native_folder_git_base()}</span>
          <select bind:value={newBase}>
            <option value="">{git.current ?? 'HEAD'}</option>
            {#each branches?.branches ?? [] as b (b)}{#if b !== git.current}<option value={b}>{b}</option>{/if}{/each}
          </select></label>
        <button type="button" class="btn primary full" disabled={!!busy || !newName.trim()} onclick={() => runGit('create', newName.trim())}>
          {m.native_folder_git_create()}
        </button>
      {:else}
      <div class="actions">
        <button type="button" class="btn" disabled={!!busy} onclick={() => act('fetch')}>
          {m.native_folder_git_fetch()}
        </button>
        <button type="button" class="btn primary" disabled={!!busy || !!pullBlock} onclick={() => act('pull')}>
          {m.native_folder_git_pull()}
        </button>
      </div>
      {#if pullBlock}
        <p class="help warn" role="alert">{pullBlock}</p>
      {/if}
      {/if}
      {#if note}
        <p class="help" class:err={note.kind === 'err'} role="alert">{note.text}</p>
      {/if}
    {:else if git}
      <p class="help">{m.native_folder_git_not_repo()}</p>
    {/if}
  </div>
</BottomSheet>

<style>
  .sheet { padding: var(--space-2) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-4)); }
  .title { margin: 0 0 var(--space-2); font-size: var(--text-base); font-weight: var(--fw-semibold); color: var(--text-primary); }
  .branch { margin: 0; font-size: var(--text-sm); font-weight: var(--fw-semibold); color: var(--text-primary); }
  .help { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  .help.warn { color: var(--warning, var(--text-secondary)); }
  .help.err { color: var(--error); }
  .actions { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
  .btn {
    flex: 1; min-height: 44px; border: 1px solid var(--fill-subtle); border-radius: var(--radius-md);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm); cursor: pointer;
  }
  .btn.primary { background: var(--accent-dim); }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .btn.full { width: 100%; margin-top: var(--space-3); }
  .tabs { display: flex; gap: 4px; margin-bottom: var(--space-3); }
  .tab {
    flex: 1; min-height: 44px; border: 0; border-radius: var(--radius-md); background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer;
  }
  .tab.on { background: var(--accent-dim); color: var(--text-primary); }
  .list { list-style: none; margin: var(--space-2) 0 0; padding: 0; max-height: 45vh; overflow-y: auto; }
  .row {
    width: 100%; min-height: 44px; display: flex; justify-content: space-between; align-items: center; gap: 8px;
    padding: 8px 12px; border: 0; border-radius: var(--radius-md); background: transparent;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .row:disabled { opacity: 0.6; cursor: default; }
  .muted { font-size: var(--text-xs); color: var(--text-muted); }
  .field { display: flex; flex-direction: column; gap: 4px; margin-top: var(--space-3); }
  .field input, .field select {
    min-height: 44px; padding: 0 12px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); color: var(--text-primary); font-size: var(--text-sm);
  }
</style>
