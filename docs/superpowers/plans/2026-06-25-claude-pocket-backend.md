# claude-pocket Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Python backend that exposes live Claude Code `tmux` sessions to a phone over HTTP: a chat feed parsed from the session JSONL, a live-state SSE stream, and input/select/interrupt endpoints that drive the real terminal.

**Architecture:** FastAPI app over four isolated components — `SessionRegistry` (tmux discovery/lifecycle), `TranscriptTailer` (JSONL → chat events), `StateMonitor` (`capture-pane` → live state), `TerminalInput` (`send-keys`). Content comes from the structured JSONL; live state comes from a narrow screen read; input goes to the live session. An SSE endpoint merges transcript + state per session. This plan stops at a curl-drivable API; the Svelte frontend and the Caddy/TLS/firewall deploy are separate plans.

**Tech Stack:** Python 3.14, `uv`, FastAPI, `uvicorn[standard]`, `sse-starlette`, `watchfiles`, `pydantic` v2, `pytest`, `pytest-asyncio`. `tmux` on the host.

## Global Constraints

- Python 3.14, dependency + venv managed by `uv` (run everything via `uv run ...`).
- Bind the server to the LAN IP `192.168.77.23` (`enp1s0`) via config `LAN_BIND_IP`; never a public interface. No VPN bind for v1.
- All HTTP routes require a Bearer token (`AUTH_TOKEN`); SSE auth via httpOnly cookie (same-origin). Single user.
- Claude transcripts live at `~/.claude/projects/<sanitized-cwd>/<session-uuid>.jsonl`, one JSON event per line, appended in real time.
- `tmux send-keys`: always send literal text (`-l --`) and the `Enter` key as **two separate** invocations. Sanitize input.
- Content (chat bubbles/tool cards) is derived ONLY from JSONL. `capture-pane` is used ONLY for live-state: `working` (label = Claude's live spinner text, e.g. "Elucidating…") / `idle` / `dead`. A `capture-pane` parse miss must never block the chat.
- NO permission-approval feature — the user runs bypass permissions and never approves tool calls (validated in Task 1 spike). BUT Claude's **interactive questions** (`ExitPlanMode`, `AskUserQuestion`, numbered selects) DO occur in bypass and ARE handled: state `awaiting_input` with parsed `options`; the user picks one and the backend selects it via `select(n)` = `(n-1)×Down` + `Enter` (validated in spike), or cancels with `Esc`. Free-text questions are plain assistant text → chat bubble, answered via the input composer.
- TDD: write the failing test first, watch it fail, implement minimally, watch it pass, commit. Conventional Commit messages.

## File Structure

```
backend/
  pyproject.toml          # uv project + deps
  app/
    __init__.py
    config.py             # Settings (bind IP, token, projects path, poll interval)
    models.py             # pydantic models: SessionInfo, ChatEvent, StateEvent
    tmux.py               # subprocess wrappers: list/new/kill/send_keys/capture_pane/has_session
    transcript.py         # parse_line / parse_transcript + TranscriptTailer
    state.py              # classify() + StateMonitor
    terminal_input.py     # TerminalInput (send_prompt/select/interrupt)
    registry.py           # SessionRegistry (list/create/kill/resolve_jsonl)
    auth.py               # token + cookie auth dependency
    sse.py                # per-session merged event generator
    api.py                # FastAPI app + routes
    main.py               # uvicorn entry (reads config, binds LAN IP)
  tests/
    fixtures/
      jsonl_samples.jsonl    # real-shape transcript lines
      pane_idle.txt          # capture-pane samples (from Task 1 spike)
      pane_thinking.txt
      pane_approval.txt
    test_jsonl_parser.py
    test_state_classifier.py
    test_tmux.py
    test_terminal_input.py
    test_registry.py
    test_api.py
  docs/spike-results.md     # Task 1 output
```

---

### Task 1: Spike — validate the tmux bridge + capture real fixtures  ✅ COMPLETE (commit dfdc765)

> **DONE.** Real findings are in `backend/docs/spike-results.md`: assumption A validated (`send-keys` submits); the spinner is `glyph + gerund + …` (NOT "esc to interrupt"); the user runs bypass → permission-approval was dropped; interactive-question selection validated (`(n-1)×Down` + `Enter`). The Step expectations below that mention `esc to interrupt` / "approval" are PRE-spike and are superseded by `spike-results.md`. Fixtures captured: `pane_idle.txt`, `pane_thinking.txt`, `pane_awaiting_input.txt`, `jsonl_samples.jsonl`.

This task validated the risky assumptions before any code was built on them, and captured the real `capture-pane` text the state classifier needs. It was exploratory (not TDD); its deliverable is a written result + fixture files.

**Files:**
- Create: `backend/docs/spike-results.md`
- Create: `backend/tests/fixtures/pane_idle.txt`, `pane_thinking.txt`, `pane_approval.txt`
- Create: `backend/tests/fixtures/jsonl_samples.jsonl`

- [ ] **Step 1: Install tmux**

Run: `paru -S --noconfirm tmux && tmux -V`
Expected: prints a version like `tmux 3.x`.

- [ ] **Step 2: Start a Claude session inside tmux**

```bash
cd ~/Projetos/claude-pocket
tmux new -d -s spike -x 200 -y 50 'claude'
sleep 5
tmux capture-pane -p -t spike > backend/tests/fixtures/pane_idle.txt
```
Open `pane_idle.txt`. Expected: the Claude Code input box / prompt is visible, no spinner. This is the `idle` fixture.

- [ ] **Step 3: Validate send-keys submits a prompt (assumption A)**

```bash
tmux send-keys -t spike -l -- 'diga apenas a palavra PONG e nada mais'
tmux send-keys -t spike Enter
sleep 2
tmux capture-pane -p -t spike > backend/tests/fixtures/pane_thinking.txt
```
Open `pane_thinking.txt`. Expected: Claude is now working — a spinner / working line (e.g. text containing `esc to interrupt`) is present. Note the EXACT spinner substring in `spike-results.md`. Wait and run `tmux capture-pane -p -t spike` again; confirm `PONG` appears, proving the prompt was submitted.

- [ ] **Step 4: Capture an approval box (assumption B)**

```bash
tmux send-keys -t spike -l -- 'rode o comando bash: echo hello'
tmux send-keys -t spike Enter
sleep 3
tmux capture-pane -p -t spike > backend/tests/fixtures/pane_approval.txt
```
Open `pane_approval.txt`. Expected: a permission prompt (text containing `Do you want to proceed?` and numbered options like `1. Yes` / `2. No`, or the "don't ask again" variant). Record the EXACT marker substrings in `spike-results.md`. Then approve manually: `tmux send-keys -t spike -l -- '1'; tmux send-keys -t spike Enter`.

- [ ] **Step 5: Capture real JSONL lines**

```bash
PROJ=$(ls -dt ~/.claude/projects/*Projetos-claude-pocket*/ | head -1)
JSONL=$(ls -t "$PROJ"*.jsonl | head -1)
tail -n 8 "$JSONL" > backend/tests/fixtures/jsonl_samples.jsonl
```
Open the file. Expected: lines with `"type":"user"`/`"type":"assistant"`, `message.content[]` of `text`/`tool_use`/`tool_result`. These are the parser fixtures.

- [ ] **Step 6: Write spike-results.md and kill the session**

In `backend/docs/spike-results.md` record: tmux version; the exact spinner marker (Step 3); the exact approval markers + how options are selected (number+Enter vs arrows) (Step 4); confirmation that send-keys submitted (Step 3). Then `tmux kill-session -t spike`.

- [ ] **Step 7: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/docs/spike-results.md backend/tests/fixtures/
git commit -m "chore: spike tmux bridge and capture state fixtures"
```

> **Gate:** if Step 3 shows the prompt was NOT submitted, or Step 4 shows no detectable approval markers, STOP and revisit the design before continuing — the rest of the plan assumes both work.

---

### Task 2: Backend scaffold, config, and models

**Files:**
- Create: `backend/pyproject.toml`, `backend/app/__init__.py`, `backend/app/config.py`, `backend/app/models.py`
- Test: `backend/tests/test_models_smoke.py` (temporary smoke test)

**Interfaces:**
- Produces: `app.config.Settings` (fields `lan_bind_ip:str`, `port:int`, `auth_token:str`, `projects_dir:Path`, `poll_interval:float`); `app.config.settings` singleton.
- Produces models: `SessionInfo`, `ChatEvent`, `StateEvent` (signatures below).

- [ ] **Step 1: Init the uv project and add deps**

```bash
cd ~/Projetos/claude-pocket/backend 2>/dev/null || (mkdir -p ~/Projetos/claude-pocket/backend && cd ~/Projetos/claude-pocket/backend)
cd ~/Projetos/claude-pocket/backend
uv init --no-workspace --name claude-pocket-backend --python 3.14 .
uv add fastapi "uvicorn[standard]" sse-starlette watchfiles pydantic pydantic-settings
uv add --dev pytest pytest-asyncio httpx
```
Expected: `pyproject.toml` created, deps resolved, `uv.lock` written.

- [ ] **Step 2: Write `app/config.py`**

```python
from pathlib import Path
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="CP_", env_file=".env")

    lan_bind_ip: str = "192.168.77.23"
    port: int = 8765
    auth_token: str = "change-me"
    projects_dir: Path = Path.home() / ".claude" / "projects"
    poll_interval: float = 0.75


