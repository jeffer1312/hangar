---
branch: main
saved_at: 2026-06-25T18:15:00-03:00
saved_commit: c7be3aface8729ed460fb31c938e27e2956790e0
plan: 2026-06-25-claude-pocket-backend.md
status: in_progress
---

## TL;DR
claude-pocket: drive a live Claude Code tmux session from the phone (LAN/Tailscale). Backend+frontend core were done before. THIS (huge) session shipped TWO big things, all committed: (1) **Plan 3** deploy/onboarding — QR pairing + Tailscale HTTPS + firewall + project scanner; (2) a **mobile UI redesign** (deep research workflow → proposal → Phases 1/2a/2b/3 + a model/effort session-only fix). Stopped before: redesign **Slice 1B** (robust metrics) + **Phase 4** (top bar + motion polish), and NEW backlog the user just asked for: **see running agents/workflows** + **image/file attachments** (audio later). Commits are local — PUSH PENDING. Dev stack is running.

## Task atual
UI redesign in progress. Remaining, in order: Slice 1B (metrics from transcript usage) → Phase 4 (5h/7d top-bar chips + UsageSheet + full motion grammar) → then the new backlog (running-agents view; attachments). Also: browser-verify Phase 3 (scanner/sessions) and the model/effort "Aplicar nesta sessão" flow (built + unit-tested + endpoint live after restart, but NOT yet browser/phone-verified).

## Concluído nesta sessão
- **Plan 3** (commit 75fcc92, PUSHED): CP_LAN_BIND_IP=auto+detect_lan_ip; QR pairing (backend prints QR of PWA URL+token; frontend ?token= auto-login; in-app QR scanner via qr-scanner); Tailscale HTTPS (`tailscale serve` → vite, trusted cert); scripts/lan-setup.sh (ufw) + show-qr.sh.
- **Redesign** (commits 0dcb0ff..HEAD, NOT pushed):
  - Phase 1 (0dcb0ff): killed the broken `<pre>` statusline; one composer-card = status row (state + live mm:ss + cost) + textarea + control row (Codex-style ContextRing + model·effort pill + send↔stop morph). lib/statusline.ts parser. StatusBar retired. + emil polish (strong --ease-out, global button :active, bubble easing).
  - Phase 2a (9317a7c): model·effort pill → BottomSheet ModelEffortSheet.
  - Phase 2b (1f61f0d): slash commands (backend GET /api/sessions/{name}/commands = builtins + cwd/skills scan; frontend SlashSuggest strip + CommandSheet); effort control.
  - Phase 3 (a82bdb5): backend GET /api/fs/roots + /api/fs/scan (allowlist CP_SCAN_ROOTS=~/pessoal,~/sistemas, realpath containment, escape rejection); FolderScanner in CreateSessionSheet (drill-in, open-vs-new dedupe); SessionCard/List reshape; NavBar in-chat switcher.
  - model/effort fix (HEAD): session-only switching (see Decisions). 101 backend tests, builds clean.
- Research artifact: docs/ui-redesign-proposal.md (proposal + critique). Requirements: docs/ui-redesign-requirements.md. New backlog: docs/future-features.md.

