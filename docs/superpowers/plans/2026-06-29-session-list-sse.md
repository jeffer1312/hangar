# Session-list SSE (sub-project C) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 5s session-list polling with a per-server SSE: a backend `GET /api/sessions/events` that pushes the session list when it changes, and `SessionList`/`Sidebar` consuming one independent `EventSource` per server instead of `setInterval`.

**Architecture:** `list_events()` is an async generator (in `sse.py`) that emits the `list_with_state()` snapshot on connect, then loops every ~1.5s, diffs the serialized list, and emits a `sessions` event only on change plus a `ping` heartbeat; served via `EventSourceResponse` at `/api/sessions/events`. The frontend opens one `EventSource` per configured server (auth via `?token=` cross-origin, mirroring `openEventStream`); each stream fills that server's slice of the existing aggregation, with its own `onerror` → offline. One server failing never touches the others.

**Tech Stack:** Python 3.14, FastAPI, `sse-starlette` (`EventSourceResponse`), pytest. Frontend: Svelte 5, TypeScript, native `EventSource`.

## Global Constraints

- Backend test runner: `cd backend && uv run pytest -v` (real TDD — failing test first). Do not break the suite (note: `test_state_classifier.py` has 2 PRE-EXISTING failures unrelated to this work — they must stay at 2, no new failures).
- Frontend gate: `npm --prefix frontend run check`. Baseline since `lottie-web` was installed is `0 ERRORS` + ~7 pre-existing warnings — do not raise error/warning counts. No frontend test runner; frontend tasks verify via `check` + manual.
- Per-server isolation is a hard requirement: one `EventSource` per server, one `onerror`→offline per server, one slice per server. A server dropping must not affect the others.
- SSE auth: `require_auth` already accepts `?token=` query (header → query → cookie). Cross-origin EventSource MUST use `?token=<server.token>` (it cannot send `Authorization` and carries no cookie cross-origin). Same-origin uses `withCredentials`.
- Preview text / open-chat SSE (`merged_events`) and the per-session route are UNTOUCHED.
- Commit messages in English, no `Co-Authored-By` trailer.

---

## File Structure

- `backend/app/sse.py` — add `list_events()` generator (sits beside `merged_events`).
- `backend/app/api.py` — add `GET /api/sessions/events` route returning `EventSourceResponse(list_events())`.
- `frontend/src/lib/api.ts` — add `openSessionsStream(server)` → `EventSource` for a specific server.
- `frontend/src/screens/SessionList.svelte` — swap the 5s poll for a per-server EventSource.
- `frontend/src/components/Sidebar.svelte` — same swap for the desktop grouped list.
- Tests: `backend/tests/test_list_sse.py`.

---

## Task 1: Backend — `list_events()` generator + route

**Files:**
- Modify: `backend/app/sse.py` (add `list_events`)
- Modify: `backend/app/api.py` (add the route)
- Test: `backend/tests/test_list_sse.py`

**Interfaces:**
- Produces: `async def list_events(poll: float = 1.5, ping_every: int = 7)` — yields `{"event": "sessions", "data": <json SessionInfo[]>}` on connect and on every change, and `{"event": "ping", "data": "{}"}` every `ping_every` ticks. `GET /api/sessions/events` returns `EventSourceResponse(list_events())`.

- [ ] **Step 1: Write the failing test**

Create `backend/tests/test_list_sse.py`. Drive the generator directly with a mocked `list_with_state` so no tmux/live server is needed. The generator calls `list_with_state()` on a module-level `SessionRegistry` instance `_list_registry` — patch that.

