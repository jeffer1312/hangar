# Session-list state via Claude hooks (sub-project A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the session list derive each session's state (`working`/`idle`/`awaiting_input`) from Claude Code hooks (pushed to per-session marker files, watched by the backend) instead of scraping the tmux pane on every list call.

**Architecture:** A tiny hook script (`state_hook.py`, mirroring `askq_capture.py`) writes `<config>/.claude-pocket-state/<session_id>.json` on each relevant hook event. A backend watcher (`hook_state.py`) keeps an in-memory `session_id → (state, ts)` map, seeded on startup and updated via `watchfiles`. `registry.list_with_state` prefers that map and falls back to `classify(pane)` only for sessions without a marker. The open-chat `StateMonitor`/`preview.py` are untouched.

**Tech Stack:** Python 3.14, FastAPI, `watchfiles`, pytest (real TDD — backend has a test runner).

## Global Constraints

- Test runner: `cd backend && uv run pytest -v`. This is the gate for every task — TDD (failing test first). Do not break the existing suite (221 passing).
- The hook script must **fail silently** (never block the prompt, never write to stdout — same as `askq_capture.py`).
- Marker file: `<config>/.claude-pocket-state/<session_id>.json` with body `{"state": "<state>", "ts": <epoch_float>}`. `<config>` = `os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")`.
- Event → state map: `UserPromptSubmit`/`PreToolUse`/`PostToolUse` → `working`; `Notification` → `awaiting_input`; `Stop` → `idle`. `dead` is NOT produced by hooks.
- The installer must be idempotent and must NOT clobber existing user hooks — including the pre-existing askq `PreToolUse` entry. Follow `hook_installer.py`'s existing bulletproofing (skip malformed `settings.json`, never overwrite).
- Out of scope: open-chat state (`StateMonitor`, `preview.py`), `dead` detection, control mode (B), list-SSE (C). This sub-project only changes the list's state *source*; the 5s frontend poll persists until C.
- Commit messages in English, no `Co-Authored-By` trailer.

---

## File Structure

- `backend/hooks/state_hook.py` — new hook script (event JSON on stdin → marker file).
- `backend/app/hook_state.py` — new module: watcher + `session_id → (state, ts)` map + `get_state`.
- `backend/app/hook_installer.py` — extend: register `state_hook.py` for the five events.
- `backend/app/registry.py` — `list_with_state` prefers the marker over the pane.
- `backend/app/main.py` (or wherever `ensure_askq_hook_installed()` is called) — wire install + start the watcher at startup.
- Tests: `backend/tests/test_state_hook.py`, `backend/tests/test_hook_state.py`, `backend/tests/test_hook_installer_state.py`, additions to `backend/tests/test_registry.py`.

---

## Task 1: `state_hook.py` — event → marker file

**Files:**
- Create: `backend/hooks/state_hook.py`
- Test: `backend/tests/test_state_hook.py`

**Interfaces:**
- Produces: a script invoked as `python3 state_hook.py`, reading a hook-event JSON on stdin with keys `hook_event_name` and `session_id`. Writes `<config>/.claude-pocket-state/<session_id>.json` = `{"state": ..., "ts": ...}`. Exit 0 always.

- [ ] **Step 1: Write the failing test**

Create `backend/tests/test_state_hook.py`. The hook is a script; test it by running it as a subprocess with a JSON stdin and a `CLAUDE_CONFIG_DIR` pointing at a tmp dir, then asserting the marker file.

```python
import json, os, subprocess, sys
from pathlib import Path

HOOK = str(Path(__file__).resolve().parent.parent / "hooks" / "state_hook.py")

def _run(payload: dict, config_dir: Path) -> None:
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(config_dir)}
    subprocess.run([sys.executable, HOOK], input=json.dumps(payload).encode(),
                   env=env, check=True, timeout=10)

def _marker(config_dir: Path, sid: str) -> dict:
    return json.loads((config_dir / ".claude-pocket-state" / f"{sid}.json").read_text())

def test_user_prompt_submit_is_working(tmp_path):
    _run({"hook_event_name": "UserPromptSubmit", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "working"

def test_stop_is_idle(tmp_path):
    _run({"hook_event_name": "Stop", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "idle"

def test_notification_is_awaiting(tmp_path):
    _run({"hook_event_name": "Notification", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "awaiting_input"

def test_pre_and_post_tool_use_are_working(tmp_path):
    for ev in ("PreToolUse", "PostToolUse"):
        _run({"hook_event_name": ev, "session_id": "s"}, tmp_path)
        assert _marker(tmp_path, "s")["state"] == "working"

def test_marker_has_float_ts(tmp_path):
    _run({"hook_event_name": "Stop", "session_id": "abc"}, tmp_path)
    assert isinstance(_marker(tmp_path, "abc")["ts"], float)

def test_unknown_event_writes_nothing(tmp_path):
    _run({"hook_event_name": "SomethingElse", "session_id": "abc"}, tmp_path)
    assert not (tmp_path / ".claude-pocket-state" / "abc.json").exists()

def test_missing_session_id_does_not_crash(tmp_path):
    _run({"hook_event_name": "Stop"}, tmp_path)  # exit 0, no marker
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd backend && uv run pytest tests/test_state_hook.py -v`
Expected: FAIL (script does not exist / FileNotFoundError on the marker).

