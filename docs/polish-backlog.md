# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

## Peers (the cross-machine mesh) have no UI at all (2026-08-07)

`backend/peers.json` is the only way to add, remove, enable or disable a remote machine. There is
no `/api/peers` route and `app/peers.py` only exposes `_load()` — nothing writes the file. So the
mesh that powers `cp-send <server>::<session>` is invisible in the app: a user who did not author
the project has no way to discover it exists, and the author has to remember the file path and the
JSON shape months later.

The two lists also collide by name, which is its own confusion: the account menu's **Servidores**
section (`+ Adicionar servidor`) is the list of *which backend this browser talks to* — client-side,
per device — while `peers.json` is *which machines this backend can relay messages to*. Same word,
different thing, and only one of them is visible.

What it needs: a settings screen listing the peers with an enable/disable toggle, reachability
status, and add/remove. Writing is the risky part — `peers.json` holds the tokens for the whole
mesh and `scripts/cp_panel_common.py:81` already notes a half-finished write would take the mesh
down — so the write path has to be atomic (temp file + rename), never a partial rewrite.

## "Adicionar servidor" dialog hides the token field (2026-08-07)

`submitAddServer` (`frontend/src/screens/SessionList.svelte:492-509`) reads two pieces of state,
`addUrl` and `addToken`, and passes both to `addServer(url, token)`. But the rendered dialog shows
a single input labelled "Colar URL do servidor (com token)" — the token field is not on screen, so
`addToken` is always empty on that path.

It happens to work because the QR path (`handleScanServer`, same file, :512-527) accepts a URL
carrying the credential as a query param (`?token=…`, plus an optional `?api=` when the API lives on
another origin), and pasting that same URL into the one visible field works by accident. Either
render the token field the submit handler already expects, or drop `addToken` and make the
URL-with-token the documented, single input — but the current state is a form whose handler and
markup disagree.

## Stability under load — never measured (2026-07-29)

Performance **was** measured and is a non-issue at this scale; stability under load was not, and
that's the open question. Numbers from the machine that runs this daily (483 processes, 4 live
sessions), so nobody has to re-measure the cheap part:

| Path | Cost | Note |
|---|---|---|
| `procinfo._proc_children_map()` | 4.9 ms | reads `stat` of every process — grows with the machine, not with sessions |
| `tmux list-panes -a` | 2.1 ms | one fork for all sessions |
| `capture_pane`, per session | 2.0 ms | one fork **each** → 8 ms at 4 sessions, ~40 ms at 20 |
| `_open_jsonl` over 9 descendants | 0.30 ms | returned **nothing**, exactly as its own comment predicts |

A poll cycle is ~15 ms. Both heavy paths are linear (processes, sessions), so they only matter on
a busy host or with many sessions — not here.

What's actually untested is what a soak run would show, and none of it is visible in a 15 ms
timing: SSE connections left open for hours (the 25s watchdog reconnecting on a half-open socket),
file descriptors and `asyncio.to_thread` workers over a long run, the `capture_pane` burst
described at `registry.py:787`, and what happens when sessions are created and killed repeatedly
while the phone is subscribed.

Worth doing as a real experiment (N sessions, SSE open, forced reconnects, watch RSS/fd count over
hours) rather than by reading more code. Deferred deliberately — nothing observed is broken.

## Structural debt in the session list (2026-07-16)

> **Items 1–3 DONE (2026-07-17).** All three extractions shipped in full. Real numbers below.

Measured while building the kanban board. Nothing is broken — this is about the shape of
the code, and it already cost real bugs this session. In order of value per risk:

1. ✅ **DONE (2026-07-17) — Extract the multi-server SSE aggregation into a store.** The
   `slots`/`recompute`/`connect` trio now lives in `lib/sessions.ts` (pure dedup/order/classify,
   7 unit tests) + `lib/sessionsStore.svelte.ts` (a refcounted singleton: one `openSessionsStream`
   per server for the whole app, `retain`/`release` per consumer, Board's parse strategy — try/catch
   + `onServersChanged`). The three drifting copies (`Sidebar`, `SessionList`, `Board`) are gone;
   `Canvas` is a fourth consumer that reuses the same store instead of a fourth copy.

2. ✅ **DONE (2026-07-17) — `ConfirmDialog.svelte`.** Extracted as a chassis
   (`.confirm-backdrop`/`.confirm-card`/`.confirm-actions`); the two non-plain confirms (resume with
   a candidate list, add-server with an input) pass their body via a `{#snippet}` children slot, with
   that body's CSS kept in `Sidebar`. The shared `withServer` helper moved to `lib/auth.ts`.

3. ✅ **DONE (2026-07-17) — `SessionContextMenu.svelte`.** The row's context menu is now its own
   component, owning `menuMuted`/`branchView`/`chainView`; it also uses the shared `withServer`
   from `lib/auth.ts`.

Real result: `Sidebar.svelte` went from **1859 lines / 44 `$state`** to **1570 lines / 37 `$state`**
(the backlog's ~1100/~25 estimate was optimistic — the three items were done in full; the rest of
what remains is legitimate list template/CSS, not duplication).

**The bigger fish, still deliberately NOT done — and the gap is widening.** `Sidebar.svelte` and
`SessionList.svelte` are *the same feature written twice*: the session list, one for desktop and
one for mobile. At the time of writing that was 1570 + 1371 = 2941 lines combined. Re-measured
**2026-08-07: 2051 + 1550 = 3601** — 660 lines added to a duplication that was already the largest
item here, so every feature since has been paid for twice. CLAUDE.md already flags the risk ("make
the change in BOTH views and verify BOTH — they drift apart easily"). The three extractions above
all survive (`lib/sessions.ts`, `lib/sessionsStore.svelte.ts`, `ConfirmDialog.svelte`,
`SessionContextMenu.svelte`, `lib/auth.ts` — all still in place), so the remaining bulk really is
the duplicated view, not the parts already factored out. Worth its own session with the repo at
rest.

Not worth touching: the kebab (30 lines), the hover preview (already a component), and
resize/collapse (5 states, cohesive with the sidebar chrome).

## From phone testing (2026-06-25)

- **Mobile UI needs real adjustment** (general). The current layout is a working first
  cut on a phone but not refined — spacing, touch targets, widths, scrolling. Do a proper
  design pass with the front-end skills on a running phone session.
  *Note (2026-08-07): this one has no definition of done, which is why it survives every
  sweep. Either give it a concrete list of screens and defects, or close it.*

## Notes
- These are UX/feature items; the core loop (chat, live state, input, statusline) works.