settings = Settings()
```

- [ ] **Step 3: Write `app/models.py`**

```python
from typing import Literal, Optional
from pydantic import BaseModel

ChatKind = Literal["user_msg", "assistant_msg", "tool_use", "tool_result"]
State = Literal["working", "idle", "awaiting_input", "dead"]


class SessionInfo(BaseModel):
    name: str
    cwd: Optional[str] = None
    jsonl: Optional[str] = None
    state: State = "idle"
    last_activity: Optional[float] = None


class ChatEvent(BaseModel):
    kind: ChatKind
    id: str
    parent_id: Optional[str] = None
    text: Optional[str] = None
    tool_name: Optional[str] = None
    tool_input: Optional[dict] = None
    tool_use_id: Optional[str] = None
    result: Optional[str] = None
    is_error: Optional[bool] = None
    ts: Optional[float] = None


class StateEvent(BaseModel):
    session: str
    state: State
    label: Optional[str] = None         # working: live status text, e.g. "Elucidating…"
    question: Optional[str] = None       # awaiting_input: the question line
    options: Optional[list[str]] = None  # awaiting_input: selectable option labels
```

- [ ] **Step 4: Write the smoke test**

```python
# backend/tests/test_models_smoke.py
from app.config import settings
from app.models import SessionInfo, ChatEvent, StateEvent


def test_settings_defaults():
    assert settings.port == 8765
    assert settings.poll_interval > 0


def test_models_construct():
    assert SessionInfo(name="cc").state == "idle"
    assert ChatEvent(kind="user_msg", id="1", text="hi").text == "hi"
    assert StateEvent(session="cc", state="working", label="Elucidating…").label == "Elucidating…"
    assert StateEvent(session="cc", state="awaiting_input", options=["Yes", "No"]).options == ["Yes", "No"]
