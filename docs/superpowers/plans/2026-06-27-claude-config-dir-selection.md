# Dynamic Claude config-dir selection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user pick which Claude config dir a new app-created session uses, detected dynamically, and resolve each session's transcript from the config dir that session actually runs with.

**Architecture:** Backend detects config dirs (env override or auto-scan of `~/.claude*`). The create endpoint accepts a validated `config_dir` and forwards it as `-e CLAUDE_CONFIG_DIR` to the new tmux session. The registry resolves each session's transcript by reading that session's live `CLAUDE_CONFIG_DIR` from `/proc/<pid>/environ`. Frontend shows a picker only when >1 dir exists.

**Tech Stack:** Python 3.14, FastAPI, Pydantic v2, pytest; Svelte 5 + TypeScript frontend.

Spec: `docs/superpowers/specs/2026-06-27-claude-config-dir-selection-design.md`

Run backend tests from `backend/` with `.venv/bin/python -m pytest`.

---

## Task 1: Config-dir detection in `config.py`

**Files:**
- Modify: `backend/app/config.py` (add `ConfigDirInfo`, `list_config_dirs`, helpers)
- Test: `backend/tests/test_config_dirs.py` (create)

- [ ] **Step 1: Write the failing test**

```python
# backend/tests/test_config_dirs.py
import os
from pathlib import Path
from app import config as cfg


def _make_dir(home: Path, name: str, *, login=True, projects=True, ts=None):
    d = home / name
    d.mkdir(parents=True, exist_ok=True)
    if login:
        (d / ".credentials.json").write_text("{}", encoding="utf-8")
    if projects:
        pj = d / "projects" / "ws"
        pj.mkdir(parents=True, exist_ok=True)
        f = pj / "a.jsonl"
        f.write_text("", encoding="utf-8")
        if ts:
            os.utime(f, (ts, ts))
    return d


def test_autoscan_finds_login_dirs_with_projects(tmp_path, monkeypatch):
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.setattr(cfg.Path, "home", classmethod(lambda cls: tmp_path))
    _make_dir(tmp_path, ".claude-work", ts=200)
    _make_dir(tmp_path, ".claude-clean", ts=100)
    _make_dir(tmp_path, ".claude-nologin", login=False)
    _make_dir(tmp_path, ".claude-noproj", projects=False)
    out = cfg.list_config_dirs()
    assert [c.label for c in out] == ["work", "clean"]  # recency: work(ts200) before clean(ts100)


def test_env_override_with_labels(tmp_path, monkeypatch):
    a = _make_dir(tmp_path, ".claude-work")
    b = _make_dir(tmp_path, ".claude-clean")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", f"trabalho:{a},{b}")
    out = cfg.list_config_dirs()
    assert [(c.label, c.path) for c in out] == [("trabalho", str(a.resolve())), ("clean", str(b.resolve()))]


def test_active_flag_matches_backend_config_dir(tmp_path, monkeypatch):
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.setattr(cfg.Path, "home", classmethod(lambda cls: tmp_path))
    work = _make_dir(tmp_path, ".claude-work")
    _make_dir(tmp_path, ".claude-clean")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(work))
    active = [c for c in cfg.list_config_dirs() if c.active]
    assert len(active) == 1 and active[0].path == str(work.resolve())
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_config_dirs.py -q`
Expected: FAIL — `module 'app.config' has no attribute 'list_config_dirs'`.

- [ ] **Step 3: Write minimal implementation**

In `backend/app/config.py` add `from pydantic import BaseModel` (alongside the existing pydantic_settings import), then add:

```python
class ConfigDirInfo(BaseModel):
    path: str
    label: str
    active: bool


def _backend_config_base() -> Path:
    return Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))


def _label_for(path: Path) -> str:
    stripped = path.name.removeprefix(".claude-").removeprefix(".claude")
    return stripped or "default"


def _is_config_dir(p: Path) -> bool:
    return (p / ".credentials.json").is_file() and (p / "projects").is_dir()


def _projects_mtime(p: Path) -> float:
    try:
        return max((c.stat().st_mtime for c in (p / "projects").iterdir()), default=0.0)
    except OSError:
        return 0.0


def list_config_dirs() -> list[ConfigDirInfo]:
    """Config dirs do Claude pra escolher na criacao. Hibrido: CP_CLAUDE_CONFIG_DIRS ('label:path'
    por virgula) tem prioridade; senao auto-scan de ~/.claude* com login + projects/, label pelo
    basename, ordenado por recencia (backup/abandonado afundam)."""
    active_base = _backend_config_base().resolve()
    raw = os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip()
    entries: list[tuple[str, Path]] = []
    if raw:
        for item in raw.split(","):
            item = item.strip()
            if not item:
                continue
            label, sep, path = item.partition(":")
            if not sep:
                path, label = label, ""
            p = Path(os.path.expanduser(path)).resolve()
            entries.append((label.strip() or _label_for(p), p))
    else:
        found = [p.resolve() for p in Path.home().glob(".claude*") if p.is_dir() and _is_config_dir(p)]
        found.sort(key=_projects_mtime, reverse=True)
        entries = [(_label_for(p), p) for p in found]
    out, seen = [], set()
    for label, p in entries:
        s = str(p)
        if s in seen:
            continue
        seen.add(s)
        out.append(ConfigDirInfo(path=s, label=label, active=(p == active_base)))
    return out
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_config_dirs.py -q`
Expected: PASS (3 passed).

