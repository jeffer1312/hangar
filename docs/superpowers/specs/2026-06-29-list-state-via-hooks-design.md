# Session-list state via Claude hooks (sub-project A) — design

**Date:** 2026-06-29
**Status:** approved, pending implementation plan

## Context

This is **sub-project A** of a larger effort (F2) to replace the 5s session-list polling with
push. F2 decomposes into three independent pieces, built in this order:

- **A (this):** the session **list** derives state (`working` / `idle` / `awaiting_input`) from
  Claude Code **hooks** instead of scraping the tmux pane.
- **C (later):** the frontend consumes a **list-level SSE** instead of the 5s GET poll — this is
  what actually stops the *frontend* network poll.
- **B (later):** session **lifecycle** (appeared/dead) via tmux **control mode**.

A alone does **not** stop the frontend poll (that is C). A changes only the *backend source* of
state: each `/api/sessions` call (and, later, C's push loop) reads a cheap in-memory map instead of
spawning a `capture-pane` per session. The win is backend CPU / subprocess churn and state
precision; it is the foundation a meaningful push (C) needs.

## Problem

`registry.list_with_state` classifies every session's state by running `tmux capture-pane` per
session and parsing the TUI (`state.py:classify`), plus a 150 ms spinner-disambiguation sleep. That
runs on every list call. The state is knowable more cheaply and precisely from Claude Code's own
hook events, which already carry the `session_id` (the existing `askq_capture.py` PreToolUse hook
reads `session_id` from the hook's stdin JSON and writes a per-session sidecar — the pattern is
proven in this repo).

## Scope

- In scope: the **session list** state (`working` / `idle` / `awaiting_input`).
- Out of scope: the **open chat**'s live state — `StateMonitor` + `preview.py` stay exactly as they
  are (the open chat already scrapes the pane for the live preview and shows the live working label
  like "Elucidating…", which hooks don't provide). Out of scope: lifecycle/`dead` detection (stays
  from tmux listing), control mode (B), and the list-SSE delivery (C).

## Design

### 1. `state_hook.py` — minimal hook (mirrors `askq_capture.py`)

A new hook script under `backend/hooks/`. Reads the hook event JSON from stdin, takes `session_id`
and the hook event name, maps it to a state, and writes
`<config>/.claude-pocket-state/<session_id>.json` containing `{"state": <state>, "ts": <epoch>}`.
Fails silently (never blocks the prompt; no stdout). Event → state map:

- `UserPromptSubmit`, `PreToolUse`, `PostToolUse` → `working` (covers turn start, mid-turn, and
  resume after a permission/answer)
- `Notification` → `awaiting_input`
- `Stop` → `idle`

`dead` is **not** produced here — a tmux session that disappears is simply no longer listed.

The hook resolves its config dir the same way `askq_capture.py` does:
`os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")`.

### 2. `hook_installer.py` — install the state hooks

Extend the existing installer to also register `state_hook.py` for the five events above, using the
same idempotent managed-block approach already used for the AskUserQuestion PreToolUse hook (insert
under each event's hook list; never clobber the user's other hooks; back up before replacing). The
`UserPromptSubmit`/`Notification`/`Stop` hooks are new event keys; `PreToolUse`/`PostToolUse` append
alongside any existing entries (the askq capture already occupies a `PreToolUse` matcher — append,
don't replace).

### 3. `hook_state.py` — the watcher + state map

A new backend module that watches the `.claude-pocket-state/` directory under every known config
dir (the registry already enumerates config dirs via `list_config_dirs`) using the same `watchfiles`
machinery the transcript tailer uses. It maintains an in-memory `dict[session_id → (state, ts)]`,
seeded by reading existing marker files on startup and updated on every file change. Exposes
`get_state(session_id) -> tuple[str, float] | None` (None when no marker exists).

### 4. `registry.list_with_state` — prefer the marker, fall back to the pane

For each listed session, derive `session_id` from the resolved jsonl (the basename without
`.jsonl`). If `hook_state.get_state(session_id)` returns a marker, use that state (no
`capture-pane`). Otherwise fall back to the current `classify(pane)` path. Net effect: pane-scraping
runs only for sessions that have no hook marker yet (manual `claude` started before hooks were
installed, or before its first hook fired); hooked sessions cost a map lookup.

The existing per-pane spinner disambiguation (the 150 ms sleep) only runs on the fallback path, so
it disappears for hooked sessions.

## Data flow

`Claude hook fires → state_hook.py writes <config>/.claude-pocket-state/<session_id>.json →
watchfiles notifies hook_state.py → in-memory map updated → list_with_state reads the map`. No
network, no `capture-pane` for hooked sessions.

## Edge cases

- **No marker:** fall back to `classify(pane)` (current behavior) — nothing regresses for un-hooked
  sessions.
- **Stale `working` marker** (a crash between `UserPromptSubmit` and `Stop`): the marker would read
  `working` indefinitely. Acceptable for v1 (rare); the session still drops out of the list when its
  tmux session dies. (A future refinement could ignore very old `working` markers; not in v1.)
- **`awaiting_input`:** comes from the `Notification` hook. If `Notification` proves not to fire for
  the AskUserQuestion / permission cases during implementation, the existing askq sidecar
  (`<config>/.claude-pocket-askq/<session_id>.json`, written by `askq_capture.py`) is a backup
  signal that awaiting is active — reconcile against it. Resolve which signal governs during impl.
- **Multiple config dirs:** the watcher watches each config dir's `.claude-pocket-state/`; markers
  are keyed by `session_id` (globally unique uuid), so no cross-dir collision.

## Testing / verification

Backend has a real test runner (`cd backend && uv run pytest`). Real TDD applies (unlike the
frontend). Cover:

- `state_hook` maps each event name to the correct state and writes the marker JSON keyed by
  `session_id` (feed it a synthetic stdin payload; assert the written file).
- `hook_state` seeds from existing markers on startup and updates the map on a file change.
- `list_with_state` prefers a present marker over the pane, and falls back to `classify(pane)` when
  no marker exists (mock `get_state` + the tmux capture).
- `hook_installer` idempotently registers the five event hooks without clobbering existing hooks
  (including the pre-existing askq `PreToolUse` entry).

## Files

- Create: `backend/hooks/state_hook.py`
- Create: `backend/app/hook_state.py`
- Modify: `backend/app/hook_installer.py` (register the state hooks)
- Modify: `backend/app/registry.py` (`list_with_state` prefers the marker)
- Tests: `backend/tests/test_state_hook.py`, `backend/tests/test_hook_state.py`, and additions to
  the registry test.
