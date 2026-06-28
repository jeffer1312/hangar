# Chat perf: windowing + pagination + queue delivery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make long/compacted Claude Code sessions usable on a phone by rendering only a bounded window of recent messages, shipping only the tail (plus the missed gap) over REST/SSE instead of the whole jsonl on every request and reconnect, and durably delivering queued prompts to the tmux tty once the pane returns to a text-accepting state.

**Architecture:** Three independent fixes layered front-to-back. **A (frontend windowing)** bounds what *mounts*: `MessageList.svelte` renders a tail slice (`events.slice(windowStart)`) anchored synchronously at first paint, with an at-bottom tail cap so a live session can't re-grow into a freeze, and an optional scroll-up reveal — a pure client-side slice of the already-loaded `events` array, no backend call. **B (backend pagination)** bounds what *transfers*: `merged_history` keeps its full read + merge for correct queue dedup but returns a `HistoryPage{events, has_older}` tail/`before`-cursor slice, and `TranscriptTailer.follow(from_pos)` replaces the `pos=0` whole-file re-ship with a tail-K backfill (`since`-resolved gap when available) so SSE reconnects self-heal instead of re-streaming the file. **C (durable-queue delivery)** adds one `delivered` boolean to each queue entry plus a state-gated, claim-before-send `drain` triggered on the StateMonitor overlay→deliverable transition, so prompts enqueued while an overlay/`awaiting_input` was up are pushed to the tty exactly once when it closes — never on a blind eager send into a menu.

**Tech Stack:** Backend — Python 3.14, FastAPI, `sse-starlette` EventSourceResponse, Pydantic, `asyncio.to_thread`, stdlib file `seek/readline/tell`, `threading.Lock` + tmp+replace atomic writes; tmux via the existing `tmux`/`terminal_input` helpers; tests with `pytest` (run from `backend/` via `.venv/bin/python -m pytest`). Frontend — Svelte 5 runes (`$state`/`$derived`/`$effect`), TypeScript, Vite, native `EventSource`/`fetch` + `URLSearchParams`, IntersectionObserver-free `onScroll` reveal, PWA service worker (`sw.ts`); verified via `npm run check` (svelte-check + tsc). No new runtime dependency, no virtualization library, no new HTTP endpoint for C.

## Ordering

Ship in the order **A → B → C**. A delivers the immediate, self-contained mobile win and the other two harden it.

1. **A — frontend windowing (first, highest impact).** The confirmed freeze is component *mount* count (~5,000 Svelte bubbles), not transfer. A is a client-side slice of events already in memory, so it works standalone with zero backend change and removes the freeze on its own. Critical correctness gates folded in from review: anchor `windowStart` *synchronously* at declaration (an `$effect` re-anchor runs after first commit and would still mount everything), and add the at-bottom tail cap so a long-running live session doesn't silently re-grow past the window.

2. **B — backend pagination + bounded SSE backfill (second).** With A shipped, the remaining cost is the whole-file re-ship on every (mobile-frequent) reconnect and the one-time full payload. The load-bearing piece is `follow(from_pos=tail)` to stop the `pos=0` re-ship; REST `HistoryPage`/`before`/`has_older` pagination is the larger surface and should be justified by measurement after A, behind a whole-history-consumer audit (`deriveActivity`, pending-reconcile, preview-dedup all read the full array) and a service-worker cache-version bump (the `list → HistoryPage` shape change is a stale-bundle footgun).

3. **C — durable-queue delivery (third).** Independent of A/B; it fixes the overlay/`awaiting_input` delivery case the boot-readiness wait doesn't cover. Sequenced last because it touches the send path, not the perf path: the minimum is the deliverability gate inside the single `send_prompt` chokepoint (stop blind-typing into an overlay); the `delivered`+drain auto-redelivery subsystem is the convenience layer on top.

**A ↔ B dependency (state explicitly):** A as a client-side slice of already-loaded events is standalone and needs no backend change. But *truly* bounding memory and transport requires B — once B paginates the initial load, the full transcript is no longer in the client array, so A's reveal-older must call B's `loadOlder()` and the prepend (`events = [...older, ...events]`) shifts every absolute index. The A/B contract must be owned explicitly: either A keeps the full array (B off) **or** A adjusts `windowStart += page.events.length` on every B prepend so the absolute window cursor tracks the shifted array (inferring append-vs-prepend from a length delta alone is a bug). Do not ship A's reveal-older and B's pagination together without wiring that prepend adjustment.

---

---

## Fix A: Frontend windowing / virtualization

Mount only the tail of the transcript, never the whole thing. The freeze is component **mount** count (~5,000 Svelte components on a 15k-line session), so we bound what mounts with a single reactive slice — no virtualization library, no `IntersectionObserver`, no sentinel. Two critique-driven corrections to the original design: (1) the window cursor must be initialised **synchronously** at declaration (a re-anchor `$effect` runs *after* the first DOM commit, so it would still mount all 5,000 on the target session — the blocker); (2) the window is **count-from-end** and the tail only advances **while the user is glued to the bottom**, which both caps growth on a live session (no silent re-freeze) and never jumps the read position of a user scrolled up. Scroll-up-to-reveal-older is deliberately dropped (ponytail) — it re-mounts everything and re-freezes by its own admission; v1 ships the bounded tail only.

---

### Task A1 — Pin the window-bounds math (TDD, failing first)

The only non-trivial logic is the *advance-vs-freeze-vs-shrink* branch — extract it to a pure helper with a runnable assert check (no frontend test runner exists; `npm run check` is svelte-check/tsc only).

**Files**
- Create `/home/jefferson/pessoal/claude-pocket/frontend/src/lib/window.check.ts`
- Create `/home/jefferson/pessoal/claude-pocket/frontend/src/lib/window.ts`

**Steps**

1. Write the failing check FIRST (`window.check.ts`):

```ts
import { strict as assert } from 'node:assert';
import { windowStartFor, nextWindowEnd } from './window';

// fatia: clampa em 0, nunca negativo/NaN
assert.equal(windowStartFor(5000, 120), 4880);
assert.equal(windowStartFor(80, 120), 0);
assert.equal(windowStartFor(0, 120), 0);

// encolheu (reset / /clear) -> re-ancora na cauda nova (independe de atBottom)
assert.equal(nextWindowEnd(false, 30, 5000), 30);
assert.equal(nextWindowEnd(true, 30, 5000), 30);
// colado no fim -> acompanha a cauda
assert.equal(nextWindowEnd(true, 5001, 5000), 5001);
// rolado pra cima -> congela (sem pulo)
assert.equal(nextWindowEnd(false, 5001, 5000), 5000);
// ja na cauda, colado -> no-op (garante terminacao do effect)
assert.equal(nextWindowEnd(true, 5000, 5000), 5000);

console.log('window.check OK');
```

