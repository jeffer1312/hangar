# claude-pocket

Drive a live [Claude Code](https://code.claude.com) session from your phone — over your own LAN/VPN, no vendor cloud — as a clean mobile chat.

You leave `claude` running in a `tmux` session on your machine. claude-pocket exposes that **same live session** to a phone: it renders the conversation as chat, shows what Claude is doing right now, and lets you send prompts, answer Claude's interactive questions, and interrupt — all from an iPhone on the couch.

> **Status:** Backend is complete and tested (221 passing tests). The Svelte PWA frontend is feature-rich — chat, uploads, git ops, slash commands, workflows, model picker, native AskUserQuestion. Typically run over a Tailscale tailnet (`tailscale serve` → HTTPS). Personal-use, single-user tool.

> **Using it?** Step-by-step guide — pairing, Tailscale, install as PWA, every feature: **[docs/USAGE.md](docs/USAGE.md)**.

## Why

The official remote options route through a vendor cloud. claude-pocket stays entirely on your own network: the phone talks to a small server on your machine, which talks to your already-running `claude`. Nothing leaves your LAN.

## How it works

The trick is to use the right source for each thing:

```
 iPhone (Safari PWA, same LAN/VPN)
   │  EventSource (SSE)  ◄──── messages + live state ────┐
   │  fetch POST  ───► input / select / interrupt        │
   ▼                                                     │
 Python API (FastAPI · uvicorn · Bearer/cookie auth)     │
   ├ SessionRegistry  → tmux list/new/kill, map → jsonl  │
   ├ TranscriptTailer → tail ~/.claude/…/<uuid>.jsonl ────┤ merge → SSE
   ├ StateMonitor     → tmux capture-pane → live state ───┤
   └ TerminalInput    → tmux send-keys (prompt/select/Esc)┘
   ▼
 tmux sessions, each running `claude` (your normal login)
```

- **Chat content** comes from Claude Code's structured **JSONL transcript** (`~/.claude/projects/<cwd>/<uuid>.jsonl`) — robust, no terminal scraping.
- **Live state** comes from a narrow `tmux capture-pane` read of the status line. States: `working` (mirrors Claude's live label, e.g. `Elucidating…`), `idle`, `awaiting_input` (Claude asked an interactive question — options become tappable buttons), `dead`.
- **Input** goes to the real session via `tmux send-keys`: prompts, option selection (`(n-1)×Down`+`Enter`), and interrupt (`Esc`).

## Run it (dev)

Requirements: `tmux`, `claude` (Claude Code), Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+.

**0. Install the `claude` wrapper (one-time, recommended):**
```bash
./scripts/install-claude-wrapper.sh          # auto-detects fish/bash/zsh; pass `all` for every shell
```
This makes the app track sessions reliably. After it, just run `claude` anywhere: it launches inside
a tmux session named after the folder, with a unique `--session-id`. That id is what binds each
session to its own transcript — so you can open **many sessions in the same folder** and none of them
leak into or overwrite another. A `claude` started **without** it (no `--session-id`, or outside
tmux) is either invisible to the app or shows up flagged **⚠ no id** with its chat disabled. The
installer also adds the tmux truecolor + window-rename config, and offers to set the claude-pocket
statusline (`scripts/omniroute-statusline.js`) as your Claude `statusLine` — that's the format the
app parses into the model / context / cost / rate-limit badges (decline to keep your own; pass
`--no-statusline` to skip). Bypass the wrapper anytime with `command claude`.

**1. Or start Claude inside tmux manually:**
```bash
tmux new -s cc          # then run `claude --session-id $(uuidgen)` inside it
```

> Theme colors look wrong inside tmux (teal / pink / washed-out)? That's a known Claude
> Code + tmux truecolor issue — see [docs/tmux-truecolor-setup.md](docs/tmux-truecolor-setup.md)
> for the one-line fix.
>
> Want the session to survive a reboot / OOM kill? Run `./scripts/tmux-persist-setup.sh`
> (auto-save + restore via resurrect/continuum) — see
> [docs/tmux-persistence-setup.md](docs/tmux-persistence-setup.md).

**2. Backend:**
```bash
cd backend
CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main
# binds http://127.0.0.1:8765 (set CP_LAN_BIND_IP to your LAN IP for phone access)
```

**3. Frontend:**
```bash
cd frontend
npm install
npm run dev            # open it, set the API base URL + token on the Login screen
```

Run the backend tests with `cd backend && uv run pytest -v`.

## Environment Variables

| Var | Default | Purpose |
|---|---|---|
| `CP_AUTH_TOKEN` | `change-me` | Bearer token protecting all routes — generate a strong one. Refuses to start with `change-me` on non-loopback. |
| `CP_LAN_BIND_IP` | `127.0.0.1` | `auto` = detect LAN IP for phone access; fixed IP also works. |
| `CP_PORT` | `8765` | Backend server port. |
| `CP_FRONT_PORT` | `5173` | Frontend dev server port (included in startup QR). |
| `CP_PUBLIC_URL` | — | Override QR base URL (e.g., Tailscale hostname). |
| `CP_SCAN_ROOTS` | — | Comma-separated paths for "New session" folder picker. |
| `CP_SYNC` | `false` | Enable cloud-sync hub (`/api/sync/*` routes) and "Criar acesso" registration. |
| `CP_SYNC_BOOTSTRAP` | — | One-time bootstrap secret for first-run account registration; locks after first use. |
| `CP_SYNC_DATA` | `~/.claude-pocket/sync-vault.json` | Path to encrypted server-list vault file. |
| `CP_SYNC_SESSION_SECRET` | — | HMAC key for session cookies; empty = random per process (logout on restart). |

## API

All routes require `Authorization: Bearer <token>` (SSE uses a `cp_token` cookie since `EventSource` can't set headers).

**Sessions**

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/sessions` | list tmux sessions + state |
| POST | `/api/sessions` | create a session (`{name, cwd}`) |
| DELETE | `/api/sessions/{name}` | kill a session |
| POST | `/api/sessions/{name}/rename` | rename a session |
| GET | `/api/claude-configs` | list available `CLAUDE_CONFIG_DIR` options |

**Transcript & stream**

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/sessions/{name}/history` | full transcript (initial load) |
| GET | `/api/sessions/{name}/events` | **SSE**: `message` / `state` / `preview` / `ask_question` / `ping` / `reset` |
| GET | `/api/sessions/{name}/transcript-image/{uuid}/{idx}` | image embedded in a transcript message |
| GET | `/api/sessions/{name}/pane` | raw `tmux capture-pane` (live peek / debug) |

**Input**

| Method | Route | Purpose |
|---|---|---|
| POST | `/api/sessions/{name}/input` | send a prompt (`{text}`) |
| POST | `/api/sessions/{name}/select` | answer an interactive menu (`{option}`, 1-based) |
| POST | `/api/sessions/{name}/answer` | answer a native AskUserQuestion |
| POST | `/api/sessions/{name}/interrupt` | send `Esc` |
| POST | `/api/sessions/{name}/keys` | send raw key(s) |
| POST | `/api/sessions/{name}/model-effort` | set model / reasoning effort |

**Files & uploads**

| Method | Route | Purpose |
|---|---|---|
| POST | `/api/sessions/{name}/upload` | upload a file into the session cwd |
| GET | `/api/sessions/{name}/uploads/{filename}` | fetch an uploaded file |
| GET | `/api/sessions/{name}/file` | read a file from the session cwd |
| GET | `/api/fs/roots` | list filesystem roots (new-session picker) |
| GET | `/api/fs/scan` | scan a dir for subfolders (new-session picker) |

**Git**

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/sessions/{name}/branches` | list git branches |
| POST | `/api/sessions/{name}/checkout` | checkout a branch |
| POST | `/api/sessions/{name}/git` | run a git op |

**Commands & workflows**

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/sessions/{name}/commands` | list available slash commands |
| GET | `/api/sessions/{name}/workflows` | list workflow runs |
| GET | `/api/sessions/{name}/workflows/{run_id}` | workflow run detail |
| GET | `/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}` | workflow agent detail |

## Security

A web terminal/agent over the network is arbitrary remote command execution if misconfigured. This tool is **LAN/VPN-only by design**:

- Bind to your LAN/VPN IP, **never** a public interface; never port-forward it on your router.
- A bearer token gates every route; put TLS in front (e.g. Caddy) before real use.
- It runs `claude` (and its tools) as you — treat the token like a shell password.

## Tech

Backend: Python 3.14, FastAPI, `sse-starlette`, `watchfiles`. Frontend: Svelte 5, Vite, TypeScript, PWA. Zero vendor cloud.

## License

MIT
