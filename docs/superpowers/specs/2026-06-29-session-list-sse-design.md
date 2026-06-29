# Session-list SSE (sub-project C) — design

**Date:** 2026-06-29
**Status:** approved, pending implementation plan

## Context

Sub-project C of F2 (replace the 5s session-list polling with push). A (state via Claude hooks)
is done: `list_with_state` now reads a cheap in-memory marker map, falling back to the pane only for
un-hooked sessions. C is the **delivery layer**: the frontend stops polling `/api/sessions` every 5s
and instead holds an SSE connection per server that pushes the list when it changes. C is what
actually stops the *frontend* network poll. B (tmux control mode for membership) is later — until
then, C's backend loop detects membership changes by re-listing (cheap, thanks to A).

## Problem

`SessionList.svelte` (mobile) and `Sidebar.svelte` (desktop) each `setInterval(loadSessions, 5000)`,
fetching `/api/sessions` from every configured server every 5s. With N servers that is N GETs (plus
a CORS preflight per cross-origin server) every 5s, forever — visible network chatter even when
nothing changed.

## Design

### 1. Backend — `GET /api/sessions/events` (list-level SSE)

A new SSE endpoint, auth via the existing `require_auth` (which already accepts `?token=` query —
needed because `EventSource` can't send an `Authorization` header and cross-origin requests carry no
cookie). Streamed with `EventSourceResponse` (sse-starlette), mirroring the existing per-session
`/api/sessions/{name}/events`.

The generator `list_events()`:

- **Emits the current `list_with_state()` snapshot immediately on connect** (event `sessions`,
  `data` = JSON `SessionInfo[]`) so the client renders without a separate initial GET.
- Then **loops every ~1.5 s**: recompute `list_with_state()`, serialize, compare to the last emitted
  serialization; emit a `sessions` event **only when it changed**. This catches both state changes
  (from A's markers) and membership changes (a session appeared / died — caught by re-listing).
- Emits a `ping` event every 10 s (heartbeat for the frontend liveness watchdog, same role as the
  chat SSE's ping).
- **Fail-loud:** an exception from `list_with_state` propagates and ends the stream (the client's
  `onerror` then reconnects) — never silently swallowed.

The 1.5 s cadence is cheaper than today's 5 s × N-clients GETs and makes the list feel more live, at
zero frontend network cost. `list_with_state`'s pane fallback (the 150 ms spinner recheck) runs only
for un-hooked sessions, so a fully-hooked machine's loop is light.

One loop runs per open connection (per client × per server). For a single user with one active
viewport that is a handful of cheap loops. A shared per-server broker (like `PreviewBroker`) is a
possible future optimization — **out of scope for v1** (YAGNI).

### 2. Frontend — one independent EventSource per server

`SessionList.svelte` and `Sidebar.svelte` replace their `setInterval(loadSessions, 5000)` with **one
`EventSource` per configured server**: `new EventSource(`${server.baseUrl}/api/sessions/events?token=${server.token}`)`.

- **Per-server isolation (explicit requirement):** each server has its own EventSource, its own slice
  in the aggregated list, and its own `onerror` handler. If one server drops, only its EventSource
  errors → only its group/slice is marked offline; the other servers keep streaming, untouched. No
  shared connection, no shared failure — the same isolation the current per-server fetch has, now
  over a persistent stream.
- On each `sessions` message: parse `SessionInfo[]`, replace that server's slice, and re-render
  through the existing aggregation/grouping (incremental groups in `SessionList`, server-grouped in
  `Sidebar`). The dedup-by-`jsonl::name` stays.
- **Reconnect:** rely on `EventSource`'s built-in auto-reconnect; add a liveness watchdog on the
  `ping` (reconnect if no ping within ~25 s, mirroring the chat SSE). `onerror` marks that server
  offline (its group shows the offline note) until its stream recovers.
- The snapshot-on-connect covers the initial load, so the standalone `/api/sessions` GET poll and its
  5 s interval are removed from both components. `fetchSessionsForServer` / `getSessions` may remain
  for any non-streaming caller, but the list screens no longer poll.

`SessionList` (<820px) and `Sidebar` (≥820px in `DesktopShell`) never mount together, so a client
holds at most one list-SSE per server.

## Out of scope

- B (tmux control mode for membership) — C's loop detects membership by re-listing until then.
- A is already shipped (markers feed the cheap `list_with_state`).
- A shared per-server SSE broker (multi-client de-duplication of the backend loop).
- The open-chat per-session SSE (`merged_events`) is untouched.

## Data flow

`list_events() loop (≈1.5s) → list_with_state() (marker map, A) → diff vs last → emit 'sessions'
on change (+ 'ping' heartbeat) → per-server EventSource on the client → replace that server's slice →
aggregate + render`. No frontend polling; one persistent connection per server.

## Testing / verification

- **Backend (pytest):** test `list_events()` — emits a `sessions` snapshot on connect; emits again
  only when `list_with_state` output changes (mock it to return A then A then B → expect connect-emit,
  no second emit, then an emit); a `ping` is produced on the heartbeat cadence (can shorten the
  interval via a parameter/patch in the test). Drive the generator directly (no live server).
- **Frontend (`npm run check` + manual):** at mobile (<820px) and desktop (≥820px) with multiple
  servers — the list updates live via SSE; the Network tab shows the persistent `events` connections
  and **no** recurring 5 s `/api/sessions` GETs; taking one server offline marks only its group
  offline while the others keep updating; the offline server's stream recovers on reconnect.

## Files

- Backend: `backend/app/sse.py` (or a new `list_sse.py`) — `list_events()` generator; `backend/app/api.py` — the `GET /api/sessions/events` route.
- Frontend: `frontend/src/lib/api.ts` — an `openSessionsStream(server)` helper returning an `EventSource`; `frontend/src/screens/SessionList.svelte` and `frontend/src/components/Sidebar.svelte` — swap poll → per-server EventSource.
- Tests: `backend/tests/test_list_sse.py`.