```

- [ ] **Step 5: Run tests**

Run: `cd ~/Projetos/claude-pocket/backend && uv run pytest tests/test_models_smoke.py -v`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/pyproject.toml backend/uv.lock backend/app/ backend/tests/test_models_smoke.py
git commit -m "feat: scaffold backend with config and models"
```

---

### Task 3: tmux command layer

Thin, mockable wrappers around the `tmux` CLI. Pure command-building + subprocess; everything else depends on this.

**Files:**
- Create: `backend/app/tmux.py`
- Test: `backend/tests/test_tmux.py`

**Interfaces:**
- Produces:
  - `list_sessions() -> list[dict]` — each `{"name": str, "cwd": str}`
  - `has_session(name: str) -> bool`
  - `new_session(name: str, cwd: str, command: str) -> None`
  - `kill_session(name: str) -> None`
  - `send_keys(name: str, keys: str, literal: bool = False) -> None`
  - `capture_pane(name: str, lines: int = 200) -> str`
  - module-level `RUN` = `subprocess.run` (patchable in tests)

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_tmux.py
from unittest.mock import MagicMock, patch
import subprocess
from app import tmux


def test_list_sessions_parses_output():
    fake = MagicMock(stdout="cc\t/home/u/p\nweb\t/home/u/w\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake) as run:
        out = tmux.list_sessions()
    assert out == [
        {"name": "cc", "cwd": "/home/u/p"},
        {"name": "web", "cwd": "/home/u/w"},
    ]
    args = run.call_args[0][0]
    assert args[:2] == ["tmux", "list-sessions"]


def test_list_sessions_empty_when_no_server():
    fake = MagicMock(stdout="", returncode=1, stderr="no server running")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_sessions() == []


def test_send_keys_literal_uses_dashdash():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "echo hi", literal=True)
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "cc", "-l", "--", "echo hi"]


def test_send_keys_named_key():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Enter")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "cc", "Enter"]


def test_capture_pane_returns_stdout():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="screen", returncode=0)) as run:
        assert tmux.capture_pane("cc") == "screen"
    assert run.call_args[0][0][:3] == ["tmux", "capture-pane"]
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_tmux.py -v`
Expected: FAIL with `ModuleNotFoundError` / `AttributeError: module 'app.tmux'`.

- [ ] **Step 3: Implement `app/tmux.py`**

```python
import subprocess

RUN = subprocess.run


def _run(args: list[str]) -> subprocess.CompletedProcess:
    return RUN(args, capture_output=True, text=True)


def list_sessions() -> list[dict]:
    cp = _run(["tmux", "list-sessions", "-F", "#{session_name}\t#{pane_current_path}"])
    if cp.returncode != 0:
        return []
    out = []
    for line in cp.stdout.splitlines():
        if not line.strip():
            continue
        name, _, cwd = line.partition("\t")
        out.append({"name": name, "cwd": cwd})
    return out


def has_session(name: str) -> bool:
    return _run(["tmux", "has-session", "-t", name]).returncode == 0


def new_session(name: str, cwd: str, command: str) -> None:
    _run(["tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50", command])


def kill_session(name: str) -> None:
    _run(["tmux", "kill-session", "-t", name])


def send_keys(name: str, keys: str, literal: bool = False) -> None:
    args = ["tmux", "send-keys", "-t", name]
    if literal:
        args += ["-l", "--", keys]
    else:
        args += [keys]
    _run(args)


def capture_pane(name: str, lines: int = 200) -> str:
    cp = _run(["tmux", "capture-pane", "-p", "-t", name, "-S", f"-{lines}"])
    return cp.stdout
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_tmux.py -v`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/tmux.py backend/tests/test_tmux.py
git commit -m "feat: add tmux command layer"
```

---

### Task 4: JSONL transcript parser (pure)

**Files:**
- Create: `backend/app/transcript.py` (parser functions only this task)
- Test: `backend/tests/test_jsonl_parser.py`

**Interfaces:**
- Produces:
  - `parse_line(line: str) -> Optional[ChatEvent]` — one JSONL line → event (or `None` to skip)
  - `parse_transcript(path: str | Path) -> list[ChatEvent]`

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_jsonl_parser.py
import json
from app.transcript import parse_line
from app.models import ChatEvent


def _line(obj) -> str:
    return json.dumps(obj)


def test_user_text_message():
    ev = parse_line(_line({
        "type": "user", "uuid": "u1", "parentUuid": None,
        "message": {"role": "user", "content": "corrige o bug"},
    }))
    assert ev == ChatEvent(kind="user_msg", id="u1", parent_id=None, text="corrige o bug")


def test_assistant_text_message():
    ev = parse_line(_line({
        "type": "assistant", "uuid": "a1", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [{"type": "text", "text": "vou olhar"}]},
    }))
    assert ev.kind == "assistant_msg"
    assert ev.text == "vou olhar"
    assert ev.parent_id == "u1"


def test_assistant_tool_use():
    ev = parse_line(_line({
        "type": "assistant", "uuid": "a2", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_9", "name": "Bash", "input": {"command": "ls"}},
        ]},
    }))
    assert ev.kind == "tool_use"
    assert ev.tool_name == "Bash"
    assert ev.tool_use_id == "toolu_9"
    assert ev.tool_input == {"command": "ls"}


def test_user_tool_result_is_not_a_bubble():
    ev = parse_line(_line({
        "type": "user", "uuid": "u2", "parentUuid": "a2",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_9", "content": "file.txt", "is_error": False},
        ]},
    }))
    assert ev.kind == "tool_result"
    assert ev.tool_use_id == "toolu_9"
    assert ev.result == "file.txt"
    assert ev.is_error is False


def test_attachment_returns_none():
    assert parse_line(_line({"type": "attachment", "uuid": "x"})) is None


