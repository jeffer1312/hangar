# Dynamic Claude config-dir selection per session

**Date:** 2026-06-27
**Status:** approved (design), pending implementation

## Problem

A user can have several Claude Code config dirs (e.g. `~/.claude-clean`, `~/.claude-work`),
each its own login + project transcripts under `<dir>/projects/`. On the PC these are reached
via shell aliases (`cc` → clean, `claude` → work) that set `CLAUDE_CONFIG_DIR`.

Two gaps today:

1. **App-created sessions ignore the config dir.** `tmux.new_session` launches `claude
   --session-id` forwarding only `COLORTERM`/`CLAUDE_CODE_TMUX_TRUECOLOR`, NOT
   `CLAUDE_CONFIG_DIR`. So a "+"-created session lands in the default `~/.claude` (often a
   stale/wrong login) → "Not logged in" welcome screen → typed messages do nothing.
2. **Transcript resolution is hardcoded to one config dir.** `SessionRegistry` resolves the
   jsonl under `settings.projects_dir` (the *backend's* `CLAUDE_CONFIG_DIR/projects`). A
   session using any other config dir has its transcript elsewhere → not found / wrong chat.
   It "works" today only because the user's terminal sessions happen to use the same dir as
   the backend.

## Goals

- Detect available Claude config dirs **dynamically**, no mandatory config, cross-platform.
- Let the user **pick** the config dir when creating a session via the app (only surfaced
  when there is more than one — nobody with a single dir sees a picker).
- Resolve each session's transcript from the **config dir that session actually uses**,
  fixing app-created and terminal-created sessions alike.

## Non-goals

- Parsing shell aliases (fish/bash/zsh/PowerShell) — too fragile/OS-specific. The dir
  basename gives the friendly label; `CP_CLAUDE_CONFIG_DIRS` gives portable curation.
- Switching a running session's config dir. Choice is at creation only.

## Design

### 1. Detection — `list_config_dirs()` (in `config.py`)

Hybrid, priority order:

1. **`CP_CLAUDE_CONFIG_DIRS`** set → use exactly those. Format: comma-separated, each
   `label:path` or bare `path` (label defaults to basename). `~` expanded.
2. Else **auto-scan** `~/.claude*` for dirs with both `.credentials.json` and `projects/`.
   - label = basename with leading `.claude-` / `.claude` stripped (`.claude-clean` → `clean`,
     `.claude` → `default`).
   - sort by recency (newest mtime among the dir's `projects/*` children) so backups /
     abandoned dirs sink.

Returns `list[{path: str, label: str, active: bool}]`; `active` = the backend's own
`CLAUDE_CONFIG_DIR` (resolved via `_default_projects_dir`'s base).

### 2. Endpoint — `GET /api/claude-configs`

Auth-gated (`Depends(require_auth)`), `response_model=list[ConfigDirInfo]`. Returns the list
from `list_config_dirs()`.

### 3. Create — config dir flows into the new session

- `CreateBody` gains `config_dir: str | None = None`.
- Handler validates: if provided, `config_dir` MUST be one of the detected dirs' paths
  (allowlist) → else `HTTPException(400)`. Prevents an arbitrary `-e CLAUDE_CONFIG_DIR=...`.
- `registry.create(name, cwd, config_dir)` passes it through.
- `tmux.new_session(name, cwd, command, config_dir)` adds `-e CLAUDE_CONFIG_DIR=<config_dir>`.
  When `config_dir` is None, forward the **backend's own** `os.environ.get("CLAUDE_CONFIG_DIR")`
  (if set) so app-created sessions match the backend instead of falling into `~/.claude`.

### 4. Transcript resolution — per-session config dir from `/proc`

`SessionRegistry.resolve_tracked` already walks `/proc/<pid>` of the session for the
`--session-id`. Add: read `CLAUDE_CONFIG_DIR` from `/proc/<claude_pid>/environ`.

- `projects_dir_for_session = <CLAUDE_CONFIG_DIR>/projects` when present.
- Fallback chain when env absent: Claude default `~/.claude/projects` → `settings.projects_dir`.
- Use this per-session `projects_dir` for: the `--session-id` jsonl path, the open-fd
  `startswith` check, and the newest-by-mtime fallback.

This is the only change to core resolution; it degrades to today's behavior when the env is
absent and the session uses the backend's dir, so the chat keeps working.

### 5. Frontend picker

In the create-session ("+") flow:

- Fetch `GET /api/claude-configs`.
- **> 1 dir** → show a picker (chips/select), default = the `active` one.
- **<= 1 dir** → no picker; create with that single dir implicitly. Dynamic.
- Pass the chosen `config_dir` to `createSession(name, cwd, config_dir)` (api.ts).

### 6. Models

```python
class ConfigDirInfo(BaseModel):
    path: str
    label: str
    active: bool
```

## Validation / security

- `config_dir` from the client is allowlisted against the detected list (no arbitrary path).
- Token-holder already has full shell, so this is defense-in-depth/cleanliness, not a new
  boundary.

## Testing

- `list_config_dirs`: tmp home with synthetic `.claude*` dirs (login vs not, projects vs not),
  env-override path, label derivation, recency sort.
- create with `config_dir`: valid → forwarded to `new_session`; invalid → 400.
- `resolve_tracked`: mock `/proc` environ to return a config dir → resolves jsonl under that
  dir's `projects/`; absent env → falls back.

## Files touched

`backend/app/config.py` (detection + model), `backend/app/api.py` (endpoint + CreateBody +
create), `backend/app/registry.py` (create signature + per-proc resolution),
`backend/app/tmux.py` (new_session config_dir), frontend `src/lib/api.ts` + the
create-session component, plus tests.
