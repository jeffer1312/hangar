---
branch: main
saved_at: 2026-06-25T15:20:00-03:00
saved_commit: 0208afd2ccd8bae18b7974a47269b25e8b2ccc8a
plan: 2026-06-25-claude-pocket-backend.md
status: in_progress
---

## TL;DR
claude-pocket: drive a live Claude Code tmux session from the phone, LAN-only. Backend was done+pushed earlier. THIS session: validated the FULL phone→claude loop on a real tmux/claude session via a browser, fixed everything it surfaced (6 bugs), and added a polish batch (statusline-on-web, bottom status bar, chat dedupe, command-meta filtering, width cap). All verified live in the browser; 53 tests pass; COMMITTED to main (not pushed). Next = Plan 3 (deploy/onboarding): auto-detect LAN IP, QR pairing, Caddy/TLS.

## Task atual
Plan 3 — deploy & onboarding. Make it reachable from the phone with minimal friction: (1) CP_LAN_BIND_IP=auto auto-detect, (2) QR pairing on startup (URL+token, scan=in), (3) Caddy + TLS in front. Decisions already recorded in docs/onboarding-and-network.md. Caddy needs installing on this machine (was MISSING).

## Concluído nesta sessão
- Proved end-to-end in a real browser (agent-browser) on a real claude tmux session: login/auth, history, live SSE, state working/idle correct, send-from-composer→claude replies, interrupt, command-meta filtered, statusline shown, live append.
- Fixed 6 bugs (all surfaced by the live e2e): #1 projects_dir hardcoded ~/.claude → CLAUDE_CONFIG_DIR-aware; #2 SSE Python-repr → model_dump_json (valid JSON); #3 cookie auth_token→cp_token; #4 classify false-"working" from frozen markers → temporal animation detection; #6 sent msg needed refresh → dedupe-by-id + reconnect (live now); #7 Claude Code command-meta (/clear, caveats) leaking as bubbles → filtered in parse_line.
- Feature: raw terminal statusline surfaced to the web (StateEvent.status_line), shown verbatim in a bottom status bar with the state pill (badge moved below). Each user sees their own statusline.
- Dev wiring: vite /api proxy (same-origin) + cp_token cookie. lan_bind default → 127.0.0.1.
- Frontend: bottom "dock" (StatusBar + Composer), content width capped ~600px, iOS-keyboard visualViewport handling moved to the dock.
- 46 → 53 tests. Committed all of it to main.
- Recorded Plan-3 decisions in docs/onboarding-and-network.md.

## Decisões
- Dev auth = vite proxy same-origin + cp_token cookie (rejected CORS + ?token= query: leak + surface). Python loopback; prod = Caddy front.
- Onboarding (Plan 3) = QR pairing (QR carries LAN-IP+token, scan=logged-in, token never typed) + CP_LAN_BIND_IP=auto (UDP-connect autodetect). Keep bearer token (no user/pass login). Default bind loopback. (docs/onboarding-and-network.md)
- #4: a static pane can't tell a live spinner from a frozen "<glyph> <word> for <N>s" marker → classify flags any bottom-most spinner as working; StateMonitor downgrades a non-animating one to idle (STALE_LIMIT=3; first sight waits for a change).
- statusline shown RAW/verbatim (not parsed) — "igual no terminal", portable across each user's custom statusline config.
- Chat dedupe by id (SSE replays full history every connect + loadHistory seeds → would double).

## Limitações conhecidas
- iOS keyboard: visualViewport lift moved from Composer to the bottom dock — works on desktop, NOT verifiable here. Validate on the real iPhone.
- Width = a centered content column (~600px), NOT a literal phone frame on desktop. Refine if wanted.
- #5 (minor): GET /api/sessions always state="idle" (registry.list never computes it; real state only via SSE). Computing in the list reintroduces the #4 static ambiguity.
- Plan 3 NOT done: no auto-detect, no QR, no Caddy/TLS. Caddy not installed on this machine.

## Erros / armadilhas
- This machine: CLAUDE_CONFIG_DIR=/home/.../.claude-work → transcripts in ~/.claude-work/projects, not ~/.claude.
- Start claude as the DIRECT tmux pane command (`tmux new-session -d -s cc -c <dir> 'claude'`), NOT via an interactive shell (p10k wizard eats keystrokes). Fresh cwd → trust + external-imports prompts; send Enter to accept.
- Foreground `sleep` is blocked by the harness; don't use it in Bash one-liners.
- /events and /history 404 until the session's transcript exists (claude writes the jsonl lazily on first message).

## Arquivos criticos
- backend/app/state.py (R) — temporal classify/StateMonitor (#4) + status_line() extractor.
- backend/app/transcript.py (R) — command-meta filter (#7).
- backend/app/config.py (R) — CLAUDE_CONFIG_DIR projects_dir (#1) + loopback bind default.
- backend/app/sse.py (R) — model_dump_json (#2). backend/app/models.py (R) — StateEvent.status_line.
- frontend/src/screens/Chat.svelte (R) — dedupe (#6) + bottom dock + keyboard. frontend/src/components/StatusBar.svelte (N) — statusline bar.
- frontend/src/lib/auth.ts (R) cp_token cookie (#3); frontend/vite.config.ts (R) /api proxy.
- docs/onboarding-and-network.md (N) — Plan 3 decisions (READ FIRST for Plan 3).

## Próximo passo
```
# Plan 3 (deploy/onboarding). Read docs/onboarding-and-network.md first.
# 1. Auto-detect LAN IP — CP_LAN_BIND_IP=auto resolves the primary LAN IP (UDP-connect to
#    8.8.8.8, no traffic). Edit backend/app/config.py + main.py; TDD in backend/tests.
# 2. QR pairing on startup — add a qrcode dep (uv add qrcode), build http://<lan-ip>:<port>
#    + token, print an ASCII QR in main.py startup. Phone scans -> URL+token auto-filled.
#    (Frontend: accept token via URL/query on the Login screen for the QR deep-link.)
# 3. Caddy + TLS — install caddy (system, needs user), write a Caddyfile that serves the
#    built PWA + reverse-proxies /api to 127.0.0.1:8765 with a LAN cert for the iPhone.
# Bring the stack back up to test: see README "Run it (dev)"; tmux cc + backend :8765 + vite :5173.
# resume: /handoff resume  (after git pull)
```