def test_blank_or_bad_line_returns_none():
    assert parse_line("") is None
    assert parse_line("{not json") is None


def test_real_fixture_lines_parse_without_error():
    from pathlib import Path
    p = Path(__file__).parent / "fixtures" / "jsonl_samples.jsonl"
    for line in p.read_text().splitlines():
        parse_line(line)  # must not raise
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_jsonl_parser.py -v`
Expected: FAIL (`ModuleNotFoundError: app.transcript`).

- [ ] **Step 3: Implement the parser in `app/transcript.py`**

```python
import json
from pathlib import Path
from typing import Optional
from app.models import ChatEvent


def _first(content: list, type_name: str) -> Optional[dict]:
    for item in content:
        if isinstance(item, dict) and item.get("type") == type_name:
            return item
    return None


def parse_line(line: str) -> Optional[ChatEvent]:
    line = line.strip()
    if not line:
        return None
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return None

    etype = obj.get("type")
    uid = obj.get("uuid", "")
    parent = obj.get("parentUuid")
    msg = obj.get("message")
    if not isinstance(msg, dict):
        return None
    content = msg.get("content")

    if etype == "user":
        if isinstance(content, str):
            return ChatEvent(kind="user_msg", id=uid, parent_id=parent, text=content)
        if isinstance(content, list):
            tr = _first(content, "tool_result")
            if tr is not None:
                res = tr.get("content")
                if isinstance(res, list):
                    res = " ".join(str(b.get("text", "")) for b in res if isinstance(b, dict))
                return ChatEvent(
                    kind="tool_result", id=uid, parent_id=parent,
                    tool_use_id=tr.get("tool_use_id"),
                    result=res if res is None else str(res),
                    is_error=bool(tr.get("is_error", False)),
                )
            txt = _first(content, "text")
            if txt is not None:
                return ChatEvent(kind="user_msg", id=uid, parent_id=parent, text=txt.get("text", ""))
        return None

    if etype == "assistant" and isinstance(content, list):
        tu = _first(content, "tool_use")
        if tu is not None:
            return ChatEvent(
                kind="tool_use", id=uid, parent_id=parent,
                tool_name=tu.get("name"), tool_use_id=tu.get("id"),
                tool_input=tu.get("input") or {},
            )
        txt = _first(content, "text")
        if txt is not None:
            return ChatEvent(kind="assistant_msg", id=uid, parent_id=parent, text=txt.get("text", ""))
    return None


def parse_transcript(path: str | Path) -> list[ChatEvent]:
    events = []
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        ev = parse_line(line)
        if ev is not None:
            events.append(ev)
    return events
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_jsonl_parser.py -v`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/transcript.py backend/tests/test_jsonl_parser.py
git commit -m "feat: parse Claude transcript JSONL into chat events"
```

---

### Task 5: TranscriptTailer (async follow)

**Files:**
- Modify: `backend/app/transcript.py` (append the `TranscriptTailer` class)
- Test: `backend/tests/test_jsonl_parser.py` (append async test)

**Interfaces:**
- Consumes: `parse_line` (Task 4).
- Produces: `TranscriptTailer(path: str | Path)` with:
  - `history() -> list[ChatEvent]`
  - `async follow() -> AsyncIterator[ChatEvent]` — yields existing lines then new appended ones

- [ ] **Step 1: Write the failing async test**

```python
# append to backend/tests/test_jsonl_parser.py
import asyncio, json, pytest
from app.transcript import TranscriptTailer


@pytest.mark.asyncio
async def test_tailer_yields_existing_then_new(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(json.dumps({"type": "user", "uuid": "u1",
                             "message": {"role": "user", "content": "hi"}}) + "\n")
    tailer = TranscriptTailer(f)
    got = []

    async def consume():
        async for ev in tailer.follow():
            got.append(ev)
            if len(got) == 2:
                return

    async def append():
        await asyncio.sleep(0.2)
        with f.open("a") as fh:
            fh.write(json.dumps({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
                                 "message": {"role": "assistant",
                                             "content": [{"type": "text", "text": "yo"}]}}) + "\n")

    await asyncio.wait_for(asyncio.gather(consume(), append()), timeout=5)
    assert [e.id for e in got] == ["u1", "a1"]
```

Add to `pyproject.toml` under `[tool.pytest.ini_options]`: `asyncio_mode = "auto"`.

- [ ] **Step 2: Run test to verify it fails**

Run: `uv run pytest tests/test_jsonl_parser.py::test_tailer_yields_existing_then_new -v`
Expected: FAIL (`ImportError: cannot import name 'TranscriptTailer'`).

- [ ] **Step 3: Implement `TranscriptTailer` in `app/transcript.py`**

```python
import asyncio
from typing import AsyncIterator
from watchfiles import awatch


class TranscriptTailer:
    def __init__(self, path: str | Path):
        self.path = Path(path)

    def history(self) -> list[ChatEvent]:
        return parse_transcript(self.path)

    async def follow(self) -> AsyncIterator[ChatEvent]:
        pos = 0
        # emit existing content first
        if self.path.exists():
            with self.path.open(encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    ev = parse_line(line)
                    if ev is not None:
                        yield ev
                pos = fh.tell()
        # then watch for appends
        async for _ in awatch(self.path.parent):
            if not self.path.exists():
                continue
            with self.path.open(encoding="utf-8", errors="replace") as fh:
                fh.seek(pos)
                for line in fh:
                    ev = parse_line(line)
                    if ev is not None:
                        yield ev
                pos = fh.tell()
```

- [ ] **Step 4: Run test to verify it passes**

