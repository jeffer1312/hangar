<script lang="ts">
  import { flushSync, onMount, untrack } from 'svelte';
  import { basename, createSession, defaultBase, pickFolderRoot, providerName, type AggSession, type Provider, type SessionOpeningExtras, type WorktreeChoice } from '@hangar/core';
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
  import NewChatAttach from '../components/newchat/NewChatAttach.svelte';
  import NewChatMic from '../components/newchat/NewChatMic.svelte';
  import HomeSessions from '../components/HomeSessions.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { homeSessionsView } from '../lib/homeSessions';
  import { homeScreen, type HomeScreen } from '../lib/homeScreen.svelte';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { faixaDeCota } from '../lib/cota';
  import { createNewChatDraft } from '../lib/newChatDraft.svelte';
  import { selectServer, getActiveId, listOwnServers } from '../lib/auth';
  import { abrirConfig } from '../lib/configNav';
  import { keyboardInset } from '../lib/keyboardInset';
  import * as m from '../paraglide/messages';

  // Tela inicial do celular: lista das sessões abertas com a faixa "Nova conversa…" embaixo; a faixa
  // cresce até a nova conversa no desenho do nativo (`render_new_chat`), na mesma tela.
  let { onOpenList, onOpenChat }: { onOpenList: () => void; onOpenChat: (name: string) => void } = $props();

  let freezeList = () => {};
  function openHomeChat(name: string) {
    freezeList();
    onOpenChat(name);
  }
  function rememberList(el: HTMLDivElement, _rows: AggSession[]) {
    let restoreTarget = homeSessionsView.scrollTop;
    let leaving = false;
    let frame = 0;
    let thaw: ReturnType<typeof setTimeout> | undefined;
    const restore = () => {
      cancelAnimationFrame(frame);
      if (restoreTarget <= 0 || leaving) return;
      el.style.overflowAnchor = 'none';
      // Cada onda tenta de novo depois do layout, sem perder o alvo quando ainda não cabe.
      frame = requestAnimationFrame(() => {
        if (restoreTarget <= 0 || leaving || !el.isConnected) return;
        el.scrollTop = restoreTarget;
        if (restoreTarget - el.scrollTop <= 1) { restoreTarget = 0; el.style.overflowAnchor = ''; }
      });
    };
    const save = () => {
      if (leaving || restoreTarget > 0) return;
      homeSessionsView.scrollTop = el.scrollTop;
    };
    const stopRestoring = (event: Event) => {
      if (event instanceof KeyboardEvent && !['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key)) return;
      restoreTarget = 0;
      el.style.overflowAnchor = '';
      save();
    };
    freezeList = () => {
      save();
      // O teardown pode emitir scroll(0) antes de desmontar a tela.
      leaving = true;
      clearTimeout(thaw);
      thaw = setTimeout(() => { leaving = false; restore(); }, 1000);
    };
    const gestures = ['wheel', 'touchmove', 'keydown'];
    el.addEventListener('scroll', save);
    for (const event of gestures) el.addEventListener(event, stopRestoring, { passive: true });
    restore();
    return {
      update: restore,
      destroy() {
        cancelAnimationFrame(frame); clearTimeout(thaw);
        el.removeEventListener('scroll', save);
        for (const event of gestures) el.removeEventListener(event, stopRestoring);
      },
    };
  }

  // A entrada do histórico marca a nova conversa aberta: Voltar do navegador fecha antes de sair.
  const COMPOSE = 'cpHomeCompose';
  let mode = $state<HomeScreen>(history.state?.[COMPOSE] ? 'newConversation' : homeScreen.pref);
  const collapsed = $derived(mode === 'sessions');
  let dockEl = $state<HTMLElement>();
  let dockHeight = $state(0);
  let launcherHeight = $state(0);
  // Altura só anima na troca de modo; crescer o campo ao digitar acompanha na hora.
  let animating = $state(false);
  let lastMode = untrack(() => mode);
  // `pre`: antes de o dock ficar `inert`, quando o clique no microfone ainda vale.
  $effect.pre(() => {
    if (mode === lastMode) return;
    lastMode = mode;
    if (mode === 'sessions') {
      // Gravando, a lista não esconde o microfone ligado: parar transcreve para o campo.
      untrack(() => dockEl)?.querySelector<HTMLButtonElement>('.mic[aria-pressed="true"]')?.click();
      if (untrack(() => dockEl)?.contains(document.activeElement)) (document.activeElement as HTMLElement).blur();
    }
    animating = true;
    const t = setTimeout(() => (animating = false), 340);
    return () => clearTimeout(t);
  });
  const openCount = $derived(sessionsStore.rows.filter((r) => r.state !== 'dead' && !r.outside).length);

  function markCompose() {
    if (!history.state?.[COMPOSE]) history.pushState({ ...history.state, [COMPOSE]: 1 }, '', location.href);
  }
  // Página escondida solta a reserva pré-aquecida, como o app em segundo plano.
  let visible = $state(document.visibilityState !== 'hidden');
  onMount(() => {
    if (mode === 'newConversation') markCompose();
    const onPop = () => { mode = history.state?.[COMPOSE] ? 'newConversation' : 'sessions'; };
    const onVisibility = () => { visible = document.visibilityState !== 'hidden'; };
    window.addEventListener('popstate', onPop);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('popstate', onPop);
      document.removeEventListener('visibilitychange', onVisibility);
      draft.releaseReserve();
    };
  });

  // Síncrono dentro do toque: o iOS só abre o teclado (e o microfone) no mesmo gesto.
  function openCompose(target: 'field' | 'mic') {
    flushSync(() => { mode = 'newConversation'; });
    markCompose();
    if (target === 'field') dockEl?.querySelector('textarea')?.focus();
    else dockEl?.querySelector<HTMLButtonElement>('.mic:not([aria-pressed="true"])')?.click();
  }
  function showSessions() {
    if (history.state?.[COMPOSE]) history.back();
    else mode = 'sessions';
  }

  const draft = createNewChatDraft();
  draft.init();

  // Reserva só enquanto compõe; sair do modo, esconder a página ou desmontar a solta.
  $effect(() => { draft.syncReserve(mode === 'newConversation' && visible); });

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
    if (draft.existing) return `${basename(draft.existing)} · ${m.newchat_worktree_existing()}`;
    if (draft.newBranch) return `${draft.branchName || m.worktree_nome_branch()} · ${m.native_create_checkout_worktree()}`;
    if (draft.branch) return `${draft.branch} · ${m.native_create_checkout_worktree()}`;
    return draft.branches?.current ?? m.native_create_checkout_current();
  });
  const otherBranches = $derived(draft.branches
    ? [...draft.branches.branches, ...draft.branches.remotes].filter((b) => b !== draft.branches?.current)
    : []);

  async function send(useSuggested = false) {
    try {
      const r = await draft.send(text, useSuggested);
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

<div class="home" class:collapsed use:keyboardInset>
  <header class="top">
    <button type="button" class="sessions-btn" aria-label={collapsed ? m.newchat_abrir_lista() : m.native_sessions()}
      onclick={collapsed ? onOpenList : showSessions}>
      {#if collapsed}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" aria-hidden="true"><line x1="4" y1="7" x2="20" y2="7" /><line x1="4" y1="12" x2="20" y2="12" /><line x1="4" y1="17" x2="14" y2="17" /></svg>
      {:else}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 18l-6-6 6-6" /></svg>
      {/if}
      <span class="sessions-label">{m.native_sessions()}</span>
      <span class="badge">{openCount}</span>
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
    <div class="middle">
      <div class="layer list-layer" inert={!collapsed} use:rememberList={sessionsStore.rows}>
        <HomeSessions onOpenChat={openHomeChat} />
      </div>
      <div class="layer greeting" inert={collapsed}>
        {#if draft.serverObj}<HomeUsage server={draft.serverObj} />{/if}
        <h1>{m.native_new_chat_title()}</h1>
      </div>
    </div>

    <div class="bottom" class:animating style:height={dockHeight ? `${collapsed ? launcherHeight : dockHeight}px` : undefined}>
    <div class="launcher" inert={!collapsed} bind:clientHeight={launcherHeight}>
      <button type="button" class="launcher-text" onclick={() => openCompose('field')}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"
          stroke-linejoin="round" aria-hidden="true"><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" /></svg>
        <span>{m.home_new_conversation()}</span>
      </button>
      <button type="button" class="launcher-mic" aria-label={m.newchat_mic()} disabled={!draft.serverObj || draft.sending}
        onclick={() => openCompose('mic')}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="9" y="2" width="6" height="12" rx="3" /><path d="M5 10v1a7 7 0 0 0 14 0v-1" /><line x1="12" y1="18" x2="12" y2="22" />
        </svg>
      </button>
    </div>

    <div class="dock" bind:this={dockEl} bind:clientHeight={dockHeight} inert={collapsed}>
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
        busy={draft.sending} blocked={draft.loading || !!draft.proxyBlocked} note={draft.note} onsend={() => send()}
        noteAction={draft.quota ? { label: m.open_quota_use({ conta: draft.quota.sugerida }), onclick: () => send(true) } : null}
        hasAttachments={draft.attachments.length > 0}>
        {#snippet top()}
          {#if draft.attachments.length > 0}
            <ul class="attached">
              {#each draft.attachments as f, i (f.name + i)}
                <li class="chip-file">
                  <span class="chip-name">{f.name}</span>
                  <button type="button" class="chip-x" aria-label={m.newchat_attach_remove({ nome: f.name })} disabled={draft.sending}
                    onclick={() => (draft.attachments = draft.attachments.filter((_, j) => j !== i))}>×</button>
                </li>
              {/each}
            </ul>
          {/if}
        {/snippet}
        {#snippet leading()}
          <NewChatAttach disabled={draft.sending} server={draft.serverObj} onFiles={(files) => (draft.attachments = [...draft.attachments, ...files])} />
          <NewChatMic server={draft.serverObj} disabled={draft.sending}
            onText={(t) => (text = text.trim() ? `${text.trimEnd()} ${t}` : t)} onError={(msg) => (draft.sendError = msg)} />
        {/snippet}
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
              <FolderGitPill server={draft.serverObj} cwd={draft.cwd} disabled={draft.sending} onChanged={() => draft.loadBranches()}
                root={draft.roots ? (pickFolderRoot(draft.roots, draft.cwd) ?? undefined) : undefined} />
            {/if}
            <button type="button" class="more" disabled={draft.sending} onclick={() => (moreOpen = true)}>{m.native_create_more()}</button>
          </div>
        {/snippet}
      </NewChatComposer>
    </div>
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
          onclick={() => { menu = null; draft.branch = ''; draft.newBranch = false; draft.pickExisting(null); }}>
          <span>{m.native_create_checkout_current()}{draft.branches?.current ? ` · ${draft.branches.current}` : ''}</span>
        </button>
      </li>
      <li>
        <button type="button" class="row" class:on={draft.newBranch} aria-pressed={draft.newBranch}
          onclick={() => { draft.pickExisting(null); draft.newBranch = true; draft.branch = ''; draft.base = draft.base || (draft.branches ? defaultBase(draft.branches) : ''); }}>
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
            onclick={() => { menu = null; draft.pickExisting(null); draft.branch = b; draft.newBranch = false; }}>
            <span>{b}</span><span class="muted">{m.native_create_checkout_worktree()}</span>
          </button>
        </li>
      {/each}
      {#each draft.worktrees as wt (wt.path)}
        <li>
          <button type="button" class="row" class:on={draft.existing === wt.path} aria-pressed={draft.existing === wt.path}
            onclick={() => { menu = null; draft.pickExisting(wt.path); }}>
            <span>{wt.branch ?? basename(wt.path)}</span>
            <span class="muted">{wt.sessions.length ? m.worktree_menu_ocupada({ nome: wt.sessions[0] }) : m.newchat_worktree_existing()}</span>
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
  .attached { list-style: none; margin: 0 0 4px; padding: 0; display: flex; flex-wrap: wrap; gap: 6px; }
  .chip-file {
    display: inline-flex; align-items: center; gap: 2px; max-width: 100%; padding-left: 10px;
    border-radius: var(--radius-full); background: var(--fill-subtle); font-size: var(--text-xs); color: var(--text-primary);
  }
  .chip-name { max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip-x {
    width: 44px; height: 32px; margin: -6px -8px -6px 0; display: inline-flex; align-items: center; justify-content: center;
    border: 0; background: transparent; color: var(--text-muted); font-size: 18px; cursor: pointer;
  }
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
  .middle { flex: 1; min-height: 0; position: relative; }
  .layer {
    position: absolute; inset: 0; transition: opacity 320ms var(--ease-out), transform 320ms var(--ease-out);
  }
  .list-layer { overflow-y: auto; margin: 0 calc(-1 * var(--space-3)); opacity: 0; transform: translateY(-60px); }
  .collapsed .list-layer { opacity: 1; transform: none; }
  .greeting { display: flex; flex-direction: column; align-items: center; justify-content: center; }
  .collapsed .greeting { opacity: 0; transform: translateY(40px); }
  .bottom { position: relative; overflow: hidden; flex-shrink: 0; }
  .bottom.animating { transition: height 320ms var(--ease-out); }
  .launcher, .dock { position: absolute; left: 0; right: 0; bottom: 0; transition: opacity 320ms var(--ease-out), transform 320ms var(--ease-out); }
  .launcher {
    display: flex; align-items: center; min-height: 54px; padding: 0 4px 0 14px; border-radius: 26px;
    background: var(--surface-raised); border: 1px solid var(--border-default); opacity: 0;
  }
  .collapsed .launcher { opacity: 1; }
  .collapsed .dock { opacity: 0; transform: translateY(16px); }
  .launcher-text {
    flex: 1; min-width: 0; min-height: 44px; display: flex; align-items: center; justify-content: flex-start; gap: 10px; border: 0; background: transparent;
    color: var(--text-muted); font-size: var(--text-base); text-align: left; cursor: pointer;
  }
  .launcher-mic {
    width: 44px; height: 44px; display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
    border: 0; border-radius: var(--radius-full); background: transparent; color: var(--text-secondary); cursor: pointer;
  }
  .launcher-mic:disabled { opacity: 0.5; cursor: default; }
  .sessions-btn {
    min-height: 44px; display: inline-flex; align-items: center; gap: 8px; padding: 0 12px 0 8px; border: 0;
    border-radius: var(--radius-full); background: transparent; color: var(--text-primary); cursor: pointer;
  }
  .sessions-btn:active { background: var(--fill-subtle); }
  .sessions-label { font-size: var(--text-base); font-weight: var(--fw-semibold); }
  .badge {
    min-width: 22px; height: 22px; padding: 0 6px; display: inline-flex; align-items: center; justify-content: center;
    border-radius: 11px; background: var(--fill-subtle); font-size: var(--text-xs); font-weight: 600; font-variant-numeric: tabular-nums;
  }
  @media (prefers-reduced-motion: reduce) { .layer, .bottom, .launcher, .dock { transition: none; } }
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
