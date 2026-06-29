# Sidebar multi-server (desktop) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the desktop `Sidebar.svelte` session list to parity with mobile — list sessions from all configured servers, grouped by server, rendered incrementally, with the mobile row cues (animated working indicator, cwd line, state chip).

**Architecture:** Reuse the incremental multi-server fetch pattern already in `SessionList.svelte` (`fetchSessionsForServer` + `loadGen`/slots/recompute), but output `groups: {server, sessions, error}[]` instead of a flat deduped list. The template renders a header per server group, then its rows. Cross-server operations (open/delete/rename) follow the established mobile pattern: `selectServer(serverId)` before the active-server API call. Mobile (`SessionList`, <820px) is untouched.

**Tech Stack:** Svelte 5 (runes: `$state`/`$derived`/`$props`), TypeScript, Vite. Lottie via the existing `Lottie.svelte` + `pensando.json`.

## Global Constraints

- Type gate (the project's only frontend test): `npm --prefix frontend run check` must pass with **no new** errors/warnings. There is **one pre-existing** error (`Lottie.svelte` "Cannot find module 'lottie-web'") and 7 pre-existing warnings — those are the baseline; do not introduce more.
- No frontend test runner exists; do not add one (project convention: backend pytest only, frontend gate is `check`). Each task's "test" is `check` + the listed manual verification.
- Do not touch the mobile flow: `SessionList.svelte`, `SessionCard.svelte` stay as-is.
- Cross-server ops MUST `selectServer(serverId)` before `deleteSession`/`renameSession`/`onSelect`, mirroring `SessionList.svelte` (api.ts reads the active server).
- Session names are unique per server but can collide across servers — key `editing` and the active-row highlight by `serverId` + `name`, never `name` alone.
- Commit messages in English, no `Co-Authored-By` trailer.

---

## File Structure

- `frontend/src/components/Sidebar.svelte` — the entire change (script load + handlers, template `nav`, styles). One file.
- No new files. The Lottie working-indicator block (~10 lines) is replicated inline from `SessionCard.svelte` rather than extracted — keeps the diff focused and does not touch the working mobile card. (Optional future `StateIndicator.svelte` extraction is explicitly out of scope here.)

---

## Task 1: Grouped multi-server list (data + template, existing row visuals)

Replace the active-server-only load with an incremental multi-server aggregation grouped by server, and render groups. Keep the **current** row visuals (CSS dot, name, id, ×) in this task — visual upgrade is Task 2. End state: the sidebar shows every server's sessions under a header, painting each server as it responds.

**Files:**
- Modify: `frontend/src/components/Sidebar.svelte` (script: imports, state, `load`, `handleCreate`, `handleDelete`, `onMainClick`, `saveEdit`, `pressStart`; template: the `<nav class="sess-list">` block; styles: add group-header rules)

**Interfaces:**
- Consumes (already exported): `fetchSessionsForServer(s: Server): Promise<SessionInfo[]>` (`lib/api`), `serverColor(id: string): string`, `selectServer(id)`, `listServers(): Server[]` (`lib/auth`).
- Produces (used by Task 2): `groups: Group[]` state where `interface Group { server: Server; sessions: SessionInfo[]; error: string | null }`; per-row server id available in template as `g.server.id`.

- [ ] **Step 1: Update imports**

In `frontend/src/components/Sidebar.svelte`, change the api import (line 3) to drop `getSessions` and add `fetchSessionsForServer`, and add `serverColor` to the auth import (line 4):

```ts
  import { fetchSessionsForServer, createSession, deleteSession, renameSession } from '../lib/api';
  import { listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor, clearCredentials } from '../lib/auth';
```

Add `Server` to the types import (line 7). First confirm the export name and location:

Run: `grep -rnE "export (interface|type) Server\b" frontend/src/lib/`
Expected: one match. Import `Server` as a type from wherever it is exported (e.g. `import type { Server } from '../lib/auth';`). If `Server` is declared but not exported in that file, add `export` to its declaration.

- [ ] **Step 2: Replace flat `sessions` state + `sorted` with grouped state**

Replace the `sessions` state declaration (line 18) and the `urgency`/`sorted` derived block (lines 26-32) with:

```ts
  interface Group { server: Server; sessions: SessionInfo[]; error: string | null }
  let groups = $state<Group[]>([]);

  const urgency: Record<State, number> = { awaiting_input: 0, working: 1, idle: 2, dead: 3 };
  // Ordena DENTRO de cada grupo: atividade desc, depois urgência do estado.
  function sortSessions(list: SessionInfo[]): SessionInfo[] {
    return [...list].sort((a, b) => {
      const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
      return byAct !== 0 ? byAct : urgency[a.state] - urgency[b.state];
    });
  }
```

- [ ] **Step 3: Replace `load()` with incremental multi-server aggregation**

Replace `load()` (lines 34-36) with the grouped incremental loader (mirrors `SessionList.svelte`). Note `servers` state (line 20) still exists and is reused here:

```ts
  let loadGen = 0;
  async function load() {
    const list = listServers();
    servers = list;
    if (list.length === 0) { groups = []; return; }
    const gen = ++loadGen;
    const slots = new Map<string, { sessions: SessionInfo[] | null; error: string | null }>();
    const recompute = () => {
      if (gen !== loadGen) return; // resposta de poll antigo — descarta
      const seen = new Set<string>(); // dedup global: backend compartilhado por 2 URLs não duplica
      groups = list.map((srv) => {
        const slot = slots.get(srv.id);
        if (!slot || !slot.sessions) return { server: srv, sessions: [], error: slot?.error ?? null };
        const fresh = slot.sessions.filter((s) => {
          const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        });
        return { server: srv, sessions: sortSessions(fresh), error: null };
      });
    };
    await Promise.all(list.map((srv) =>
      fetchSessionsForServer(srv)
        .then((ss) => { slots.set(srv.id, { sessions: ss, error: null }); })
        .catch((e) => { slots.set(srv.id, { sessions: null, error: e instanceof Error ? e.message : 'offline' }); })
        .finally(recompute),
    ));
  }
```

- [ ] **Step 4: Fix the session handlers for the grouped model + cross-server ops**

Replace `handleCreate` (lines 43-47), `handleDelete` (lines 48-54), `onMainClick` (lines 70-75), and `saveEdit` (lines 82-92) with versions that operate per-server and refresh via `load()`:

```ts
  async function handleCreate(name: string, cwd?: string, configDir?: string | null) {
    // O CreateSessionSheet já posicionou o servidor-alvo como ativo (selectServer).
    await createSession(name, cwd, configDir);
    onSelect(name);
    load();
  }
  async function handleDelete(name: string, serverId: string, e: MouseEvent) {
    e.stopPropagation();
    selectServer(serverId); // api.ts mira o server ativo -> aponta pro dono da sessão
    try { await deleteSession(name); } catch { /* ignora */ }
    load();
  }
  function onMainClick(name: string, serverId: string, tracked: boolean | undefined) {
    if (longPressed) { longPressed = false; return; } // foi toque longo (renomear)
    if (tracked === false) return; // sem id confiável -> não abre
    selectServer(serverId); // o Chat usa o server ativo
    onSelect(name);
  }
  async function saveEdit(old: string, serverId: string) {
    const nv = editValue.trim();
    editing = null;
    if (!nv || nv === old) return;
    selectServer(serverId);
    try {
      const r = await renameSession(old, nv);
      if (old === currentSession) onSelect(r.name);
    } catch { /* load corrige */ }
    load();
  }
```

- [ ] **Step 5: Key `editing` by serverId+name in `pressStart`**

`editing` is set from `pressStart` and compared in the template. To stop a name collision across servers from opening two editors, key it by a composite `serverId::name`. Replace `pressStart` (lines 62-66):

```ts
  function pressStart(key: string) {
    longPressed = false;
    clearTimeout(pressTimer);
    pressTimer = setTimeout(() => { longPressed = true; editing = key; editValue = key.split('::').slice(1).join('::'); }, 500);
  }
```

(The template passes the composite key; `editValue` strips the `serverId::` prefix back to the bare name. `onEditKey` and `autofocus` are unchanged.)

- [ ] **Step 6: Rewrite the `<nav>` template to render groups**

Replace the whole `<nav class="sess-list">…</nav>` block (lines 168-209) with grouped rendering. Keep the existing row internals (`.dot`, `.sess-name`, `.sess-id`/`.sess-badge`, `.sess-del`) — only wrap them in groups and thread `g.server.id` through the handlers:

```svelte
  <nav class="sess-list" aria-label="Sessões">
    {#each groups as g (g.server.id)}
      {#if !collapsed}
        <div class="grp-head" title={g.error ? `${g.server.label}: ${g.error}` : g.server.label}>
          <span class="grp-dot" style="background: {serverColor(g.server.id)};" aria-hidden="true"></span>
          <span class="grp-label">{g.server.label}</span>
          {#if g.error}<span class="grp-off">offline</span>{/if}
        </div>
      {/if}
      {#each g.sessions as s (s.name)}
        {@const rowKey = `${g.server.id}::${s.name}`}
        <div class="sess-row" class:active={g.server.id === activeId && s.name === currentSession}>
          {#if editing === rowKey}
            <input
              class="sess-edit"
              bind:value={editValue}
              use:autofocus
              onkeydown={(e) => onEditKey(e, s.name)}
              onblur={() => saveEdit(s.name, g.server.id)}
              aria-label="Renomear sessão"
            />
          {:else}
            <button
              class="sess-main"
              class:untracked={s.tracked === false}
              title={collapsed ? s.name : (s.tracked === false ? 'claude aberto sem --session-id: transcript nao rastreavel' : 'Toque longo pra renomear')}
              onpointerdown={() => pressStart(rowKey)}
              onpointerup={pressEnd}
              onpointerleave={pressEnd}
              onpointercancel={pressEnd}
              oncontextmenu={(e) => e.preventDefault()}
              onclick={() => onMainClick(s.name, g.server.id, s.tracked)}
            >
              <span class="dot dot--{s.state}" aria-hidden="true"></span>
              {#if !collapsed}<span class="sess-name">{s.name}</span>{/if}
              {#if !collapsed}
                {#if s.tracked === false}
                  <span class="sess-badge" title="sem --session-id: nao rastreavel">sem id</span>
                {:else if shortId(s)}
                  <span class="sess-id" title={`session-id: ${shortId(s)}…`}>#{shortId(s)}</span>
                {/if}
              {/if}
            </button>
            {#if !collapsed}
              <button class="sess-del" onclick={(e) => handleDelete(s.name, g.server.id, e)} aria-label={`Apagar ${s.name}`}>×</button>
            {/if}
          {/if}
        </div>
      {/each}
    {/each}
  </nav>
```

- [ ] **Step 7: Add group-header styles**

In the `<style>` block, add right after the `.sess-list` rule (line 297):

```css
  .grp-head {
    display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2) var(--space-2) 4px;
    font-size: var(--text-xs); font-weight: 600; color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.04em;
  }
  .grp-head:not(:first-child) { margin-top: var(--space-2); }
  .grp-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  .grp-label { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .grp-off { color: var(--warning); font-weight: 600; text-transform: none; letter-spacing: 0; }
```

- [ ] **Step 8: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: a line ending `1 ERRORS 7 WARNINGS` (the pre-existing baseline — no new problems). If the count rose, read the new error and fix it before committing.

- [ ] **Step 9: Manual verification**

The frontend dev server is already running (systemd user service `claude-pocket-frontend.service`, Vite HMR). Open the app in a desktop-width browser window (≥820px) with 2 servers configured (one reachable, one offline). Confirm:
- Each server appears as a header with its colored dot; its sessions listed under it.
- The reachable server's group paints immediately; the offline one shows `offline` in its header within ≤4s (does not block the other).
- Clicking a session opens its chat; a session on the non-active server still opens correctly (active server switches to it).
- Delete (`×`) and long-press rename still work, on either server's sessions.
- Collapse (the top toggle) hides headers + labels, leaving the status dots.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/components/Sidebar.svelte
git commit -m "feat(frontend): desktop sidebar lists all servers' sessions, grouped + incremental"
```

---

## Task 2: Adopt mobile row visuals (Lottie indicator, cwd, state chip)

Upgrade each sidebar row to the mobile cues, sized for the 270px column: animated Lottie "pensando" working indicator (replacing the CSS dot), a cwd line under the name, and a compact state chip.

**Files:**
- Modify: `frontend/src/components/Sidebar.svelte` (script: add Lottie import + state metadata + `cwdParts`; template: row internals from Task 1; styles: cwd + chip + `.lead`, remove now-unused `.dot*` if no longer referenced)

**Interfaces:**
- Consumes: `groups` from Task 1; `Lottie.svelte` (`data`, `size`, `loop`, `autoplay`, `frame` props) + `pensando.json` (both already used by `SessionCard.svelte`).
- Produces: none (terminal visual task).

- [ ] **Step 1: Add Lottie import + state metadata + cwd helper**

After the existing imports (line 7), add:

```ts
  import Lottie from './Lottie.svelte';
  import pensando from '../lib/lottie/pensando.json';

  const stateLabels: Record<State, string> = { working: 'exec', idle: 'pronto', awaiting_input: 'aguardando', dead: 'encerrado' };
  const stateColors: Record<State, string> = { working: 'var(--accent)', idle: 'var(--success)', awaiting_input: 'var(--warning)', dead: 'var(--error)' };
  const stateChipBg: Record<State, string> = {
    working: 'var(--accent-dim)', idle: 'rgba(52,199,89,0.12)',
    awaiting_input: 'rgba(255,159,10,0.14)', dead: 'rgba(255,69,58,0.12)',
  };
  const STATIC_FRAME = 0; // f0 = anel cheio e simétrico (frames do meio ficam ralos)

  // cwd -> prefixo truncável + basename que nunca encolhe (mesma lógica do SessionCard).
  function cwdParts(cwd: string | undefined) {
    const p = (cwd ?? '').replace(/\/+$/, '');
    const i = p.lastIndexOf('/');
    return i < 0 ? { prefix: '', base: p } : { prefix: p.slice(0, i + 1), base: p.slice(i + 1) };
  }
```

- [ ] **Step 2: Replace the row's leading dot with the Lottie indicator**

In the `.sess-main` button (Task 1's template), replace the `<span class="dot dot--{s.state}" aria-hidden="true"></span>` line with:

```svelte
              <span class="lead" aria-hidden="true">
                {#if s.state === 'working'}
                  <Lottie data={pensando as any} size={18} loop autoplay />
                {:else}
                  <Lottie data={pensando as any} size={18} loop={false} autoplay={false} frame={STATIC_FRAME} />
                {/if}
              </span>
```

- [ ] **Step 3: Replace the name/id block with name + cwd stack + state chip**

In the `.sess-main` button, replace everything between the `.lead` span and the button's closing `</button>` (the `{#if !collapsed}<span class="sess-name">…` name line and the `{#if !collapsed}{#if s.tracked === false}…#{shortId}…{/if}` block) with:

```svelte
              {#if !collapsed}
                <span class="row-info">
                  <span class="name-row">
                    <span class="sess-name">{s.name}</span>
                    {#if s.tracked === false}<span class="sess-badge" title="sem --session-id: nao rastreavel">sem id</span>{/if}
                  </span>
                  {#if s.cwd}
                    {@const cp = cwdParts(s.cwd)}
                    <span class="cwd" title={s.cwd}><span class="cwd-prefix">{cp.prefix}</span><span class="cwd-base">{cp.base}</span></span>
                  {/if}
                </span>
                <span class="state-chip" style="color: {stateColors[s.state]}; background: {stateChipBg[s.state]};">{stateLabels[s.state]}</span>
              {/if}
```

The per-row `#{shortId}` id is dropped (identity now reads from the group header + cwd). This likely makes `shortId` (lines 78-81) unused — Step 5's `check` will flag it; if so, delete the `shortId` function.

- [ ] **Step 4: Update row styles**

In `<style>`, change `.sess-main` (lines 303-307): drop `height: 38px`, add `min-height: 46px`. Then add these rules (after `.sess-main`):

```css
  .row-info { display: flex; flex-direction: column; gap: 1px; flex: 1; min-width: 0; }
  .name-row { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
  .cwd { display: flex; min-width: 0; font-family: var(--font-mono); font-size: 10px; }
  .cwd-prefix { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }
  .cwd-base { flex: 0 0 auto; white-space: nowrap; color: var(--text-secondary); }
  .state-chip {
    flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
  }
  .lead { width: 18px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center; }
```

- [ ] **Step 5: Remove the now-unused `.dot` rules (guarded)**

The leading dot is gone. Confirm no markup still uses `class="dot`:

Run: `grep -n 'class="dot' frontend/src/components/Sidebar.svelte`
Expected: no matches. If none, delete the `.dot { … }`, `.dot--working/awaiting_input/idle/dead` rules and `@keyframes dot-pulse` (lines 331-339). Leave the footer `.srv-dot` rules (different class). If `grep` shows matches, keep the `.dot*` rules.

- [ ] **Step 6: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: `1 ERRORS 7 WARNINGS` baseline (no new problems). If `shortId` is flagged unused, delete it and re-run.

- [ ] **Step 7: Manual verification**

Desktop width (≥820px), HMR live:
- A working session shows the animated "pensando" ring; idle/awaiting/dead show the static ring (state-tinted).
- Each row shows name + cwd (basename never truncated away) + a compact state chip.
- Collapsed mode still shows only the indicator (no name/cwd/chip).
- Mobile (<820px) unchanged — `SessionList` cards look exactly as before.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/Sidebar.svelte
git commit -m "feat(frontend): adopt mobile row cues in desktop sidebar (working anim, cwd, state chip)"
```

---

## Self-Review notes

- **Spec coverage:** §1 data layer → Task 1 Step 3 (incremental load, `loadGen`, dedup, `groups`). §2 grouped layout → Task 1 Steps 6-7. §2 mobile cues → Task 2. §3 server switcher (footer markup unchanged; active server = create/open target) → preserved; cross-server ops via `selectServer` in Task 1 Step 4. §4 reuse (no shared list component; indicator replicated inline) → File Structure note. Collapsed behavior → Task 1 Step 6 + Task 2 Step 7.
- **Cross-server correctness:** every mutating/navigating handler calls `selectServer(serverId)` first (Task 1 Step 4), matching `SessionList.svelte`.
- **Name collisions across servers:** active highlight uses `g.server.id === activeId && s.name === currentSession`; `editing` keyed by `serverId::name` (Task 1 Steps 5-6).
- **No new dependencies, no new files, mobile untouched.**