Run: `uv run pytest tests/test_jsonl_parser.py -v`
Expected: all passed (8 total).

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/transcript.py backend/tests/test_jsonl_parser.py backend/pyproject.toml
git commit -m "feat: add async TranscriptTailer"
```

---

### Task 6: State classifier (pure)

Classifies a `capture-pane` snapshot into a live state. Markers are the REAL ones captured in the Task 1 spike: spinner = a spinner glyph + gerund + `…` (e.g. `✽ Elucidating…`); interactive widget = a `❯ N.` cursor line + a `?` question line.

**Files:**
- Create: `backend/app/state.py` (the `classify` function this task)
- Test: `backend/tests/test_state_classifier.py`

**Interfaces:**
- Produces: `classify(pane_text: str) -> tuple[str, Optional[str], Optional[str], Optional[list[str]]]` returning `(state, label, question, options)`. `working` → `label` (live spinner text); `awaiting_input` → `question` + `options`; otherwise `idle`. `dead` is decided by the caller (Task 7).
- Module constant: `SPINNER_GLYPHS: str` — real spinner glyph set from the spike.

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_state_classifier.py
from pathlib import Path
from app.state import classify


def test_working_with_spinner_label():
    state, label, q, opts = classify("● PONG\n\n✽ Elucidating…\n\n❯ \n  ← for agents\n")
    assert state == "working"
    assert label == "Elucidating…"


def test_working_elapsed_form():
    state, label, q, opts = classify("✻ Crunched for 8s\n❯ \n")
    assert state == "working" and label == "Crunched for 8s"


def test_assistant_bullet_is_not_spinner():
    # ● is the message bullet, not a spinner glyph
    state, label, q, opts = classify("● PONG\n❯ \n")
    assert state == "idle"


def test_awaiting_input_parses_question_and_options():
    pane = (
        "   Claude has written up a plan. Would you like to proceed?\n"
        "\n"
        "   ❯ 1. Yes, and bypass permissions\n"
        "     2. Yes, manually approve edits\n"
        "     3. No, keep planning\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert question == "Claude has written up a plan. Would you like to proceed?"
    assert options == ["Yes, and bypass permissions", "Yes, manually approve edits", "No, keep planning"]


def test_numbered_list_without_cursor_stays_idle():
    # a plain numbered list (no ❯ cursor on an option) is NOT a widget
    state, *_ = classify("Steps:\n  1. do this\n  2. do that\n❯ \n")
    assert state == "idle"


def test_idle_when_no_spinner_or_widget():
    state, label, q, opts = classify("❯ \n  ← for agents\n")
    assert state == "idle"


def test_real_fixtures():
    fx = Path(__file__).parent / "fixtures"
    assert classify((fx / "pane_idle.txt").read_text())[0] == "idle"
    s, lbl, *_ = classify((fx / "pane_thinking.txt").read_text())
    assert s == "working" and lbl
    s2, _, q2, opts2 = classify((fx / "pane_awaiting_input.txt").read_text())
    assert s2 == "awaiting_input" and opts2 and "proceed?" in (q2 or "")
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_state_classifier.py -v`
Expected: FAIL (`ModuleNotFoundError: app.state`).

- [ ] **Step 3: Implement `classify` in `app/state.py`**

```python
import re
from typing import Optional

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*[❯ ]?\s*\d+\.\s+(.*\S)\s*$")
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)


def _question(pane_text: str) -> Optional[str]:
    found = None
    for line in pane_text.splitlines():
        s = line.strip()
        if s.endswith("?"):
            found = s
    return found


def classify(pane_text: str):
    """Return (state, label, question, options).

    'working' -> label is the live spinner text; 'awaiting_input' -> question +
    options; otherwise 'idle'. 'dead' is decided by the caller (StateMonitor).
    """
    for line in pane_text.splitlines():
        s = line.strip()
        if len(s) >= 2 and s[0] in SPINNER_GLYPHS and s[1] == " ":
            return ("working", s[1:].strip(), None, None)

    if _CURSOR_RE.search(pane_text):
        options = [m.group(1).strip()
                   for m in (_OPTION_RE.match(ln) for ln in pane_text.splitlines()) if m]
        if options:
            return ("awaiting_input", None, _question(pane_text), options)

    return ("idle", None, None, None)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_state_classifier.py -v`
Expected: all 7 passed.

- [ ] **Step 5: Confirm against real fixtures**

The markers are already the real ones from the spike (`backend/docs/spike-results.md`). `test_real_fixtures` reads `pane_idle.txt`, `pane_thinking.txt`, and `pane_awaiting_input.txt` and must pass. If it fails, fix `SPINNER_GLYPHS` / the regexes to match the fixtures — do NOT edit the fixtures.

- [ ] **Step 6: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/state.py backend/tests/test_state_classifier.py
git commit -m "feat: classify live session state from capture-pane"
```

---

### Task 7: StateMonitor (async poll)

**Files:**
- Modify: `backend/app/state.py` (append `StateMonitor`)
- Test: `backend/tests/test_state_classifier.py` (append async test)

**Interfaces:**
- Consumes: `classify` (Task 6), `tmux.capture_pane`/`tmux.has_session` (Task 3), `StateEvent` (Task 2).
- Produces: `StateMonitor(name: str, poll: float = 0.75)` with `async stream() -> AsyncIterator[StateEvent]` that yields ONLY when the `(state, label, question, options)` tuple changes, and yields `dead` once when the session is gone.

- [ ] **Step 1: Write the failing async test**

```python
# append to backend/tests/test_state_classifier.py
import pytest
from unittest.mock import patch
from app import state as state_mod
from app.state import StateMonitor