- [ ] **Step 3: Implement `backend/hooks/state_hook.py`**

```python
#!/usr/bin/env python3
# ponytail: hook minimo — le o JSON do evento no stdin, mapeia hook_event_name -> estado e grava
# um marcador por session_id. SEM stdout. Falha em silencio (nunca trava o prompt). Espelha o
# padrao do askq_capture.py. Usado pelo backend (hook_state.py) pra saber o estado da LISTA sem
# raspar o pane.
import json
import os
import sys
import time

_STATE = {
    "UserPromptSubmit": "working",
    "PreToolUse": "working",
    "PostToolUse": "working",
    "Notification": "awaiting_input",
    "Stop": "idle",
}

try:
    o = json.loads(sys.stdin.read())
    state = _STATE.get(o.get("hook_event_name"))
    sid = o.get("session_id")
    if state and sid:
        base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
        d = os.path.join(base, ".claude-pocket-state")
        os.makedirs(d, exist_ok=True)
        tmp = os.path.join(d, sid + ".json.tmp")
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump({"state": state, "ts": time.time()}, fh)
        os.replace(tmp, os.path.join(d, sid + ".json"))  # escrita atomica (watcher nunca le parcial)
except Exception:
    pass
sys.exit(0)
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cd backend && uv run pytest tests/test_state_hook.py -v`
Expected: PASS (all 7).

- [ ] **Step 5: Commit**

```bash
git add backend/hooks/state_hook.py backend/tests/test_state_hook.py
git commit -m "feat(backend): state_hook.py — maps Claude hook events to per-session state markers"
```

---

## Task 2: `hook_state.py` — watcher + in-memory state map

**Files:**
- Create: `backend/app/hook_state.py`
- Test: `backend/tests/test_hook_state.py`

**Interfaces:**
- Consumes: marker files written by Task 1.
- Produces: module-level singleton `hook_state` (instance of `HookState`) with `get_state(session_id: str) -> tuple[str, float] | None`; `load_existing(dirs: list[Path]) -> None` (seed from markers); `_apply(path: Path) -> None` (read one marker into the map); `async def watch(dirs: list[Path]) -> None` (long-lived `watchfiles.awatch` loop). Marker filename stem = `session_id`.

- [ ] **Step 1: Write the failing test**

Create `backend/tests/test_hook_state.py`. Test the pure map logic (seed + apply a change) without the async watch loop.

```python
import json, time
from pathlib import Path
from app import hook_state

def _write(d: Path, sid: str, state: str):
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))

def test_load_existing_seeds_map(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    _write(sd, "bbb", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    assert hs.get_state("bbb")[0] == "idle"

def test_get_state_none_when_absent(tmp_path):
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("missing") is None

def test_apply_updates_existing(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    _write(sd, "aaa", "idle")            # state flips
    hs._apply(sd / "aaa.json")
    assert hs.get_state("aaa")[0] == "idle"

def test_apply_ignores_bad_json(tmp_path):
    sd = tmp_path / ".claude-pocket-state"; sd.mkdir(parents=True)
    (sd / "x.json").write_text("{ not json")
    hs = hook_state.HookState()
    hs._apply(sd / "x.json")             # no raise
    assert hs.get_state("x") is None
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd backend && uv run pytest tests/test_hook_state.py -v`
Expected: FAIL (`app.hook_state` does not exist).

- [ ] **Step 3: Implement `backend/app/hook_state.py`**

Mirror how `transcript.py` imports/uses `watchfiles` for the async loop (read it for the `awatch` call style). Implementation:

