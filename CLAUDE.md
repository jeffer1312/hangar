# claude-pocket

Drive a live Claude Code session (running in a `tmux` session on your machine) from your phone over
LAN/VPN, as a mobile chat. Single-user, LAN/VPN-only by design. Backend: Python 3.14 + FastAPI
(`backend/`). Frontend: Svelte 5 PWA (`frontend/`).

- **Architecture + full API table + run guide:** [`README.md`](README.md).
- **End-user / setup guide** (pairing, Tailscale, install as PWA, every feature): [`docs/USAGE.md`](docs/USAGE.md).
- Other docs in `docs/`: design brief, onboarding/network, polish backlog, tmux setup, future features.

## Architecture at a glance

The app never scrapes the terminal for chat content — it reads Claude Code's **JSONL transcript** and
only peeks at the tmux pane for live **state**. Backend pieces (`backend/app/`):

- `registry.py` — SessionRegistry: tmux list/new/kill ↔ maps each session to its `<uuid>.jsonl`.
- `transcript.py` — tails `~/.claude/projects/<cwd>/<uuid>.jsonl` (the chat content).
- `state.py` — classifies live state from `tmux capture-pane`: `working` / `idle` / `awaiting_input` / `dead`.
- `terminal_input.py` + `tmux.py` — input via `tmux send-keys` (prompt / option select via `(n-1)×Down`+`Enter` / `Esc`).
- `sse.py` — merges the above into the SSE stream. `api.py` — FastAPI routes. `auth.py` — bearer token / `cp_token` cookie.
- Also: `pqueue.py` (durable input queue), `preview.py` (live in-flight block), `askquestion.py`
  (native AskUserQuestion stepper), `uploads.py`, `git_ops.py`, `commands.py`, `workflows.py`,
  `model_picker.py`, `config.py`, `fs.py`, `hook_installer.py`.

Frontend (`frontend/src/`): `screens/` (Chat, …), `components/` (MessageList, NavBar, Composer, bubbles,
sheets, Spinner/Lottie, …), `lib/` (`api.ts` SSE client, `activity.ts`, `markdown.ts`, `format.ts`,
`types.ts`), `app.css` (design tokens + shared keyframes).

## Dev commands

Requirements: `tmux`, `claude` (Claude Code), Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+.
Frontend uses **npm** (has `package-lock.json`).

```bash
# Backend — binds http://127.0.0.1:8765 (set CP_LAN_BIND_IP to a LAN IP for phone access)
cd backend && CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main
cd backend && uv run pytest -v             # backend test suite

# Frontend (run from repo root with --prefix, or cd frontend first)
npm --prefix frontend run dev              # Vite dev server
npm --prefix frontend run build            # production build — does NOT typecheck
npm --prefix frontend run check            # svelte-check + tsc — THIS is the type gate
```

Sessions must run as `claude --session-id <uuid>` **inside tmux** — `scripts/install-claude-wrapper.sh`
sets this up. A `claude` without an id, or outside tmux, is invisible to the app or flagged ⚠ no id.

## SSE event model

The frontend `EventSource` (`screens/Chat.svelte`) listens for:

- `message` — transcript events: `user_msg` / `assistant_msg` / `tool_use` / `tool_result`.
- `state` — live state + status line (model / context / cost / rate badges).
- `preview` — live in-flight assistant text (full-replace; dropped when the real block commits).
- `ask_question` — opens the native AskUserQuestion sheet.
- `ping` — liveness heartbeat; resets a 25s watchdog that reconnects on half-open connections.
- `reset` — transcript swapped (e.g. `/clear`) → wipe and reload history.

## Conventions & gotchas (read before touching UI / backend lifecycle)

- **Two views: mobile & desktop (820px breakpoint).** `App.svelte` switches on
  `matchMedia('(min-width: 820px)')`: desktop → `DesktopShell` (which uses `Sidebar.svelte`), mobile →
  `SessionList.svelte`. Lots of UI has a per-view path (the session list is the clearest — `Sidebar` vs
  `SessionList`; sheets also re-dock as a right-side panel via `@media (min-width: 820px)`). Whenever you
  touch the front, make the change in BOTH views and verify BOTH — they drift apart easily (e.g. the
  session-list ordering ended up alphabetical only in `SessionList`, not `Sidebar`).

- **iOS black-rectangle repaint.** Glass on NavBar/Composer lives in a `::before` leaf with a near-opaque
  solid bg and **no** `backdrop-filter` / `transform` / `translateZ` on WebKit — those promote a layer that
  renders pure black during momentum scroll. Don't reintroduce them. Liquid-glass blur is Chromium-only
  (`html[data-liquid]`).
- **The message list is windowed.** `MessageList.svelte` mounts only the last `WINDOW=120` events; scroll-to-top
  reveals older pages (in-memory, no backend call). Don't render the whole transcript at once.
- **Queue/pending dedup.** Messages sent while Claude is `working` echo as `pending` / `queued-` bubbles and
  reconcile against the real transcript by normalized text/line. Touch `Chat.svelte` dedup carefully.
- **The phone app renders `AskUserQuestion` natively.** The `ask_question` SSE event opens the
  `AskQuestionSheet` stepper; since the pending payload isn't in the jsonl, a PreToolUse hook
  (`askq_capture.py`, installed idempotently by `hook_installer.py`) captures it into a sidecar. Verified
  live: use AskUserQuestion freely, it shows as the stepper. Numbered plain text is only a fallback for a
  session where that capture hook isn't installed. Raw TUI option pickers (not the tool) surface separately
  via `OptionButtons` (selection sent by `terminal_input.py`), so free composer text does not answer a picker.
- **Restarting the backend.** No `--reload` (it holds SSE + watchfiles). `pkill -f app.main` can match your
  own shell; SIGTERM can hang on an open SSE connection. Kill `-9` the pid bound to the port and relaunch
  detached (`setsid`).
- **CSS animations.** Shared tokens/keyframes live in `app.css` (`--ease-out`, `--spring`, …); a global
  `prefers-reduced-motion` rule neutralizes loops, so new keyframes don't each need their own guard.

## tmux + Claude Code truecolor

Inside tmux, Claude Code caps color depth to 256 and renders theme colors wrong (teal / pink / washed-out)
while rendering correctly outside tmux. Fix in the **shell rc** (settings.json env is unreliable here):

```sh
export COLORTERM=truecolor
export CLAUDE_CODE_TMUX_TRUECOLOR=1
```

Plus a `~/.tmux.conf` with `default-terminal "xterm-256color"` (not `tmux-256color`). Full explanation, fish
syntax, reference config, and verify steps: [`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md)
and [`docs/tmux.conf.example`](docs/tmux.conf.example).