- [ ] **Step 5: Commit**

```bash
git add backend/app/config.py backend/tests/test_config_dirs.py
git commit -m "feat(config): detect available Claude config dirs (env override + auto-scan)"
```

---

## Task 2: `tmux.new_session` forwards `CLAUDE_CONFIG_DIR`

**Files:**
- Modify: `backend/app/tmux.py` (`new_session` signature + `-e CLAUDE_CONFIG_DIR`; add `import os`)
- Test: `backend/tests/test_tmux.py` (append)

- [ ] **Step 1: Write the failing test**

```python
# append to backend/tests/test_tmux.py
from unittest.mock import patch
from app import tmux as tmux_mod


class _CP:
    returncode = 0
    stdout = ""
    stderr = ""


def test_new_session_forwards_explicit_config_dir():
    captured = {}
    with patch.object(tmux_mod, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux_mod.new_session("s", "/tmp", "claude --session-id x", config_dir="/home/u/.claude-clean")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-clean" in captured["args"]


def test_new_session_falls_back_to_backend_config_dir(monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/home/u/.claude-work")
    captured = {}
    with patch.object(tmux_mod, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux_mod.new_session("s", "/tmp", "claude --session-id x")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-work" in captured["args"]
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_tmux.py -k new_session_forwards -q`
Expected: FAIL — `new_session() got an unexpected keyword argument 'config_dir'`.

- [ ] **Step 3: Write minimal implementation**

In `backend/app/tmux.py`, add `import os` at the top, and replace `new_session` body (keep its existing comment block above):

```python
def new_session(name: str, cwd: str, command: str, config_dir: str | None = None) -> bool:
    cfg = config_dir or os.environ.get("CLAUDE_CONFIG_DIR")
    args = [
        "tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        "-e", "COLORTERM=truecolor",
        "-e", "CLAUDE_CODE_TMUX_TRUECOLOR=1",
    ]
    if cfg:
        # sessao app-criada usa o MESMO config dir que o backend (ou o escolhido), em vez de cair
        # no ~/.claude default (deslogado -> tela de boas-vindas).
        args += ["-e", f"CLAUDE_CONFIG_DIR={cfg}"]
    args.append(command)
    return _run(args).returncode == 0
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_tmux.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/app/tmux.py backend/tests/test_tmux.py
git commit -m "feat(tmux): forward CLAUDE_CONFIG_DIR (chosen or backend's) to new sessions"
```

---

## Task 3: Registry — `create(config_dir)` + per-session transcript resolution

**Files:**
- Modify: `backend/app/registry.py` (`_config_dir_of` helper, `resolve_tracked` per-proc projects dir, `create` signature)
- Test: `backend/tests/test_registry.py` (append)

- [ ] **Step 1: Write the failing test**

```python
# append to backend/tests/test_registry.py
from app import registry as reg_mod
from app.registry import SessionRegistry, sanitize_cwd


def test_resolve_uses_session_config_dir(tmp_path, monkeypatch):
    cfg = tmp_path / ".cfg"
    sid = "11111111-1111-1111-1111-111111111111"
    cwd = "/work/proj"
    jpath = cfg / "projects" / sanitize_cwd(cwd) / f"{sid}.jsonl"
    jpath.parent.mkdir(parents=True, exist_ok=True)
    jpath.write_text("", encoding="utf-8")

    monkeypatch.setattr(reg_mod.tmux, "pane_pid", lambda name: 4242)
    monkeypatch.setattr(reg_mod, "_descendant_pids", lambda root: [4242])
    monkeypatch.setattr(reg_mod, "_cmdline", lambda pid: f"claude --session-id {sid}")
    monkeypatch.setattr(reg_mod, "_config_dir_of", lambda pid: cfg)

    SessionRegistry._jsonl_cache.clear()
    r = SessionRegistry(projects_dir=tmp_path / "backend-projects")  # intentionally a different dir
    resolved, tracked = r.resolve_tracked("cc", cwd)
    assert tracked is True
    assert resolved == str(jpath)   # used the SESSION's config dir, not the backend's
    SessionRegistry._jsonl_cache.clear()
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_registry.py -k resolve_uses_session_config_dir -q`
Expected: FAIL — `module 'app.registry' has no attribute '_config_dir_of'`.