```python
import json
from pathlib import Path
from typing import Optional

from watchfiles import awatch

_SUBDIR = ".claude-pocket-state"


class HookState:
    """Estado da LISTA por sessao, vindo dos hooks do Claude (state_hook.py grava marcadores).
    Mapa em memoria session_id -> (state, ts). get_state() devolve None se nao ha marcador
    (o caller cai no fallback de raspar o pane)."""

    def __init__(self) -> None:
        self._map: dict[str, tuple[str, float]] = {}

    def get_state(self, session_id: Optional[str]) -> Optional[tuple[str, float]]:
        if not session_id:
            return None
        return self._map.get(session_id)

    def _apply(self, path: Path) -> None:
        # Le UM marcador pro mapa. Falha-soft: marcador parcial/corrompido e ignorado.
        try:
            o = json.loads(path.read_text(encoding="utf-8"))
            state, ts = o["state"], float(o["ts"])
        except Exception:
            return
        self._map[path.stem] = (state, ts)

    def load_existing(self, dirs: list[Path]) -> None:
        # Semeia o mapa com os marcadores ja presentes (no startup do backend).
        for base in dirs:
            sd = base / _SUBDIR
            if not sd.is_dir():
                continue
            for f in sd.glob("*.json"):
                self._apply(f)

    async def watch(self, dirs: list[Path]) -> None:
        # Loop longo: observa cada <config>/.claude-pocket-state e aplica cada mudanca.
        watched = []
        for base in dirs:
            sd = base / _SUBDIR
            sd.mkdir(parents=True, exist_ok=True)  # garante existir pro awatch nao falhar
            watched.append(str(sd))
        async for changes in awatch(*watched):
            for _change, p in changes:
                path = Path(p)
                if path.suffix == ".json":
                    self._apply(path)


# Singleton de modulo (igual ao padrao do registry/installer).
hook_state = HookState()
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cd backend && uv run pytest tests/test_hook_state.py -v`
Expected: PASS (all 4).

- [ ] **Step 5: Commit**

```bash
git add backend/app/hook_state.py backend/tests/test_hook_state.py
git commit -m "feat(backend): hook_state.py — watches per-session state markers into an in-memory map"
```

---

## Task 3: install the state hooks (idempotent)

**Files:**
- Modify: `backend/app/hook_installer.py`
- Test: `backend/tests/test_hook_installer_state.py`

**Interfaces:**
- Consumes: `state_hook.py` (Task 1).
- Produces: `_ensure_event_hook(settings_path: Path, event: str, command: str) -> bool` and `ensure_state_hooks_installed() -> list[str]` — registers `state_hook.py` for `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop` in each config dir's `settings.json`, idempotently, preserving existing hooks.

- [ ] **Step 1: Write the failing test**

Create `backend/tests/test_hook_installer_state.py`. Drive a single `settings.json` in a tmp dir through `_ensure_event_hook`.

```python
import json
from pathlib import Path
from app import hook_installer as hi

EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"]

def _install(p: Path):
    for ev in EVENTS:
        hi._ensure_event_hook(p, ev, "python3 /x/state_hook.py")

def test_installs_all_five_events(tmp_path):
    p = tmp_path / "settings.json"
    _install(p)
    data = json.loads(p.read_text())
    for ev in EVENTS:
        cmds = [h["command"] for b in data["hooks"][ev] for h in b["hooks"]]
        assert "python3 /x/state_hook.py" in cmds

def test_idempotent(tmp_path):
    p = tmp_path / "settings.json"
    _install(p); first = p.read_text()
    _install(p)                       # second run: no change
    assert p.read_text() == first

def test_preserves_existing_pretooluse(tmp_path):
    p = tmp_path / "settings.json"
    p.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": "python3 /x/askq_capture.py"}]}
    ]}}))
    _install(p)
    cmds = [h["command"] for b in json.loads(p.read_text())["hooks"]["PreToolUse"] for h in b["hooks"]]
    assert "python3 /x/askq_capture.py" in cmds          # askq kept
    assert "python3 /x/state_hook.py" in cmds            # state added alongside

def test_skips_malformed_settings(tmp_path):
    p = tmp_path / "settings.json"; p.write_text("{ not json")
    _install(p)
    assert p.read_text() == "{ not json"                 # never clobbered
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd backend && uv run pytest tests/test_hook_installer_state.py -v`
Expected: FAIL (`_ensure_event_hook` does not exist).