@pytest.mark.asyncio
async def test_monitor_emits_only_on_change():
    panes = iter([
        "❯ \n",                  # idle
        "✽ Elucidating…\n",      # working
        "✽ Elucidating…\n",      # still working, same label (no emit)
        "❯ \n",                  # idle again
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if len(seen) == 3:
                break
    assert seen == [("idle", None), ("working", "Elucidating…"), ("idle", None)]
```

- [ ] **Step 2: Run test to verify it fails**

Run: `uv run pytest tests/test_state_classifier.py::test_monitor_emits_only_on_change -v`
Expected: FAIL (`ImportError: cannot import name 'StateMonitor'`).

- [ ] **Step 3: Implement `StateMonitor` in `app/state.py`**

```python
import asyncio
from typing import AsyncIterator
from app import tmux
from app.models import StateEvent


class StateMonitor:
    def __init__(self, name: str, poll: float = 0.75):
        self.name = name
        self.poll = poll

    async def stream(self) -> AsyncIterator[StateEvent]:
        last_key = object()
        while True:
            if not tmux.has_session(self.name):
                yield StateEvent(session=self.name, state="dead")
                return
            state, label, question, options = classify(tmux.capture_pane(self.name))
            key = (state, label, question, tuple(options or ()))
            if key != last_key:
                last_key = key
                yield StateEvent(session=self.name, state=state, label=label,
                                 question=question, options=options)
            await asyncio.sleep(self.poll)
```

Note: `classify` lives in the same module (Task 6), so `StateMonitor.stream` calls it directly. The test patches both `tmux.has_session` (True) and `tmux.capture_pane`.

- [ ] **Step 4: Run test to verify it passes**

Run: `uv run pytest tests/test_state_classifier.py -v`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/state.py backend/tests/test_state_classifier.py
git commit -m "feat: add StateMonitor poll loop"
```

---

### Task 8: TerminalInput

**Files:**
- Create: `backend/app/terminal_input.py`
- Test: `backend/tests/test_terminal_input.py`

**Interfaces:**
- Consumes: `tmux.send_keys` (Task 3).
- Produces: `TerminalInput` with `send_prompt(name, text)`, `select(name, option)` (1-based; navigates `(option-1)×Down` then `Enter` — validated in the spike), `interrupt(name)` (sends `Escape`; also cancels an interactive widget).

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_terminal_input.py
from unittest.mock import patch, call
from app import terminal_input
from app.terminal_input import TerminalInput


def test_send_prompt_literal_then_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().send_prompt("cc", "corrige o bug")
    assert sk.call_args_list == [
        call("cc", "corrige o bug", literal=True),
        call("cc", "Enter"),
    ]


def test_send_prompt_rejects_control_chars():
    import pytest
    with pytest.raises(ValueError):
        TerminalInput().send_prompt("cc", "bad\x00null")


def test_select_option_three_navigates_then_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 3)
    assert sk.call_args_list == [call("cc", "Down"), call("cc", "Down"), call("cc", "Enter")]


def test_select_option_one_just_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 1)
    assert sk.call_args_list == [call("cc", "Enter")]


def test_interrupt_sends_escape():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().interrupt("cc")
    assert sk.call_args_list == [call("cc", "Escape")]
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_terminal_input.py -v`
Expected: FAIL (`ModuleNotFoundError: app.terminal_input`).

- [ ] **Step 3: Implement `app/terminal_input.py`**

```python
from app.tmux import send_keys


class TerminalInput:
    def send_prompt(self, name: str, text: str) -> None:
        if any(ord(c) < 32 and c != "\t" for c in text):
            raise ValueError("control characters not allowed in prompt")
        send_keys(name, text, literal=True)
        send_keys(name, "Enter")

    def select(self, name: str, option: int) -> None:
        if option < 1:
            raise ValueError("option must be >= 1")
        for _ in range(option - 1):
            send_keys(name, "Down")
        send_keys(name, "Enter")

    def interrupt(self, name: str) -> None:
        send_keys(name, "Escape")
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_terminal_input.py -v`
Expected: all 5 passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/terminal_input.py backend/tests/test_terminal_input.py
git commit -m "feat: add TerminalInput driving the live tmux session"
```

---

### Task 9: SessionRegistry

**Files:**
- Create: `backend/app/registry.py`
- Test: `backend/tests/test_registry.py`

**Interfaces:**
- Consumes: `tmux` (Task 3), `settings.projects_dir` (Task 2), `SessionInfo` (Task 2).
- Produces: `SessionRegistry` with `list() -> list[SessionInfo]`, `create(name, cwd) -> SessionInfo`, `kill(name)`, `resolve_jsonl(cwd) -> Optional[str]`.
- `resolve_jsonl` maps a cwd to its project dir (`~/.claude/projects/<sanitized>`) and returns the most-recently-modified `*.jsonl`.

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_registry.py
from unittest.mock import patch
from app import registry
from app.registry import SessionRegistry, sanitize_cwd


def test_sanitize_cwd_matches_claude_scheme():
    assert sanitize_cwd("/home/jeffer1312/Projetos/claude-pocket") == \
        "-home-jeffer1312-Projetos-claude-pocket"


def test_resolve_jsonl_picks_newest(tmp_path):
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    old = proj / "old.jsonl"; old.write_text("{}"); 
    new = proj / "new.jsonl"; new.write_text("{}")
    import os, time
    os.utime(old, (time.time() - 100, time.time() - 100))
    reg = SessionRegistry(projects_dir=tmp_path)
    assert reg.resolve_jsonl("/home/u/p").endswith("new.jsonl")