```python
import asyncio, json
from app import sse

class _Info:
    def __init__(self, name, state):
        self.name, self.state, self.cwd, self.jsonl, self.tracked, self.last_activity = name, state, "/p", f"/x/{name}.jsonl", True, None
    def model_dump(self, mode="json"):
        return {"name": self.name, "state": self.state, "cwd": self.cwd, "jsonl": self.jsonl, "tracked": self.tracked, "last_activity": self.last_activity}

async def _take(gen, n):
    out = []
    async for ev in gen:
        out.append(ev)
        if len(out) >= n:
            break
    return out

def test_emits_snapshot_on_connect(monkeypatch):
    async def fake_list():
        return [_Info("cc", "idle")]
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 1))
    assert evs[0]["event"] == "sessions"
    assert json.loads(evs[0]["data"])[0]["name"] == "cc"

def test_emits_again_only_on_change(monkeypatch):
    seq = [[_Info("cc", "idle")], [_Info("cc", "idle")], [_Info("cc", "working")]]
    calls = {"i": 0}
    async def fake_list():
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # connect-emit (idle), unchanged (idle, no emit), then working -> 2nd emit
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["state"] == "idle"
    assert json.loads(evs[1]["data"])[0]["state"] == "working"

def test_ping_emitted_on_cadence(monkeypatch):
    async def fake_list():
        return [_Info("cc", "idle")]
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # ping_every=1 -> after the connect snapshot, the next tick has no change but emits a ping
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=1), 2))
    assert evs[0]["event"] == "sessions"
    assert evs[1]["event"] == "ping"
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd backend && uv run pytest tests/test_list_sse.py -v`
Expected: FAIL (`sse.list_events` / `sse._list_registry` do not exist).

- [ ] **Step 3: Implement `list_events` in `backend/app/sse.py`**

The module already has `_registry = SessionRegistry()` (used by the jsonl watcher). Add a clearly-named instance for the list stream and the generator. Add after the existing `_registry`:

```python
# Instancia stateless pro stream de lista (separada do _registry do jsonl_watcher pra clareza).
_list_registry = SessionRegistry()


async def list_events(poll: float = 1.5, ping_every: int = 7):
    """SSE da LISTA de sessoes. Emite o snapshot de list_with_state() na conexao e, num loop de
    `poll`s, reemite SO quando o resultado muda (estado via markers do A; membership por re-listar).
    Heartbeat 'ping' a cada `ping_every` ticks (alimenta o watchdog do front). Fail-loud: excecao
    do list_with_state propaga e encerra o stream (o EventSource do cliente reconecta)."""
    last = None
    ticks = 0
    while True:
        infos = await _list_registry.list_with_state()
        data = json.dumps([i.model_dump(mode="json") for i in infos], ensure_ascii=False)
        if data != last:
            last = data
            yield {"event": "sessions", "data": data}
        ticks += 1
        if ping_every and ticks % ping_every == 0:
            yield {"event": "ping", "data": "{}"}
        await asyncio.sleep(poll)
```

Confirm `SessionInfo` (returned by `list_with_state`) is a pydantic model with `model_dump` — it is the same `response_model=list[SessionInfo]` type returned by `GET /api/sessions`. If `model_dump(mode="json")` errors on any field, fall back to `model_dump()` + `json.dumps(..., default=str)`.

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cd backend && uv run pytest tests/test_list_sse.py -v`
Expected: PASS (all 3).

- [ ] **Step 5: Add the route in `backend/app/api.py`**

`EventSourceResponse` and `require_auth` are already imported. Add the route. The literal `/api/sessions/events` (3 segments) cannot be shadowed by `/api/sessions/{name}/events` (4 segments); place it near the existing `GET /api/sessions` for tidiness:

```python
@app.get("/api/sessions/events", dependencies=[Depends(require_auth)])
async def sessions_events():
    from app.sse import list_events
    return EventSourceResponse(list_events())
```

- [ ] **Step 6: Verify the route + full suite**

Run: `cd backend && uv run python -c "import app.api"` — expect no error.
Run: `cd backend && uv run pytest -q` — expect all pass except the 2 known pre-existing `test_state_classifier.py` failures (count unchanged).
Smoke (optional, backend running): `curl -N -H "Authorization: Bearer $CP_AUTH_TOKEN" http://127.0.0.1:8765/api/sessions/events` should stream a `sessions` event then pings.

- [ ] **Step 7: Commit**

```bash
git add backend/app/sse.py backend/app/api.py backend/tests/test_list_sse.py
git commit -m "feat(backend): list-level SSE /api/sessions/events (snapshot on connect, push on change)"
```