- [ ] **Step 3: Write minimal implementation**

In `backend/app/registry.py`, add near `_cmdline`:

```python
def _config_dir_of(pid: int) -> Optional[Path]:
    # CLAUDE_CONFIG_DIR do processo claude (setado pelo alias/picker). None se ausente -> fallback.
    try:
        with open(f"/proc/{pid}/environ", "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CLAUDE_CONFIG_DIR="):
                    return Path(kv.split(b"=", 1)[1].decode(errors="replace"))
    except OSError:
        return None
    return None
```

In `resolve_tracked`, the cmdline loop derives the per-session projects dir:

```python
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if sid:
                    cdir = _config_dir_of(p)
                    proj = (cdir / "projects") if cdir else self.projects_dir
                    j = str(proj / sanitize_cwd(cwd) / f"{sid}.jsonl")
                    self._jsonl_cache[name] = j
                    return j, True
```

Change `create`:

```python
    def create(self, name: str, cwd: str, config_dir: str | None = None) -> SessionInfo:
        name = re.sub(r"[^A-Za-z0-9_-]", "-", name.strip()).strip("-")
        if not name:
            raise ValueError("nome invalido")
        if tmux.has_session(name):
            raise ValueError("ja existe uma sessao com esse nome")
        sid = str(uuid.uuid4())
        base = (Path(config_dir) / "projects") if config_dir else self.projects_dir
        jsonl = str(base / sanitize_cwd(cwd) / f"{sid}.jsonl")
        if not tmux.new_session(name, cwd, f"claude --session-id {sid}", config_dir):
            raise ValueError("falha ao criar sessao no tmux")
        self._jsonl_cache[name] = jsonl
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_registry.py -q`
Expected: PASS (all registry tests).

- [ ] **Step 5: Commit**

```bash
git add backend/app/registry.py backend/tests/test_registry.py
git commit -m "feat(registry): per-session config dir for create + transcript resolution"
```

---

## Task 4: API — `/api/claude-configs` endpoint + `CreateBody.config_dir`

**Files:**
- Modify: `backend/app/api.py` (import, `CreateBody.config_dir`, endpoint, validation in `create_session`)
- Test: `backend/tests/test_api.py` (append)

- [ ] **Step 1: Write the failing test**

```python
# append to backend/tests/test_api.py
from fastapi.testclient import TestClient
import app.api as api_mod
from app.api import app
from app.config import settings as _settings


def _auth():
    return {"Authorization": f"Bearer {_settings.auth_token}"}


def test_claude_configs_endpoint(monkeypatch):
    monkeypatch.setattr(api_mod, "list_config_dirs",
                        lambda: [api_mod.ConfigDirInfo(path="/h/.claude-work", label="work", active=True)])
    r = TestClient(app).get("/api/claude-configs", headers=_auth())
    assert r.status_code == 200
    assert r.json() == [{"path": "/h/.claude-work", "label": "work", "active": True}]


def test_create_rejects_unknown_config_dir(monkeypatch):
    monkeypatch.setattr(api_mod, "list_config_dirs",
                        lambda: [api_mod.ConfigDirInfo(path="/h/.claude-work", label="work", active=True)])
    r = TestClient(app).post("/api/sessions", headers=_auth(),
                             json={"name": "x", "cwd": "/tmp", "config_dir": "/h/.evil"})
    assert r.status_code == 400
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_api.py -k "claude_configs or unknown_config_dir" -q`
Expected: FAIL — endpoint 404 / `config_dir` rejected by `extra='forbid'`.

- [ ] **Step 3: Write minimal implementation**

In `backend/app/api.py` extend the config import:
```python
from app.config import settings, list_config_dirs, ConfigDirInfo
```
(If `settings` was not previously imported in api.py, this line adds it; otherwise just add the two new names.)

Add `config_dir` to `CreateBody`:
```python
class CreateBody(_StrictBody):
    name: str = Field(min_length=1)
    cwd: str = Field(min_length=1)
    config_dir: str | None = None
```

Add the endpoint near the other session routes:
```python
@app.get("/api/claude-configs", dependencies=[Depends(require_auth)], response_model=list[ConfigDirInfo])
def claude_configs():
    return list_config_dirs()
```