## Decisões (não-óbvias — críticas)
- **Model/effort session-only (KEY):** full-arg `/model <x>` and `/effort <y>` SAVE AS THE USER'S DEFAULT (wrong). The ONLY session-only path is Claude Code's interactive `/model` picker (a UNIFIED model+effort picker) + pressing **`s`** (`Enter`=default, `Esc`=cancel). Backend `model_picker.py` + `terminal_input.set_model_effort` + `POST /api/sessions/{name}/model-effort {model?,effort?,scope}` drive it NON-blind (open picker, read pane, Up/Down to the model row — number keys confirm-as-default so AVOID them — Right to step effort which is a model-dependent set, then `s`). The sheet applies only on the "Aplicar nesta sessão" button (no command on slider move).
- **Metrics = HYBRID** (user choice): statusline parse (client-side, works for THIS user's rich statusline) is what Phase 1 ships; the robust/portable path (Slice 1B, NOT done) = the transcript `message.usage` (VERIFIED present: input_tokens, output_tokens, cache_read/creation + model) → real ctx tokens/turn/model. cost/5h/7d only exist in the user's custom statusline.
- TLS = Tailscale serve (trusted cert, no manual install). awaiting_input = OptionButtons default (free-text into a TUI menu is risky). Subagents CANNOT invoke skills → the design skills (ui-ux-pro-max/impeccable/design-taste/emil) were applied in the MAIN LOOP / embedded in the research workflow prompts.

## Limitações conhecidas
- Phase 3 (scanner/sessions) + the model/effort "Aplicar" flow are built + tested but NOT browser/phone-verified yet.
- Slice 1B not done → on a default Claude Code install the metrics row would be sparse (this user's statusline is rich so it's fine for them now).
- `/api/sessions` list still returns state=idle always (real state only via SSE) — so Phase-3 session pulse/sort are mostly inert until that's populated.
- Driving the `/model` picker is inherently fragile (TUI layout/keys); guarded with Esc-on-failure.
- agent-browser daemon was flaky (timeouts/blank) — retries usually recover.

## Erros / armadilhas
- This machine: CLAUDE_CONFIG_DIR=/home/.../.claude-work → transcripts under ~/.claude-work/projects.
- Start claude as the DIRECT tmux pane command (`tmux new-session -d -s cc -c <dir> 'claude'`) — an interactive shell triggers the p10k wizard. Fresh cwd → trust + external-imports prompts (Enter to accept).
- Foreground `sleep` is blocked by the harness. Backend stdout is block-buffered when redirected (QR shows with flush / in a real tty).
- Tailscale HTTPS must be enabled in the admin console (one-time, done). `tailscale serve`/`cert` need root or `tailscale set --operator=$USER`.

## Arquivos criticos
- Redesign docs: docs/ui-redesign-proposal.md (R the proposal+critique FIRST to continue), docs/ui-redesign-requirements.md, docs/future-features.md (N — the new backlog), docs/onboarding-and-network.md.
- Frontend (N/R): src/components/{Composer,ContextRing,LiveMetrics,ModelEffortSheet,BottomSheet,CommandSheet,SlashSuggest,FolderScanner,SessionSwitcherSheet,SessionCard}.svelte, src/lib/{statusline.ts,format.ts,api.ts}, src/screens/{Chat,SessionList}.svelte, src/app.css (easing/button:active).
- Backend (N/R): app/{statusline parse in state.py, model_picker.py(N), fs.py(N), commands.py(N), terminal_input.py, api.py, config.py, models.py}; tests/{test_model_picker,test_commands,test_fs_scan,test_config,...}.

## Próximo passo
```
# Bring the stack up (token is per-launch; pick one), then resume:
cd backend && CP_AUTH_TOKEN=<tok> CP_PUBLIC_URL=https://jefferson-felizardo.tailcac351.ts.net uv run python -m app.main  # prints QR; :8765
cd frontend && npm run dev -- --host          # :5173, allowedHosts .ts.net
sudo tailscale serve --bg 5173                # HTTPS on the tailnet (cert already provisioned)
# (firewall ufw 5173 already open, persists). Phone: scan QR / open https://<you>.ts.net/?token=<tok>

# Work order:
# 0. PUSH the redesign commits (0dcb0ff..HEAD) — only Plan 3 (75fcc92) is on the remote.
# 1. Browser/phone-verify Phase 3 (scanner+sessions) and the model/effort "Aplicar nesta sessao".
# 2. Slice 1B: source ctx/tokens/model from transcript message.usage (extend transcript.py
#    parse_line to carry usage+model on assistant ChatEvents; LiveMetrics/Ring read it). VERIFIED the JSONL has usage.
# 3. Phase 4: NavBar 5h/7d chips + UsageSheet (resets, raw fallback) + full motion grammar (reduced-motion).
# 4. New backlog (docs/future-features.md): running agents/workflows panel; image/file attachments (audio later).
# resume: /handoff resume  (after git pull)
```