2. Run (from anywhere — relative import resolves to the file's dir):

```
npx --yes tsx /home/jefferson/pessoal/claude-pocket/frontend/src/lib/window.check.ts
```

Expected fail: `Cannot find module './window'`.

3. Minimal implementation (`window.ts`):

```ts
// Janela de render do chat: monta SO os ultimos N eventos (a cauda), nunca o transcript inteiro.
// Contagem-A-PARTIR-DO-FIM (relativa): um prepend futuro (paginacao backend / fix B) nao corrompe a
// janela, porque ela e sempre medida do fim.

/** Indice inicial (inclusivo) da fatia visivel, dado o fim da janela e o tamanho. Clampa em 0. */
export function windowStartFor(windowEnd: number, size: number): number {
  return Math.max(0, windowEnd - size);
}

/** Proximo fim de janela:
 *  - encolheu (reset / /clear) -> re-ancora na cauda nova (senao a slice fica fora do array = chat em branco);
 *  - colado no fim -> acompanha a cauda (remonta o topo SO com o usuario no fundo = sem pulo);
 *  - rolado pra cima -> congela. */
export function nextWindowEnd(atBottom: boolean, len: number, windowEnd: number): number {
  if (windowEnd > len) return len;   // transcript encolheu: clampa
  if (atBottom) return len;          // gruda no fim: janela segue a cauda
  return windowEnd;                  // lendo historico: congela
}
```

4. Re-run the same command. Expected pass: `window.check OK`.

5. Commit: `test(frontend): pin chat window bounds (windowStartFor/nextWindowEnd)`

---

### Task A2 — Window the MessageList render (synchronous init + tail slice + maintenance)

One file. Init the cursor synchronously so the *first* paint is already the tail (fixes the blocker), slice the keyed `{#each}`, and fold window maintenance into the existing auto-scroll effect.

**Files**
- Modify `/home/jefferson/pessoal/claude-pocket/frontend/src/components/MessageList.svelte`

**Steps**

1. Add the import (after line 12, with the other `../lib` imports):

```svelte
  import { windowStartFor, nextWindowEnd } from '../lib/window';
```

2. Add the window state right after `atBottom` (line 30):

```svelte
  // Janela de render: monta SO os ultimos WINDOW eventos (a cauda). Sessao longa/compactada (milhares de
  // linhas no .jsonl) montando tudo = tempestade de mount/layout = congela no celular. windowEnd inicia
  // SINCRONO em events.length (o prop ja vem populado: o Chat so monta o MessageList apos loadHistory) ->
  // ja no PRIMEIRO paint a fatia e a cauda, sem montar os 5000 e so depois encolher.
  // WINDOW = botao de calibragem (ajuste no device real); tool_result e filtrado depois, entao bolhas < WINDOW.
  const WINDOW = 120;
  let windowEnd = $state(events.length);
```

3. Replace `visibleEvents` (line 59):

```svelte
  // Renderiza so tool_use (tool_result vira card) e SO dentro da janela [windowEnd-WINDOW, windowEnd].
  // Fatiamos o array CRU por indice ANTES de filtrar -> windowEnd/length sao indices crus; filtrar a
  // fatia mantem o {#each} keyed (ev.id) valido. toolResults (acima) segue sobre o array INTEIRO, entao
  // um tool_use na janela ainda resolve seu result.
  const visibleEvents = $derived(
    events.slice(windowStartFor(windowEnd, WINDOW), windowEnd).filter(ev => ev.kind !== 'tool_result')
  );
```

4. Replace the auto-scroll effect (lines 66-73) to also maintain `windowEnd`:

```svelte
  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do cronometro/status
  // atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    const len = events.length;
    void pending.length;
    void dockH; // composer cresceu (anexo/multilinha) -> re-scrolla pra ultima msg limpar o glass
    void preview; // preview cresce token a token -> acompanha o fundo enquanto o usuario esta colado
    // Mantem a janela: encolheu (reset/clear) re-ancora na cauda; colado no fim acompanha a cauda
    // (remonta o topo SO com o usuario no fundo = sem pulo); rolado pra cima congela. Termina: ao
    // escrever windowEnd=len o effect re-roda e nextWindowEnd vira no-op.
    const next = nextWindowEnd(atBottom, len, windowEnd);
    if (next !== windowEnd) windowEnd = next;
    if (!atBottom) return;
    tick().then(scrollToBottom);
  });
```

`scrollToBottom`/`rafScroll`, `onScroll`, `toolResults`, the `{#each}`, preview/pending/spinner/OptionButtons blocks, and all CSS stay untouched.

5. Run svelte-check:

```
npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check
```

Expected: green except the **2 known pre-existing** MessageList type errors; **no new** errors (`windowEnd: number`, both helpers typed/used).

6. Commit: `perf(frontend): window MessageList to last N events (mount only the tail)`

---

### Task A3 — Regression + device calibration

No DOM unit runner exists; verify in the browser on the real session and tune the one knob.

**Files**
- (calibration only) `/home/jefferson/pessoal/claude-pocket/frontend/src/components/MessageList.svelte` — `const WINDOW`

**Steps**

1. Re-run the pure check and svelte-check (must stay green / only the 2 known errors):

```
npx --yes tsx /home/jefferson/pessoal/claude-pocket/frontend/src/lib/window.check.ts
npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check
```

2. Browser-verify on the real 15,386-line session (devtools, iOS-style viewport):
   - **Open**: mounted bubble count ≈ `WINDOW` (not thousands) on first paint — no freeze. (`$0`/`document.querySelectorAll('.messages-inner > *').length`.)
   - **Live append while at bottom**: new event auto-scrolls in; top bubble unmounts silently (you're pinned to bottom — no jump); mounted count stays ≈ `WINDOW`. Leave it running a while → count does **not** grow (no re-freeze).
   - **Scroll up + live append**: read position does **not** jump; window is frozen (older-than-window not revealed — expected for v1).
   - **Scroll back to bottom**: window catches up to the tail and re-pins.
   - **`/clear` (reset path)**: chat is **not** blank — re-anchors to the small fresh transcript.
   - **No new transforms/layers** introduced → no iOS black-glitch regression.

3. If a tall device shows a visible gap above the tail (window too small) or the open is still heavy (too large), tune only `const WINDOW` and re-verify.

4. Commit (only if `WINDOW` changed): `chore(frontend): calibrate chat WINDOW size on device`

---

**Ponytail notes:** `[slice(windowStartFor(windowEnd,WINDOW), windowEnd)] + [advance only at-bottom]` → skipped: scroll-up-to-reveal-older, IntersectionObserver sentinel, scrollHeight-delta compensation, the `revealing` lock, any virtualization lib, and coupling to backend pagination (Fix B); add reveal-older only when deep phone scrollback is an actual reported need — and then prefer `content-visibility:auto` on revealed bubbles over manual scroll math. `WINDOW=120` is the deliberate calibration knob, not a guess.

**Ecc notes:** blocker closed by synchronous `windowEnd = $state(events.length)` (first paint is the tail, not all ~5,000); silent re-freeze closed by `nextWindowEnd` advancing the tail **only when `atBottom`** (mounted count bounded to ~`WINDOW` on a live session) while the count-from-end measure makes Fix A immune to a future Fix B front-prepend; blank-chat-on-`/clear` closed by the `windowEnd > len` shrink-clamp; the negative/NaN slice silent-failure is clamped (`Math.max(0, …)`) and pinned by the `window.check.ts` asserts.

---

I have what I need. The actual code confirms the lazy path: `tail_pump` calls `.follow()` with no args, the frontend already re-seeds via `loadHistory`/`reset`, and the SSE comment literally documents the "replays the whole transcript on every (re)connect" cost. Here is the plan section.

---

## Fix B: Backend pagination + windowed SSE backfill

The critiques collapse this fix to almost nothing, and they are right. The mobile freeze is DOM mount (Fix A), so REST `/history` stays **full** — the ponytail HIGH (drop the `HistoryPage`/`before`/`limit`/`CursorNotFound` apparatus + frontend `loadOlder`/`hasOlder`) and the ECC HIGH (truncating the `events` array silently breaks `deriveActivity`, the pending-reconcile loop, and the preview-dedup that all read the whole array) point the same way, and keeping `/history` full also kills the ECC A+B-prepend HIGH and the PWA `list→object` stale-cache footgun for free. The single change that earns its keep is bounding the SSE re-ship: `TranscriptTailer.follow()` backfills only the **tail** (last `_BACKFILL_LINES` lines) instead of the whole file from `pos=0` on every reconnect. The `since`/`offset_after_id` cursor is rejected (ponytail medium); the only resulting hole — a *non-foreground* reconnect that missed > `_BACKFILL_LINES` lines — is closed for the common case (foreground-after-long-background) by re-seeding `/history` on `visibilitychange`, with the residual ceiling marked.

---

### Task 1 — Backend: backfill only the tail on (re)connect

**Files**
- Create: `/home/jefferson/pessoal/claude-pocket/backend/tests/test_transcript_tail.py`
- Modify: `/home/jefferson/pessoal/claude-pocket/backend/app/transcript.py`

**Steps**

1. Failing test first — `backend/tests/test_transcript_tail.py`:

```python
import asyncio
import json

import pytest

from app.transcript import TranscriptTailer, _BACKFILL_LINES


def _user(uid: str, text: str) -> str:
    return json.dumps({"type": "user", "uuid": uid,
                       "message": {"role": "user", "content": text}}) + "\n"


def test_tail_offset_zero_when_few_lines(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b"))
    # <= max_lines linhas -> backfill do inicio (offset 0): sessao curta mantem o backfill completo.
    assert TranscriptTailer(f)._tail_offset(10) == 0


def test_tail_offset_returns_kth_from_last(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b") + _user("u3", "c"))
    t = TranscriptTailer(f)
    pos = t._tail_offset(2)                 # so as 2 ultimas linhas (u2, u3)
    evs, _ = t._read_from(pos)
    assert [e.id for e in evs] == ["u2", "u3"]


def test_tail_offset_ignores_partial_last_line(tmp_path):
    f = tmp_path / "s.jsonl"
    # ultima linha sem \n = append em voo: nao conta nem desloca o tail (espelha _read_from).
    f.write_text(_user("u1", "a") + _user("u2", "b") + '{"type":"user","uuid":"u3"')
    t = TranscriptTailer(f)
    pos = t._tail_offset(1)                 # 2 linhas completas; parcial ignorada -> tail = u2
    evs, _ = t._read_from(pos)
    assert [e.id for e in evs] == ["u2"]


def test_tail_offset_missing_file_is_zero(tmp_path):
    assert TranscriptTailer(tmp_path / "nope.jsonl")._tail_offset(5) == 0


@pytest.mark.asyncio
async def test_follow_backfills_only_tail(tmp_path, monkeypatch):
    monkeypatch.setattr("app.transcript._BACKFILL_LINES", 2)
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b") + _user("u3", "c"))
    got: list[str] = []

    async def consume():
        async for ev in TranscriptTailer(f).follow():
            got.append(ev.id)
            if len(got) == 2:
                return

    await asyncio.wait_for(consume(), timeout=5)
    assert got == ["u2", "u3"]             # u1 (fora do tail) NAO veio no backfill
```

2. Run + expected fail:

```
cd backend && .venv/bin/python -m pytest tests/test_transcript_tail.py -q
```
Expected: collection error / red — `ImportError: cannot import name '_BACKFILL_LINES' from 'app.transcript'` (and `AttributeError: '_tail_offset'`).

3. Minimal implementation — `backend/app/transcript.py`. Add the module const right after the imports (after line 8 `from app.models import ChatEvent`):

```python
# Backfill do SSE: re-envia so as ULTIMAS N linhas do transcript em cada (re)conexao, nao o arquivo
# inteiro. Antes o follow() comecava em pos=0 e re-shippava dezenas de MB a cada reconexao do mobile
# (background/foreground, watchdog). 200 e a maneta de calibracao: cobre o gap de uma reconexao normal
# (poucos segundos) com folga; sessao com <= 200 linhas mantem o backfill completo (offset 0).
_BACKFILL_LINES = 200
```

Add `_tail_offset` as a method on `TranscriptTailer` (right after `_read_from`):

```python
    def _tail_offset(self, max_lines: int) -> int:
        # Offset do inicio da (max_lines)-esima linha a partir do fim -> o follow() faz backfill so do
        # tail. Conta LINHAS completas (terminadas em \n) sem parsear JSON (mais barato que _read_from).
        # <= max_lines linhas, ou arquivo ausente -> 0 (backfill do inicio = comportamento antigo).
        # ponytail: varre o arquivo pra frente sem parse; reverse-seek so se o disco virar gargalo.
        if not self.path.exists():
            return 0
        starts: list[int] = []
        with self.path.open(encoding="utf-8", errors="replace") as fh:
            while True:
                start = fh.tell()
                line = fh.readline()
                if not line:
                    break
                if not line.endswith("\n"):
                    break  # ultima linha incompleta (append em voo): ignora, nao registra o start
                starts.append(start)
        if len(starts) <= max_lines:
            return 0
        return starts[-max_lines]
```

Change the first line of `follow()` (currently `pos = 0` at line 206):

```python
    async def follow(self) -> AsyncIterator[ChatEvent]:
        # Backfill so do TAIL (ultimas _BACKFILL_LINES linhas), nao o arquivo inteiro. _tail_offset
        # devolve 0 quando ha poucas linhas -> sessao curta mantem o backfill completo. A leitura roda
        # no threadpool (nao bloqueia o loop); custo <= o _read_from(0) de antes (varredura sem parse).
        pos = await asyncio.to_thread(self._tail_offset, _BACKFILL_LINES)
        # backfill inicial + cada append: a leitura de arquivo roda no threadpool (nao bloqueia o loop).
        evs, pos = await asyncio.to_thread(self._read_from, pos)
        for ev in evs:
            yield ev
        async for _ in awatch(self.path.parent):
            evs, pos = await asyncio.to_thread(self._read_from, pos)
            for ev in evs:
                yield ev
```

`follow()` keeps its zero-arg signature, so `sse.tail_pump` (`TranscriptTailer(path).follow()`), the `__reset__` rebind, and the `_StubTailer*.follow(self)` stubs in `test_sse.py` all keep working untouched — `sse.py`, `api.py`, `models.py`, and the SSE tests need **no** change.

4. Run + expected pass:

```
cd backend && .venv/bin/python -m pytest tests/test_transcript_tail.py tests/test_jsonl_parser.py tests/test_sse.py -q
```
Expected: green. Then full suite — `cd backend && .venv/bin/python -m pytest -q` — only the 2 KNOWN `test_state_classifier` failures remain (`test_tailer_yields_existing_then_new` still passes: its 1-line file has ≤ 200 lines → offset 0 → full backfill).

5. Commit message:

```
feat(sse): backfill only the tail on (re)connect, not the whole transcript
```

---

### Task 2 — Frontend: re-seed history on foreground so the tail backfill can't drop missed messages

No frontend test runner exists (only `npm run check`); verification is the type check plus a manual mobile trace. This is the gap guard for the tail-K backfill, not new feature surface.

**Files**
- Modify: `/home/jefferson/pessoal/claude-pocket/frontend/src/screens/Chat.svelte`

**Steps**

1. Replace `onVisible()` (currently at lines 244-246):

```js
  // App voltou pro foreground (mobile suspende a conexao no background). Agora o backfill do SSE so
  // traz o TAIL (ultimas _BACKFILL_LINES linhas), entao um background LONGO pode ter perdido mais que
  // isso. Re-seed do history (REST, completo e ordenado) ANTES de reconectar fecha o buraco; o backfill
  // tail do SSE so faz a ponte ate a subscricao (dedup por id, sem reordenar). Falha aqui NAO trava a
  // tela (o connectSSE/onerror re-sincroniza) -> ignora e segue. Reconexoes de blip (watchdog/onerror)
  // continuam SO com o tail-K: cobrem poucos segundos sem re-shippar o arquivo inteiro.
  async function onVisible() {
    if (document.visibilityState !== 'visible') return;
    try {
      const fresh = await getHistory(sessionName);
      events = fresh;
      rebuildIndex();
    } catch { /* offline momentaneo: o connectSSE/onerror cuida do re-sync */ }
    connectSSE();
  }
```

`getHistory`, `events`, `rebuildIndex`, and `sessionName` are already in scope (used by `loadHistory`); no new import, no new type. `/history` keeps returning a bare `ChatEvent[]` — no `HistoryPage`, no `before`/`limit`, no `loadOlder`/`hasOlder`.

2. Run + expected pass:

```
cd frontend && npm run check
```
Expected: same result as baseline — the 2 KNOWN pre-existing `MessageList` type errors remain, **no new** errors (this change only touches already-typed symbols).

3. Manual verify (real 15,386-line session, mobile/devtools):
   - Open the app: chat seeds via `/history` (full, REST), SSE backfill is now only the tail — confirm in the Network tab the `/events` initial frame is small, not the whole file.
   - Trigger a watchdog/onerror reconnect (toggle network ~5s): SSE re-subscribes and ships only the tail; no duplicate bubbles (idIndex dedup), no gap.
   - Background the app, let the session run > 200 lines, foreground: `onVisible` re-seeds `/history` → no missing middle, no error screen on a transient failure.
   - `/clear`: the existing `reset` event still re-seeds from the new transcript.

4. Commit message:

```
fix(chat): re-seed history on foreground so tail backfill can't drop missed messages
```

---

**Ponytail notes:** rejected the whole REST pagination apparatus the ponytail critic flagged as HIGH×2 + medium (`HistoryPage`, `before`/`limit`, `CursorNotFound`, `since`/`offset_after_id`, `models.py`/`api.py`/`types.ts`/`api.ts` shape changes, frontend `loadOlder`/`hasOlder`) — Fix B is one const + one method + one changed line in `transcript.py` plus one foreground re-seed; `/history` stays full on purpose, and the before-cursor "load older" pages get built only if, with Fix A shipped, transfer is MEASURED as the next bottleneck.

**Ecc notes:** keeping `/history` full avoids silently truncating the `events` array that `deriveActivity`/pending-reconcile/preview-dedup depend on (HIGH), the A+B front-prepend `windowStart` corruption (HIGH), and the PWA `list→object` stale-cache footgun; the tail backfill is line-bounded (hard cap, no client-controlled size) and strictly cheaper than today's full re-parse, the foreground re-seed is non-fatal (catch → never an error screen) and closes the common missed-gap, residual ceiling = a non-foreground reconnect that misses > 200 lines self-heals on the next foreground or `reset` — marked, not silent.

---

## Fix C: Durable-queue delivery (delivered-mark + idle drain)

Add ONE boolean field `delivered` to each queue entry plus a state-gated drain, and move the over-engineering critics agreed on into a single chokepoint: the deliverability gate lives **inside `send_prompt`** (root cause — `/input` and the drain both route through it, so neither can blind-type into an open AskUserQuestion/picker overlay and mis-navigate it). `send_prompt` returns `"sent"` / `"deferred"`; `/input` queues `delivered=True` when it actually typed, `delivered=False` when it deferred. A drain fires on the StateMonitor's existing overlay→idle transition (no new poll loop, no endpoint, no sidecar), claims-1-sends-1 atomically under the existing `_append_lock` (single-flight across N phone tabs, no double-send), reverts only on a provably pre-send `"deferred"` (a post-keystroke failure stays delivered = at-most-once for a non-idempotent tty), and a cheap queue read short-circuits before any `capture_pane` subprocess so zero-pending reconnects stay free. Display is untouched: `merged_history`/`follow` keep ignoring `delivered` (no bubble flicker).

### Task 1 — Queue schema + atomic claim/revert primitives

**Files:** Modify `/home/jefferson/pessoal/claude-pocket/backend/app/pqueue.py`, Modify `/home/jefferson/pessoal/claude-pocket/backend/tests/test_pqueue.py`

1. **Failing test first** — append to `tests/test_pqueue.py`:

```python
def test_append_default_pending_and_eager_delivered():
    PromptQueue("s").append("pendente")
    PromptQueue("s").append("eager", delivered=True)
    rows = PromptQueue("s").load()
    assert rows[0]["delivered"] is False
    assert rows[1]["delivered"] is True


def test_claim_undelivered_flips_and_is_idempotent():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=True)
    claimed = PromptQueue("s").claim_undelivered()
    assert [c["text"] for c in claimed] == ["a"]              # so a pendente
    assert all(r["delivered"] for r in PromptQueue("s").load())
    assert PromptQueue("s").claim_undelivered() == []          # 2a vez: nada (idempotente)


def test_claim_limit_one():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=False)
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["a"]
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["b"]


def test_claim_respects_min_ts():
    e = PromptQueue("s").append("antiga", delivered=False)
    assert PromptQueue("s").claim_undelivered(min_ts=e["ts"] + 1000) == []
    assert PromptQueue("s").load()[0]["delivered"] is False     # nao reivindicada


def test_claim_ignores_legacy_entry_without_key():
    # Entrada legada (escrita antes do campo): `is False` ESTRITO -> NAO reivindicada (senao um
    # upgrade re-enviaria todo prompt antigo ja entregue).
    p = PromptQueue("s")
    p.path.write_text('{"id":"old1","text":"legada","ts":1.0}\n', encoding="utf-8")
    assert p.claim_undelivered() == []
    assert "delivered" not in p.load()[0]


def test_set_delivered_reverts():
    e = PromptQueue("s").append("x", delivered=True)
    PromptQueue("s").set_delivered(e["id"], False)
    assert PromptQueue("s").load()[0]["delivered"] is False


def test_merged_history_ignores_delivered_flag(tmp_path):
    # delivered NAO afeta exibicao: entrada entregue mas ainda nao gravada no transcript continua
    # aparecendo como bubble queued- (o dedup por texto so a remove quando o user_msg real cai).
    j = tmp_path / "t.jsonl"
    j.write_text("", encoding="utf-8")
    PromptQueue("s").append("oi claude", delivered=True)
    hist = pqueue.merged_history("s", str(j))
    assert any(e.id.startswith("queued-") and e.text == "oi claude" for e in hist)
```

2. **Run + expect fail** — `(cd backend && .venv/bin/python -m pytest tests/test_pqueue.py -q)` → fails: `TypeError: append() got an unexpected keyword argument 'delivered'` and `AttributeError: 'PromptQueue' object has no attribute 'claim_undelivered'`.

3. **Minimal implementation** — in `pqueue.py`, extract the atomic write and add the field + two primitives (replace `append`, add `_write_atomic`, `claim_undelivered`, `set_delivered`):

```python
    def _write_atomic(self, rows: list[dict]) -> None:
        # Escrita atomica (tmp + replace) pra um reader nunca pegar o arquivo pela metade.
        tmp = self.path.with_suffix(".jsonl.tmp")
        tmp.write_text(
            "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows), encoding="utf-8"
        )
        tmp.replace(self.path)

    def append(self, text: str, delivered: bool = False) -> dict:
        # delivered=False por padrao = enfileirada mas NAO digitada na TUI (o /input passa True quando
        # o send_prompt realmente digitou). So entradas False sao drenadas -> sem isto um upgrade
        # re-enviaria toda entrada legada (= double-send em massa).
        entry = {"id": uuid.uuid4().hex, "text": text, "ts": time.time(), "delivered": delivered}
        # ponytail: lock global serializa o read-modify-write; 2 POSTs /input concorrentes (handlers
        # sync no threadpool) senao liam as mesmas rows e um sobrescrevia o outro (entrada perdida).
        # upgrade: lock per-path se o throughput de uma sessao virar gargalo.
        with _append_lock:
            rows = self.load()
            rows.append(entry)
            if len(rows) > _MAX_ENTRIES:
                rows = rows[-_MAX_ENTRIES:]
            self._write_atomic(rows)
        return entry

    def claim_undelivered(self, min_ts: float = 0.0, limit: int | None = None) -> list[dict]:
        """Reivindica (atomicamente) entradas ainda nao entregues: vira delivered=True e devolve as
        reivindicadas. Sob _append_lock -> com N drains concorrentes so UM pega cada entrada (os
        outros pegam []) = single-flight, sem double-send. `is False` ESTRITO: legada (sem a chave) ou
        ja entregue NAO entra. min_ts poda entradas de sessao antiga (pre-/clear)."""
        with _append_lock:
            rows = self.load()
            claimed = []
            for r in rows:
                if r.get("delivered") is False and float(r.get("ts") or 0.0) >= min_ts:
                    r["delivered"] = True
                    claimed.append(dict(r))
                    if limit is not None and len(claimed) >= limit:
                        break
            if claimed:
                self._write_atomic(rows)
            return claimed

    def set_delivered(self, entry_id: str, value: bool) -> None:
        """Marca UMA entrada (por id) como delivered=value e reescreve atomico. Usado pra reverter um
        claim quando o envio nao chegou a tocar a TUI (provadamente pre-envio)."""
        with _append_lock:
            rows = self.load()
            for r in rows:
                if str(r.get("id")) == entry_id:
                    r["delivered"] = value
                    break
            else:
                return
            self._write_atomic(rows)
```

`merged_history`, `follow`, `_entry_event` stay byte-identical (they never read `delivered`).

4. **Run + expect pass** — `(cd backend && .venv/bin/python -m pytest tests/test_pqueue.py -q)` → all pass (4 prior + 7 new).

5. **Commit:** `feat(pqueue): delivered flag + atomic claim/revert primitives`

### Task 2 — Extract `is_overlay` helper in state.py

**Files:** Modify `/home/jefferson/pessoal/claude-pocket/backend/app/state.py`, Modify `/home/jefferson/pessoal/claude-pocket/backend/tests/test_state_classifier.py`

1. **Failing test first** — append to `tests/test_state_classifier.py`:

```python
def test_is_overlay_true_with_nav_footer():
    pane = "alguma conversa\n● resposta\n────────\n  Esc to cancel · Enter to select\n"
    assert state_mod.is_overlay(pane) is True


def test_is_overlay_false_without_footer():
    assert state_mod.is_overlay("● PONG\n❯ \n") is False
```

2. **Run + expect fail** — `(cd backend && .venv/bin/python -m pytest tests/test_state_classifier.py -q -k is_overlay)` → `AttributeError: module 'app.state' has no attribute 'is_overlay'`.

3. **Minimal implementation** — in `state.py`, add the module fn (just below `_FOOTER_RE`) and call it from `StateMonitor.stream`:

```python
def is_overlay(pane_text: str) -> bool:
    # Overlay so-TUI aberto: rodape de navegacao por teclas no FUNDO do pane (ultimas 8 linhas — nao o
    # pane todo, senao a MESMA frase citada na conversa/scrollback dava falso-positivo). Cobre pickers
    # (/model) e paineis (/status, /config, /help) alem do AskUserQuestion. Fonte unica de "overlay"
    # (StateMonitor e terminal_input.deliverable usam esta).
    return bool(_FOOTER_RE.search("\n".join(pane_text.splitlines()[-8:])))
```

Replace the inline line in `StateMonitor.stream` (currently `overlay = bool(_FOOTER_RE.search("\n".join(pane.splitlines()[-8:])))`) with `overlay = is_overlay(pane)`. Behavior identical.

4. **Run + expect pass** — `(cd backend && .venv/bin/python -m pytest tests/test_state_classifier.py -q)` → the 2 new `is_overlay` tests pass; the 2 KNOWN pre-existing failures stay (unchanged, unrelated).

5. **Commit:** `refactor(state): extract is_overlay helper`

### Task 3 — Deliverability gate in `send_prompt` + `deliverable()` + `drain()`

**Files:** Modify `/home/jefferson/pessoal/claude-pocket/backend/app/terminal_input.py`, Modify `/home/jefferson/pessoal/claude-pocket/backend/tests/test_terminal_input.py`

1. **Failing test first** — in `tests/test_terminal_input.py`, add imports `from app import pqueue` / `from app.pqueue import PromptQueue`, update the two existing `send_prompt` cases for the gate + return, and add the drain/deliverable cases:

```python
@pytest.fixture
def tmp_queue(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_send_prompt_literal_then_enter():
    # Gate: pane entregavel (sessao viva, sem overlay) + marcador de ready -> envia e devolve "sent".
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value="? for shortcuts\n"), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "corrige o bug") == "sent"
    assert sk.call_args_list == [
        call("cc", "corrige o bug", literal=True),
        call("cc", "Enter"),
    ]


def test_send_prompt_defers_on_overlay():
    # Overlay aberto (rodape de navegacao) -> NAO digita as cegas; devolve "deferred", zero teclas.
    pane = "● plano\n────────\n  Esc to cancel · Enter to select\n"
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value=pane), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "oi") == "deferred"
    sk.assert_not_called()


def test_send_prompt_rejects_control_chars():
    with pytest.raises(ValueError):
        TerminalInput().send_prompt("cc", "bad\x00null")


def test_deliverable_false_when_no_session(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: False)
    assert terminal_input.deliverable("cc") is False


def test_deliverable_true_on_capture_error(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: True)
    def boom(name, lines=200):
        raise OSError("capture falhou")
    monkeypatch.setattr(terminal_input.tmux, "capture_pane", boom)
    assert terminal_input.deliverable("cc") is True   # degrada pro envio de hoje, sem regressao


def test_drain_sends_pending_and_marks_delivered(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    PromptQueue("cc").append("dois", delivered=False)
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: sent.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 2
    assert sent == ["um", "dois"]
    assert all(e["delivered"] for e in PromptQueue("cc").load())


def test_drain_noop_and_reverts_when_overlay(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: "deferred")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert PromptQueue("cc").load()[0]["delivered"] is False   # revertida (nao perdida)


def test_drain_does_not_revert_on_send_failure(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    def boom(self, name, text):
        raise RuntimeError("tty caiu no meio")
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt", boom)
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    # at-most-once: permanece True -> NAO re-enfileira -> nao digita 2x um prompt nao-idempotente.
    assert PromptQueue("cc").load()[0]["delivered"] is True


def test_drain_cheap_check_skips_capture_when_nothing_pending(tmp_queue, monkeypatch):
    PromptQueue("cc").append("ja entregue", delivered=True)
    called = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: called.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert called == []   # nem chamou send_prompt (e nem capture_pane)


def test_drain_skips_entries_before_start_ts(tmp_queue, tmp_path, monkeypatch):
    PromptQueue("cc").append("velha", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2999-01-01T00:00:00Z"}\n', encoding="utf-8")  # start_ts > ts da entrada
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 0 and sent == []
```

2. **Run + expect fail** — `(cd backend && .venv/bin/python -m pytest tests/test_terminal_input.py -q)` → `AttributeError: module 'app.terminal_input' has no attribute 'deliverable'` / `... 'drain'`, and the updated `send_prompt` cases fail until the gate/return land.

3. **Minimal implementation** — in `terminal_input.py` add the imports and the two module fns, and gate `send_prompt` (add the deliverability short-circuit + `return` value):

```python
from app.pqueue import PromptQueue, _transcript_start_ts
from app.state import classify, is_overlay
```

```python
def deliverable(name: str) -> bool:
    # Pode entregar texto livre AGORA? False se a sessao morreu (defer p/ recriacao, sem queimar 12s no
    # _wait_input_ready) ou se ha overlay/menu aberto (digitar as cegas navegaria o menu errado). Erro
    # de captura (sessao viva, pane ileg.) -> True: degrada pro envio de hoje, sem regressao.
    if not tmux.has_session(name):
        return False
    try:
        pane = _capture(name)
    except Exception:
        return True
    state, _, _, _ = classify(pane)
    return state != "awaiting_input" and not is_overlay(pane)


def drain(name: str, jsonl: str) -> int:
    """Entrega ao tty as entradas pendentes (delivered=False) quando o pane volta a aceitar texto.
    Retorna quantas entregou. claim-1-envia-1: um crash entre o claim e o envio deixa NO MAXIMO 1
    entrada 'stranded', nao o lote, e recheca o overlay (via send_prompt) a cada iteracao."""
    q = PromptQueue(name)
    # ECC: cheap-check SEM subprocess primeiro — a maioria das reconexoes nao tem pendencia; sem isto,
    # todo (re)connect dispararia um capture-pane atoa (pressao no threadpool em rajada de mobile).
    if not any(e.get("delivered") is False for e in q.load()):
        return 0
    start_ts = _transcript_start_ts(jsonl)   # poda entradas de sessao antiga (pre-/clear)
    ti = TerminalInput()
    sent = 0
    while True:
        claimed = q.claim_undelivered(min_ts=start_ts, limit=1)
        if not claimed:
            return sent
        entry = claimed[0]
        try:
            result = ti.send_prompt(name, entry["text"])
        except Exception:
            # Falha POS-gate (tty caiu no meio): pode ter emitido tecla -> at-most-once, NAO reverte.
            # ponytail: stranded-mas-visivel (a bubble queued- segue aparecendo, display ignora delivered);
            # upgrade: render distinto / re-drain confirmado-por-transcript se virar reclamacao real.
            return sent
        if result == "deferred":
            # send_prompt NAO tocou a TUI (overlay reabriu entre claim e envio): reverte com seguranca
            # (provadamente pre-envio) e para — espera o proximo idle.
            q.set_delivered(entry["id"], False)
            return sent
        sent += 1
```

And in `TerminalInput.send_prompt`, keep the control-char check first, then insert the gate before `_wait_input_ready`, and `return "sent"` at the end:

```python
    def send_prompt(self, name: str, text: str) -> str:
        # Validacao PRE-envio: input ruim nunca toca a TUI nem entra na fila. \n/\t ok; outros controles nao.
        if any(ord(c) < 32 and c not in "\t\n" for c in text):
            raise ValueError("control characters not allowed in prompt")
        # Gate de entregabilidade (chokepoint UNICO p/ texto livre — /input e drain passam por aqui):
        # nao digitar as cegas num overlay (AskUserQuestion/picker), as teclas o corromperiam. Sem pane
        # entregavel agora, devolve "deferred" SEM tocar a TUI; o caller enfileira pendente e o drain
        # entrega quando o overlay fechar / a sessao voltar.
        if not deliverable(name):
            return "deferred"
        _wait_input_ready(name)
        if "\n" in text:
            tmux.paste_text(name, text)
            time.sleep(0.05)
            send_keys(name, "Enter")
        elif text.lstrip().startswith("/"):
            send_keys(name, text, literal=True)
            time.sleep(_SLASH_SETTLE)
            send_keys(name, "Enter")
            time.sleep(_SLASH_SETTLE)
            send_keys(name, "Enter")
        else:
            send_keys(name, text, literal=True)
            send_keys(name, "Enter")
        return "sent"
```

4. **Run + expect pass** — `(cd backend && .venv/bin/python -m pytest tests/test_terminal_input.py -q)` → all pass (and now fast: the updated `send_prompt` test no longer burns the 12s `_wait_input_ready` timeout).

5. **Commit:** `feat(terminal-input): deliverability gate in send_prompt + idle drain`

### Task 4 — `/input` eager-send vs defer-on-overlay

**Files:** Modify `/home/jefferson/pessoal/claude-pocket/backend/app/api.py`, Modify `/home/jefferson/pessoal/claude-pocket/backend/tests/test_api.py`

1. **Failing test first** — in `tests/test_api.py`, replace `test_input_route_calls_terminal` and add the defer/control-char cases:

```python
def test_input_eager_send_marks_delivered(api_client):
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp, \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    sp.assert_called_once_with("cc", "oi")
    ap.assert_called_once_with("oi", delivered=True)


def test_input_defer_on_overlay_marks_pending(api_client):
    with patch("app.api.terminal.send_prompt", return_value="deferred"), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    ap.assert_called_once_with("oi", delivered=False)


def test_input_control_char_400_without_queue(api_client):
    with patch("app.api.terminal.send_prompt",
               side_effect=ValueError("control characters not allowed")), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "bad\u0007"}, headers=_h())
    assert r.status_code == 400
    ap.assert_not_called()   # validado no send_prompt ANTES de enfileirar
```

2. **Run + expect fail** — `(cd backend && .venv/bin/python -m pytest tests/test_api.py -q -k input)` → fails: `append` called with positional `"oi"` (no `delivered=`) and `assert_called_once_with("oi", delivered=True)` mismatches.

3. **Minimal implementation** — in `api.py` `input_prompt`, capture the return and pass `delivered=` (slash/`/clear` branch unchanged):

```python
@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
def input_prompt(name: str, body: InputBody):
    try:
        result = terminal.send_prompt(name, body.text)
    except ValueError as e:
        # send_prompt rejeita control chars (pre-envio). 400 limpo (o frontend mostra) em vez de 500.
        raise HTTPException(400, str(e))
    from app.pqueue import PromptQueue
    stripped = body.text.lstrip()
    if stripped.startswith("/"):
        if stripped[1:].split(maxsplit=1)[:1] == ["clear"]:
            try:
                PromptQueue(name).clear()
            except OSError:
                pass
    else:
        # delivered = o send_prompt REALMENTE digitou ("sent"); pane em overlay -> "deferred" (nao
        # tocou a TUI) e a entrada fica pendente pro drain entregar quando o overlay fechar.
        try:
            PromptQueue(name).append(body.text, delivered=(result == "sent"))
        except OSError:
            pass
    return {"ok": True}
```

4. **Run + expect pass** — `(cd backend && .venv/bin/python -m pytest tests/test_api.py -q)` → all pass.

5. **Commit:** `feat(api): /input eager-send vs defer-on-overlay`

### Task 5 — SSE drain trigger on overlay→idle transition

**Files:** Modify `/home/jefferson/pessoal/claude-pocket/backend/app/sse.py`, Modify `/home/jefferson/pessoal/claude-pocket/backend/tests/test_sse.py`

1. **Failing test first** — append to `tests/test_sse.py` (add `from app.models import StateEvent`):

```python
async def _seq_states():
    # overlay aberto (nao-entregavel) -> idle (entregavel): a transicao dispara o drain UMA vez.
    yield StateEvent(session="cc", state="awaiting_input", overlay=True)
    yield StateEvent(session="cc", state="idle", overlay=False)
    yield StateEvent(session="cc", state="idle", overlay=False)   # repetido NAO redispara


class _StubMonitorSeq:
    def __init__(self, name):
        pass

    def stream(self):
        return _seq_states()


@pytest.mark.asyncio
async def test_drain_fires_once_on_overlay_to_idle(monkeypatch):
    calls = []
    monkeypatch.setattr("app.sse.TranscriptTailer", _StubTailerOne)
    monkeypatch.setattr("app.sse.StateMonitor", _StubMonitorSeq)
    monkeypatch.setattr("app.sse.drain", lambda name, jsonl: calls.append((name, jsonl)) or 0)
    seen_idle = 0
    async for ev in merged_events("cc", "j"):
        if ev["event"] == "state" and json.loads(ev["data"])["state"] == "idle":
            seen_idle += 1
            if seen_idle >= 2:
                await asyncio.sleep(0.05)   # deixa o to_thread(drain) rodar
                break
    assert calls == [("cc", "j")]            # exatamente 1 drain, no jsonl corrente
```

2. **Run + expect fail** — `(cd backend && .venv/bin/python -m pytest tests/test_sse.py -q -k drain)` → `AttributeError: <module 'app.sse'> does not have the attribute 'drain'` (monkeypatch target missing).

3. **Minimal implementation** — in `sse.py` add the import, init the trigger state before the loop, and fire the drain inside the existing `event == "state"` block:

```python
from app.terminal_input import drain
```

Before the `while True:` loop (next to `ask_q_emitted = False`):

```python
    prev_deliverable = False     # init False -> 1o estado entregavel pos-(re)connect tambem dispara 1
                                 # drain (recovery de restart/reconexao com pendencia)
    drain_tasks: set = set()     # drains fire-and-forget; NAO entram em `tasks` (nao cancelar no disconnect)
```

Inside `if event == "state":`, after the existing ask_question handling and before the final `yield`:

```python
                # Drain gatilho: quando o pane volta a aceitar texto livre (overlay/menu fechou, ou a
                # sessao voltou ao idle), entrega as enfileiradas pendentes. Deriva a entregabilidade
                # dos campos do PROPRIO StateEvent — reusa o stream do StateMonitor, sem novo poll.
                deliverable_now = (
                    parsed_state.get("state") not in ("awaiting_input", "dead")
                    and not parsed_state.get("overlay")
                )
                if deliverable_now and not prev_deliverable:
                    # fire-and-forget no threadpool (drain bloqueia em send_prompt) — nunca await no
                    # loop SSE. FORA de `tasks`: deixar um drain em voo terminar apos o phone
                    # desconectar e correto (entrega duravel nao depende do phone ficar conectado).
                    dt = asyncio.create_task(asyncio.to_thread(drain, name, current_jsonl))
                    drain_tasks.add(dt)
                    dt.add_done_callback(drain_tasks.discard)
                prev_deliverable = deliverable_now
```

4. **Run + expect pass** — `(cd backend && .venv/bin/python -m pytest tests/test_sse.py -q)` → all pass (the existing error-propagation + json-string tests stay green; `drain` import doesn't change their stubs).

5. **Commit:** `feat(sse): drain pending queue on overlay->idle transition`

### Task 6 — Full backend verification

**Files:** none (verification only)

1. **Run the whole suite** — `(cd backend && .venv/bin/python -m pytest -q)`.
2. **Expect** — only the 2 KNOWN `test_state_classifier` failures remain; nothing else regressed (the previously-slow `test_terminal_input` now runs fast).
3. **Manual trace (documented, not automated):** send during a live multi-question AskUserQuestion → `/input` returns `deferred`, a `queued-` bubble shows, no tty corruption → answer the menu → on overlay→idle the SSE fires `drain` → single delivery, then text-dedup absorbs the bubble. Kill+restart backend with a `delivered=false` entry → delivered once on the next idle (prev_deliverable=False fires on connect). Restart with a `delivered=true` or a legacy (no-key) entry → never re-sent.
4. **No commit — verification only.**

**Ponytail notes:** root-cause gate lives in the single `send_prompt` chokepoint (no per-caller duplication, no `/drain` endpoint, no background scheduler, no sidecar file, one bool field); reused `_append_lock` + tmp/replace + the StateMonitor stream + `_transcript_start_ts`; deliberate ceilings marked with `ponytail:` — global lock, claim-before-send at-most-once (a crash strands ≤1 entry, bubble stays visible), per-connection trigger made benign by the cheap-check + atomic single-flight claim.

**Ecc notes:** no double-send (atomic claim-1-send-1, eager entries are `delivered=True` and never drained, revert only on provably pre-send `"deferred"`, post-keystroke failure is non-revertible); no silent loss (failure leaves the entry observable as a `queued-` bubble, never mark-and-forget); no new auth/unbounded surface (no endpoint, queue capped at 1000, `_transcript_start_ts` early-returns); dead session defers instead of burning the 12s ready-wait; zero-pending reconnects spawn no `capture_pane` subprocess.