def test_list_maps_sessions_to_jsonl(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_sessions",
                      return_value=[{"name": "cc", "cwd": "/home/u/p"}]), \
         patch.object(reg, "resolve_jsonl", return_value="/x/s.jsonl"):
        out = reg.list()
    assert out[0].name == "cc" and out[0].jsonl == "/x/s.jsonl"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_registry.py -v`
Expected: FAIL (`ModuleNotFoundError: app.registry`).

- [ ] **Step 3: Implement `app/registry.py`**

```python
import re
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


class SessionRegistry:
    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)

    def resolve_jsonl(self, cwd: str) -> Optional[str]:
        proj = self.projects_dir / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None
        files = sorted(proj.glob("*.jsonl"), key=lambda f: f.stat().st_mtime, reverse=True)
        return str(files[0]) if files else None

    def list(self) -> list[SessionInfo]:
        out = []
        for s in tmux.list_sessions():
            out.append(SessionInfo(name=s["name"], cwd=s["cwd"], jsonl=self.resolve_jsonl(s["cwd"])))
        return out

    def create(self, name: str, cwd: str) -> SessionInfo:
        sid = str(uuid.uuid4())
        tmux.new_session(name, cwd, f"claude --session-id {sid}")
        return SessionInfo(name=name, cwd=cwd, jsonl=str(self.projects_dir / sanitize_cwd(cwd) / f"{sid}.jsonl"))

    def kill(self, name: str) -> None:
        tmux.kill_session(name)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_registry.py -v`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/registry.py backend/tests/test_registry.py
git commit -m "feat: add SessionRegistry mapping tmux sessions to transcripts"
```

---

### Task 10: Auth dependency

**Files:**
- Create: `backend/app/auth.py`
- Test: `backend/tests/test_api.py` (auth cases; file continues in Task 11)

**Interfaces:**
- Consumes: `settings.auth_token` (Task 2).
- Produces: `require_auth(request) -> None` FastAPI dependency accepting `Authorization: Bearer <token>` OR cookie `cp_token=<token>`; raises `HTTPException(401)` otherwise. `login_response(resp)` sets the httpOnly cookie.

- [ ] **Step 1: Write the failing tests**

```python
# backend/tests/test_api.py
import pytest
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings


@pytest.fixture
def client():
    settings.auth_token = "secret"
    app = FastAPI()

    @app.get("/ping", dependencies=[Depends(require_auth)])
    def ping():
        return {"ok": True}

    return TestClient(app)


def test_rejects_without_token(client):
    assert client.get("/ping").status_code == 401


def test_accepts_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200


def test_accepts_cookie(client):
    r = client.get("/ping", cookies={"cp_token": "secret"})
    assert r.status_code == 200
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_api.py -v`
Expected: FAIL (`ModuleNotFoundError: app.auth`).

- [ ] **Step 3: Implement `app/auth.py`**

```python
from fastapi import Request, HTTPException
from app.config import settings


def require_auth(request: Request) -> None:
    auth = request.headers.get("Authorization", "")
    token = auth[7:] if auth.startswith("Bearer ") else request.cookies.get("cp_token")
    if token != settings.auth_token:
        raise HTTPException(status_code=401, detail="unauthorized")
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_api.py -v`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/auth.py backend/tests/test_api.py
git commit -m "feat: add bearer/cookie auth dependency"
```

---

### Task 11: API + SSE + entry point

**Files:**
- Create: `backend/app/sse.py`, `backend/app/api.py`, `backend/app/main.py`
- Test: `backend/tests/test_api.py` (append route tests)

**Interfaces:**
- Consumes: `SessionRegistry` (9), `TranscriptTailer` (5), `StateMonitor` (7), `TerminalInput` (8), `require_auth` (10).
- Produces FastAPI `app` with routes:
  - `GET /api/sessions` → `list[SessionInfo]`
  - `POST /api/sessions` `{name, cwd}` → `SessionInfo`
  - `DELETE /api/sessions/{name}` → `{ok: true}`
  - `GET /api/sessions/{name}/history` → `list[ChatEvent]`
  - `GET /api/sessions/{name}/events` → SSE (`message`/`state`)
  - `POST /api/sessions/{name}/input` `{text}` → `{ok: true}`
  - `POST /api/sessions/{name}/select` `{option}` → `{ok: true}`
  - `POST /api/sessions/{name}/interrupt` → `{ok: true}`

- [ ] **Step 1: Write the failing route tests**

```python
# append to backend/tests/test_api.py
from unittest.mock import patch
from app.models import SessionInfo


@pytest.fixture
def api_client():
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}


def test_list_sessions_route(api_client):
    with patch("app.api.registry.list", return_value=[SessionInfo(name="cc", cwd="/p")]):
        r = api_client.get("/api/sessions", headers=_h())
    assert r.status_code == 200
    assert r.json()[0]["name"] == "cc"


def test_input_route_calls_terminal(api_client):
    with patch("app.api.terminal.send_prompt") as sp:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    sp.assert_called_once_with("cc", "oi")


def test_select_route(api_client):
    with patch("app.api.terminal.select") as sel:
        r = api_client.post("/api/sessions/cc/select", json={"option": 2}, headers=_h())
    assert r.status_code == 200
    sel.assert_called_once_with("cc", 2)


def test_routes_require_auth(api_client):
    assert api_client.get("/api/sessions").status_code == 401
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_api.py -v`
Expected: FAIL (`ModuleNotFoundError: app.api`).

- [ ] **Step 3: Implement `app/sse.py`**

```python
import asyncio
from app.transcript import TranscriptTailer
from app.state import StateMonitor


async def merged_events(name: str, jsonl: str):
    tailer = TranscriptTailer(jsonl)
    monitor = StateMonitor(name)
    queue: asyncio.Queue = asyncio.Queue()

    async def pump_messages():
        async for ev in tailer.follow():
            await queue.put(("message", ev.model_dump()))

    async def pump_state():
        async for st in monitor.stream():
            await queue.put(("state", st.model_dump()))

    tasks = [asyncio.create_task(pump_messages()), asyncio.create_task(pump_state())]
    try:
        while True:
            event, data = await queue.get()
            yield {"event": event, "data": data}
    finally:
        for t in tasks:
            t.cancel()