- [ ] **Step 3: Implement in `backend/app/hook_installer.py`**

Append after `ensure_askq_hook_installed`:

```python
STATE_HOOK = str((Path(__file__).parent.parent / "hooks" / "state_hook.py").resolve())
_STATE_COMMAND = f"python3 {STATE_HOOK}"
_STATE_EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"]


def _ensure_event_hook(settings_path: Path, event: str, command: str) -> bool:
    """Acrescenta {command} sob settings['hooks'][event], idempotente, preservando todo o resto.
    Mesma blindagem do _ensure_settings_file: settings.json quebrado/estranho e PULADO (False)."""
    data: dict = {}
    if settings_path.exists():
        raw = settings_path.read_text(encoding="utf-8").strip()
        if raw:
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                return False
        if not isinstance(data, dict):
            return False
    existing_hooks = data.get("hooks")
    if existing_hooks is not None and not isinstance(existing_hooks, dict):
        return False
    ev_list = (existing_hooks or {}).get(event)
    if ev_list is not None and not isinstance(ev_list, list):
        return False
    for block in ev_list or []:
        if not isinstance(block, dict):
            continue
        for h in block.get("hooks") or []:
            if isinstance(h, dict) and h.get("command") == command:
                return False  # ja instalado
    hooks = data.setdefault("hooks", {})
    hooks.setdefault(event, []).append({"hooks": [{"type": "command", "command": command}]})
    settings_path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return True


def ensure_state_hooks_installed() -> list[str]:
    """Instala (idempotente) o state_hook nos 5 eventos, em cada config dir. Fail-soft por arquivo;
    nunca derruba o startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            changed = False
            for ev in _STATE_EVENTS:
                if _ensure_event_hook(d / "settings.json", ev, _STATE_COMMAND):
                    changed = True
            if changed:
                touched.append(str(d))
        except Exception:
            continue
    return touched
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cd backend && uv run pytest tests/test_hook_installer_state.py -v`
Expected: PASS (all 4).

- [ ] **Step 5: Commit**

```bash
git add backend/app/hook_installer.py backend/tests/test_hook_installer_state.py
git commit -m "feat(backend): install state_hook for 5 Claude events (idempotent, preserves existing hooks)"
```

---

## Task 4: `list_with_state` prefers the marker; wire startup

**Files:**
- Modify: `backend/app/registry.py` (`list_with_state`, ~lines 283-307)
- Modify: `backend/app/main.py` (+ `backend/app/api.py` if the watcher start belongs in the FastAPI lifespan) — startup wiring next to `ensure_askq_hook_installed()`
- Test: additions to `backend/tests/test_registry.py`

**Interfaces:**
- Consumes: `hook_state.get_state` (Task 2), `ensure_state_hooks_installed` (Task 3).
- Produces: `list_with_state` returns the hook-marker state for sessions that have one, falling back to `classify(pane)` otherwise.

- [ ] **Step 1: Write the failing test**

Add to `backend/tests/test_registry.py`:

```python
import asyncio
from app.registry import SessionRegistry
from app import hook_state as hs_mod

def test_list_with_state_prefers_marker(monkeypatch):
    reg = SessionRegistry()
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": "/x/sid123.jsonl", "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("working", 1.0) if sid == "sid123" else None)
    called = {"pane": 0}
    def fake_capture(name):
        called["pane"] += 1; return ""
    monkeypatch.setattr("app.registry.tmux.capture_pane", fake_capture)
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "working"
    assert called["pane"] == 0          # marcador presente -> NAO raspa o pane

def test_list_with_state_falls_back_to_pane(monkeypatch):
    reg = SessionRegistry()
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": "/x/none.jsonl", "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: None)   # sem marcador
    monkeypatch.setattr("app.registry.tmux.capture_pane", lambda name: "")  # pane vazio -> idle
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "idle"
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd backend && uv run pytest tests/test_registry.py -k list_with_state -v`
Expected: FAIL (`list_with_state` still always captures the pane; `prefers_marker` asserts `called["pane"] == 0`).

- [ ] **Step 3: Implement the marker preference in `list_with_state`**

Add at the top of `registry.py`: `from app.hook_state import hook_state` (and confirm `from pathlib import Path` is present — it is). Replace the `list_with_state` body from `infos = await asyncio.to_thread(self.list)` onward with:

```python
        infos = await asyncio.to_thread(self.list)
        if not infos:
            return infos
        # Estado pela marca dos hooks quando existe (custo ~0); senao cai no pane (fallback).
        def _sid(jsonl):
            return Path(jsonl).stem if jsonl else None
        pending = []  # infos sem marcador -> precisa raspar o pane
        for info in infos:
            marker = hook_state.get_state(_sid(info.jsonl))
            if marker:
                info.state = marker[0]
                info.last_activity = _jsonl_mtime(info.jsonl)
            else:
                pending.append(info)
        if pending:
            frames = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, info.name) for info in pending])
            classified = [classify(t) for t in frames]
            spinners = [_live_spinner(t) for t in frames]
            spin_idx = [k for k, c in enumerate(classified) if c[0] == "working"]
            if spin_idx:
                await asyncio.sleep(0.15)
                f2 = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, pending[k].name) for k in spin_idx])
                for j, k in enumerate(spin_idx):
                    sp2 = _live_spinner(f2[j])
                    if sp2 is None or sp2 == spinners[k]:
                        classified[k] = ("idle", None, None, None)
            for info, c in zip(pending, classified):
                info.state = c[0]
                info.last_activity = _jsonl_mtime(info.jsonl)
        return infos
```

- [ ] **Step 4: Run the new tests + the full suite**

Run: `cd backend && uv run pytest tests/test_registry.py -k list_with_state -v`
Expected: PASS (both).
Run: `cd backend && uv run pytest -q`
Expected: whole suite passes (the existing `list_with_state` test must still pass — with no markers present it falls through to the pane path unchanged).

- [ ] **Step 5: Wire install + watcher at startup**

Read `backend/app/main.py` to find where `ensure_askq_hook_installed()` is called at startup. Next to it, call `ensure_state_hooks_installed()` (same fail-soft style), then seed the map:

```python
from app.hook_installer import ensure_state_hooks_installed
from app.hook_state import hook_state
from app.config import list_config_dirs, _backend_config_base

ensure_state_hooks_installed()
_state_dirs = list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
hook_state.load_existing(_state_dirs)
```

Start the long-lived watcher in the app's ASYNC startup (a running loop is required — not at import time). Read `api.py`/`main.py` for the existing startup style: if there is a FastAPI `lifespan` or `@app.on_event("startup")`, add `asyncio.create_task(hook_state.watch(_state_dirs))` there; if not, add a minimal `lifespan` to the FastAPI `app` in `api.py` that creates that task on startup. Match the project's existing pattern; do not invent a new framework idiom. Keep `_state_dirs` reachable from wherever the task is created.

- [ ] **Step 6: Verify the full suite + manual smoke**

Run: `cd backend && uv run pytest -q`
Expected: all pass.

Manual (optional, needs a live session): restart the backend (it reinstalls the hooks), send a prompt in a tracked session, and confirm `<config>/.claude-pocket-state/<sid>.json` flips `working`→`idle` and that `/api/sessions` reflects it; a session with no marker still shows a pane-classified state.

- [ ] **Step 7: Commit**

```bash
git add backend/app/registry.py backend/app/main.py backend/app/api.py backend/tests/test_registry.py
git commit -m "feat(backend): list_with_state prefers hook state markers, pane only as fallback; wire startup"
```

---

## Self-Review notes

- **Spec coverage:** §1 `state_hook.py` → Task 1. §2 installer → Task 3. §3 `hook_state.py` watcher/map → Task 2. §4 `list_with_state` prefers marker → Task 4 (Steps 1-4); startup wiring → Task 4 Step 5. Edge cases: no-marker fallback → Task 4 `falls_back_to_pane`; bad JSON → Task 2 `_apply_ignores_bad_json`; idempotent/preserve-askq → Task 3 tests. The `awaiting`-via-`Notification` backup-against-askq question is left to impl per the spec (the `Notification`→`awaiting_input` mapping is in Task 1).
- **Atomic write:** Task 1 writes `.tmp` then `os.replace` so the watcher never reads a partial file.
- **Type consistency:** `get_state(sid) -> (state, ts) | None`, `_apply(path)`, `load_existing(dirs)`, `watch(dirs)`, `ensure_state_hooks_installed()`, `_ensure_event_hook(path, event, command)` — names match across Tasks 2/3/4.
- **No open-chat changes, no new deps** (`watchfiles` already used). The 5s frontend poll persists (C's job).