---

## Task 2: Frontend — `openSessionsStream` + SessionList over SSE

**Files:**
- Modify: `frontend/src/lib/api.ts` (add `openSessionsStream`)
- Modify: `frontend/src/screens/SessionList.svelte` (poll → per-server EventSource)

**Interfaces:**
- Consumes: the `sessions`/`ping` events from Task 1; the `Server` type (`{id, baseUrl, token, label}`) from `lib/auth`.
- Produces: `openSessionsStream(s: Server): EventSource`.

- [ ] **Step 1: Add `openSessionsStream` to `api.ts`**

Mirror the existing `openEventStream` (its same-origin/cross-origin + `?token` logic), but for a SPECIFIC server (not the active one):

```ts
// EventSource da LISTA de UM servidor (baseUrl/token explicitos). ?token cross-origin (EventSource
// nao manda header e cross-origin nao leva cookie); withCredentials same-origin. Por-servidor:
// cada um tem o seu, falha isolada.
export function openSessionsStream(s: Server): EventSource {
  const isSameOrigin = !s.baseUrl || s.baseUrl === window.location.origin;
  const url = isSameOrigin
    ? `${s.baseUrl}/api/sessions/events`
    : `${s.baseUrl}/api/sessions/events?token=${encodeURIComponent(s.token)}`;
  return new EventSource(url, { withCredentials: isSameOrigin });
}
```

Ensure `Server` is imported in `api.ts` (the file already references `Server` in `fetchSessionsForServer` — reuse that import).

- [ ] **Step 2: Read the current `SessionList.svelte` load logic**

Read `frontend/src/screens/SessionList.svelte` in full. The current `loadSessions` already builds per-server slots and a `recompute()` that aggregates + dedups; `onMount` runs it once and on a 5s `setInterval`. You keep the slot/recompute aggregation but feed slots from EventSources instead of fetches.

- [ ] **Step 3: Replace the poll with per-server EventSources**

In `SessionList.svelte`, replace the `loadSessions` fetch loop and the `onMount` interval with a per-server EventSource manager. Keep the EXACT existing aggregation (the `recompute` that builds `AggSession[]` + `serverErrors` from slots, dedup by `${jsonl ?? cwd}::${name}`, server ordering). Only the slot SOURCE changes from fetch to stream. Add:

```ts
  import { openSessionsStream } from '../lib/api';
  import type { SessionInfo } from '../lib/types';

  let streams = new Map<string, EventSource>();           // server.id -> EventSource
  // keep the existing per-server `slots` map + `recompute()` from the current incremental loadSessions

  function connect(list: Server[]) {
    for (const [id, es] of streams) {                     // fecha streams de servers removidos
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;                    // ja conectado
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
        slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data) as SessionInfo[], error: null });
        loading = false;
        recompute();
      });
      es.onerror = () => {                                 // falha ISOLADA: so este server. EventSource auto-reconecta.
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
      };
      streams.set(s.id, es);
    }
    recompute();
  }

  onMount(() => {
    servers = listServers();
    connect(servers);
    return () => { for (const es of streams.values()) es.close(); streams.clear(); };
  });
```

- Wherever the existing code reassigns `servers = listServers()` after add/remove/rename of a server, also call `connect(servers)` so streams open/close to match.
- Remove `setInterval(loadSessions, 5000)`. Remove the now-unused `getAllSessions`/`fetchSessionsForServer`/`loadSessions` (keep `createSession`/`deleteSession`). Keep `loading` true until the first `sessions` arrives (set `false` in the listener).

Do NOT reinvent the dedup/ordering — reuse the current `recompute`/`slots` shapes verbatim; only swap the data source.

- [ ] **Step 4: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: `0 ERRORS`, pre-existing warning count unchanged. Remove dead imports if `check` flags them.

- [ ] **Step 5: Manual verification**