Validate + forward in `create_session`:
```python
@app.post("/api/sessions", dependencies=[Depends(require_auth)], response_model=SessionInfo)
def create_session(body: CreateBody):
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, "config_dir invalido")
    try:
        return registry.create(body.name, body.cwd, body.config_dir)
    except ValueError as e:
        raise HTTPException(409, str(e))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_api.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/app/api.py backend/tests/test_api.py
git commit -m "feat(api): GET /api/claude-configs + validated config_dir on create"
```

---

## Task 5: Frontend — list helper, create param, picker

**Files:**
- Modify: `frontend/src/lib/types.ts` (add `ConfigDirInfo`)
- Modify: `frontend/src/lib/api.ts` (add `listClaudeConfigs`, extend `createSession`)
- Modify: the create-session component (found via grep below)
- Verify: `cd frontend && npm run check`

- [ ] **Step 1: Locate the create flow**

Run: `grep -rn "createSession" frontend/src`
Note the component that calls it — that is where the picker goes.

- [ ] **Step 2: Add the type** (`frontend/src/lib/types.ts`)

```typescript
export interface ConfigDirInfo {
  path: string;
  label: string;
  active: boolean;
}
```

- [ ] **Step 3: Add API helpers** (`frontend/src/lib/api.ts`)

```typescript
export function listClaudeConfigs(): Promise<ConfigDirInfo[]> {
  return apiFetch<ConfigDirInfo[]>('/api/claude-configs');
}
```
Extend the existing `createSession` to take and send an optional config dir (keep its current return type):
```typescript
export function createSession(name: string, cwd: string, configDir?: string | null): Promise<SessionInfo> {
  return apiFetch<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ name, cwd, config_dir: configDir ?? null }),
  });
}
```
Import `ConfigDirInfo` from `./types` where the other types are imported.

- [ ] **Step 4: Add the picker** (create-session component)

On open: `listClaudeConfigs()` → `configs`; default `selected` to the `active` one's path. Render the picker ONLY when `configs.length > 1`. Pass `selected` to `createSession`. Svelte 5 sketch (adapt to the component's existing style):
```svelte
<script lang="ts">
  import { listClaudeConfigs, createSession } from '../lib/api';
  import type { ConfigDirInfo } from '../lib/types';
  let configs = $state<ConfigDirInfo[]>([]);
  let selected = $state<string | null>(null);
  $effect(() => {
    listClaudeConfigs()
      .then((cs) => { configs = cs; selected = cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null; })
      .catch(() => {});
  });
  // in the submit handler: await createSession(name, cwd, selected);
</script>

{#if configs.length > 1}
  <label class="cfg-pick">
    Claude config
    <select bind:value={selected}>
      {#each configs as c}
        <option value={c.path}>{c.label}{c.active ? ' (atual)' : ''}</option>
      {/each}
    </select>
  </label>
{/if}
```

- [ ] **Step 5: Type-check**

Run: `cd frontend && npm run check`
Expected: no NEW errors in touched files (the 2 pre-existing `MessageList.svelte` errors are unrelated).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/api.ts frontend/src/lib/types.ts frontend/src/<create-component>.svelte
git commit -m "feat(frontend): config-dir picker in the new-session flow"
```

---

## Task 6: Full verification + manual smoke

- [ ] **Step 1: Backend suite**

Run: `cd backend && .venv/bin/python -m pytest -q`
Expected: new tests pass; only the 2 pre-existing `test_state_classifier` failures remain.

- [ ] **Step 2: ruff**

Run: `cd backend && uvx ruff check app/`
Expected: All checks passed.

- [ ] **Step 3: Restart + manual smoke**

Restart per the project procedure (kill by port pid, `setsid` relaunch — NOT `pkill -f app.main`). Then:
- `GET /api/claude-configs` (with token) returns the expected dirs.
- Create a session via the app picking the non-default dir; confirm its claude is **logged in** (not the welcome screen) and the chat/transcript renders (resolution found the jsonl under the chosen dir's `projects/`).

---

## Self-review

- **Spec coverage:** detection (T1), endpoint (T4), create+validation (T4), config-dir forward (T2), per-session resolution (T3), picker (T5), tests (T1–4). All mapped.
- **Type consistency:** `ConfigDirInfo{path,label,active}` identical in config.py / api.py response_model / types.ts. `create(name, cwd, config_dir=None)` ↔ `new_session(..., config_dir=None)` ↔ `CreateBody.config_dir` ↔ `createSession(name, cwd, configDir)`.
- **`extra='forbid'` note:** `config_dir` must be a declared field on `CreateBody` (T4 Step 3) so the body validates; clients omitting it still work (defaults None).