```

- [ ] **Step 4: Implement `app/api.py`**

```python
from fastapi import FastAPI, Depends, HTTPException
from pydantic import BaseModel
from sse_starlette.sse import EventSourceResponse
from app.auth import require_auth
from app.registry import SessionRegistry
from app.terminal_input import TerminalInput
from app.sse import merged_events

app = FastAPI(title="claude-pocket")
registry = SessionRegistry()
terminal = TerminalInput()


class CreateBody(BaseModel):
    name: str
    cwd: str


class InputBody(BaseModel):
    text: str


class SelectBody(BaseModel):
    option: int


@app.get("/api/sessions", dependencies=[Depends(require_auth)])
def list_sessions():
    return registry.list()


@app.post("/api/sessions", dependencies=[Depends(require_auth)])
def create_session(body: CreateBody):
    return registry.create(body.name, body.cwd)


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth)])
def kill_session(name: str):
    registry.kill(name)
    return {"ok": True}


@app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)])
def history(name: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import parse_transcript
    return parse_transcript(jsonl)


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    return EventSourceResponse(merged_events(name, jsonl))


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
def input_prompt(name: str, body: InputBody):
    terminal.send_prompt(name, body.text)
    return {"ok": True}


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    terminal.select(name, body.option)
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
def interrupt(name: str):
    terminal.interrupt(name)
    return {"ok": True}
```

- [ ] **Step 5: Implement `app/main.py`**

```python
import uvicorn
from app.config import settings


def main():
    uvicorn.run("app.api:app", host=settings.lan_bind_ip, port=settings.port)


if __name__ == "__main__":
    main()
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `uv run pytest tests/test_api.py -v`
Expected: all passed (auth + routes).

- [ ] **Step 7: Run the full suite**

Run: `cd ~/Projetos/claude-pocket/backend && uv run pytest -v`
Expected: every test passes.

- [ ] **Step 8: Manual end-to-end smoke (real tmux + curl)**

```bash
# terminal 1: a real Claude session
tmux new -d -s cc -c ~/Projetos/claude-pocket 'claude'
# terminal 2: run the API bound to localhost for the smoke (override bind)
cd ~/Projetos/claude-pocket/backend
CP_LAN_BIND_IP=127.0.0.1 CP_AUTH_TOKEN=secret uv run python -m app.main &
sleep 2
curl -s -H 'Authorization: Bearer secret' http://127.0.0.1:8765/api/sessions | python3 -m json.tool
curl -s -H 'Authorization: Bearer secret' -H 'content-type: application/json' \
     -d '{"text":"diga apenas PONG"}' http://127.0.0.1:8765/api/sessions/cc/input
# watch the live stream (Ctrl-C to stop):
curl -N -H 'Authorization: Bearer secret' http://127.0.0.1:8765/api/sessions/cc/events
```
Expected: `/api/sessions` lists `cc` with a `jsonl` path; the input call makes Claude respond in the tmux pane; the SSE stream prints `event: state` (thinking→idle) and `event: message` with the assistant reply. Then `tmux kill-session -t cc` and stop the server.

- [ ] **Step 9: Commit**

```bash
cd ~/Projetos/claude-pocket
git add backend/app/sse.py backend/app/api.py backend/app/main.py backend/tests/test_api.py
git commit -m "feat: expose sessions via REST + SSE API"
```

---

## Self-Review

**Spec coverage:**
- SessionRegistry → Task 9. TranscriptTailer → Tasks 4–5. StateMonitor → Tasks 6–7. TerminalInput → Task 8. SSE merge + REST routes (§5.5) → Task 11. Auth (§5.6) → Task 10. State machine (§7) → Tasks 6–7 + SSE. Risk spikes (§9, §10 A/B) → Task 1. JSONL real-time (§10 C) → confirmed pre-plan. LAN bind (§10 D) → config in Task 2, applied in Task 11/`main.py`.
- **Out of this plan (separate plans, by design):** Svelte frontend (§5 frontend), Caddy/TLS/firewall/iPhone cert deploy (§5.6 network), `history` reconnect via `Last-Event-ID` (§8 — v1 uses `/history` refetch; documented as acceptable), error states `dead` surfaced in UI.
- **Coverage gap noted:** `Last-Event-ID` reidratação (§5.5) is deferred — the SSE endpoint streams from "now"; initial load uses `GET /history`. Acceptable for v1; revisit if reconnect drops messages.

**Placeholder scan:** No TBD/TODO; every code step has complete code; spike (Task 1) is explicitly exploratory with concrete commands. Task 6/7 note a reconcile-with-spike step but ship working defaults.

**Type consistency:** `ChatEvent`/`StateEvent`/`SessionInfo` fields are used identically across Tasks 4–11. `classify` returns `(state, label, question, options)` consumed the same way in Task 7. `TerminalInput.send_prompt/select/interrupt` signatures match Task 11 calls. `SessionRegistry.list/create/kill/resolve_jsonl` match Task 11 usage. State set is `working/idle/awaiting_input/dead` everywhere.

## Notes for the deploy plan (Plan 3)
- `EventSource` can't send headers → frontend relies on the `cp_token` httpOnly cookie (same-origin via Caddy). Add a `POST /api/login` that sets the cookie before the frontend ships.
- Bind moves from `127.0.0.1` (smoke) to `192.168.77.23`; Caddy terminates TLS and serves the Svelte build same-origin.