Mobile width (<820px), dev server live (HMR), 2+ servers:
- List renders from the SSE snapshot on open; the Network tab shows a persistent `events` connection per server and NO recurring 5s `/api/sessions` GET.
- A session state change updates its card within ~1.5s with no client request.
- One server offline → only its group/cards show offline; the others keep updating; it reconnects when back.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/api.ts frontend/src/screens/SessionList.svelte
git commit -m "feat(frontend): SessionList consumes per-server SSE instead of polling /api/sessions"
```

---

## Task 3: Frontend — Sidebar (desktop) over SSE

**Files:**
- Modify: `frontend/src/components/Sidebar.svelte` (poll → per-server EventSource)

**Interfaces:**
- Consumes: `openSessionsStream(server)` (Task 2); the `sessions`/`ping` events (Task 1).

- [ ] **Step 1: Read the current `Sidebar.svelte` load logic**

Read `frontend/src/components/Sidebar.svelte` in full. It already aggregates multiple servers into `groups` (a `load()` with `loadGen`/`slots`/`recompute`, polled on a 5s `setInterval`). You feed `groups` from EventSources instead of fetches, keeping the grouped aggregation and the `selectServer`-before-op handlers untouched.

- [ ] **Step 2: Replace the poll with per-server EventSources**

In `Sidebar.svelte`, replace `load()`'s fetch loop and the `onMount` 5s interval with the same per-server EventSource manager as `SessionList`, but feeding the Sidebar's existing grouped `recompute()` (header per server, dedup, `sortSessions`). Keep that `recompute` and the `slots` shape verbatim; only the source changes:

```ts
  import { openSessionsStream } from '../lib/api';

  let streams = new Map<string, EventSource>();
  // reuse the existing per-server `slots` map + the grouped `recompute()` that builds `groups`

  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
        slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data), error: null });
        recompute();
      });
      es.onerror = () => {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
      };
      streams.set(s.id, es);
    }
    recompute();
  }

  onMount(() => {
    servers = listServers();
    connect(servers);
    return () => { for (const es of streams.values()) es.close(); streams.clear(); };
  });
```

- After create/rename/delete and server add/remove (handlers that reassign `servers` via `listServers()`), call `connect(servers)` so streams match the server set. The `selectServer(serverId)`-before-op logic in `handleDelete`/`saveEdit`/`onMainClick` stays exactly as is.
- Remove `setInterval(load, 5000)` and the now-unused `fetchSessionsForServer` import. Keep the `slots`/`recompute` output type (`groups`) exactly as the component already uses it.

- [ ] **Step 3: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: `0 ERRORS`, warning count unchanged. Remove unused imports.

- [ ] **Step 4: Manual verification**

Desktop width (≥820px, `DesktopShell`), HMR live, 2+ servers:
- The grouped sidebar renders from the SSE snapshot; no recurring 5s GET in the Network tab.
- State changes appear within ~1.5s with no client request.
- One server offline → only its group shows offline; others keep streaming; reconnects on return.
- Collapse, create, rename, delete, and server switching all still work.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/Sidebar.svelte
git commit -m "feat(frontend): desktop Sidebar consumes per-server SSE instead of polling"
```

---

## Self-Review notes

- **Spec coverage:** §1 backend `list_events` + route → Task 1. §2 `openSessionsStream` + SessionList → Task 2; Sidebar → Task 3. Per-server isolation (one EventSource / one onerror / one slice per server) → Tasks 2-3 `connect()`. Snapshot-on-connect removes the initial GET → Task 1 (first loop iteration emits) consumed by Tasks 2-3 (`loading=false` on first `sessions`). Heartbeat/ping → Task 1; v1 reconnect relies on native `EventSource` auto-reconnect (the ping is available for a future watchdog but not required).
- **Auth:** cross-origin uses `?token=`; same-origin `withCredentials` — Task 2 Step 1 mirrors `openEventStream`.
- **Fail-loud backend:** `list_events` lets `list_with_state` exceptions propagate (ends stream → client reconnects); no swallow.
- **Type consistency:** `openSessionsStream(s: Server): EventSource`, the `sessions` event carries `SessionInfo[]`, slots keyed by `server.id` — consistent across Tasks 1-3.
- **No new deps. Open-chat SSE untouched. The 2 pre-existing `test_state_classifier` failures are orthogonal.**
