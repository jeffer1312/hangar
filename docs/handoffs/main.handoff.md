---
branch: main
plan: superpowers/plans/2026-06-25-claude-pocket-backend.md
status: in_progress
saved_commit: 9c2237096a473841f65e84eb64f4b2fe8cc848e0
---

## TL;DR
claude-pocket: drive a live Claude Code tmux session from the phone, LAN-only. BACKEND 100% done (11 TDD tasks, 46 tests, 0 warnings, final-reviewed) and PUSHED public to github.com/jeffer1312/claude-pocket (branch main). Frontend = working FIRST CUT (Svelte+Vite PWA, builds clean) but NOT yet polished with the front-end skills. Next (other machine): design-polish w/ skills + network/TLS deploy + real end-to-end test on iPhone.

## Task atual
Backend SDD complete + published. Next phase = frontend design-polish pass USING the front-end skills (impeccable / frontend-design / taste-skill / ui-ux-pro-max) on a RUNNING app, then the network/deploy plan (Caddy+TLS+firewall+iPhone cert), then a live end-to-end test from the phone.

## Concluído nesta sessão
- Design: spec + plan written (docs/superpowers/{specs,plans}/2026-06-25-claude-pocket-*.md). Plan was EXECUTED.
- Spike (Task 1) validated 3 risky assumptions on a REAL claude/tmux session: send-keys submits; spinner = glyph+gerund (NOT "esc to interrupt"); interactive-question SELECT = (n-1)xDown+Enter. Fixtures in backend/tests/fixtures/.
- Backend: 11 tasks via TDD subagent-per-task, each independently reviewed + fixed. 46 tests, 0 warnings. Final whole-branch review caught+fixed 2 blockers (SSE pump-error swallow; default-token footgun).
- Frontend: first-cut Svelte+Vite PWA built in PARALLEL via a workflow (design->build->verify; builds clean, type-clean). Its RESULT is committed at frontend/.
- Published: scrubbed PII, unified git author, fixed httpx2 dep, gh repo create --public --push.
- Per-commit detail: git log --oneline 87d705e..9c22370 (27 commits).

## Decisões
- Architecture: chat CONTENT from the Claude JSONL transcript; live STATE from a narrow tmux capture-pane; INPUT via tmux send-keys. No terminal scraping.
- States = working(label)/idle/awaiting_input/dead. DROPPED permission-approval (user runs bypass, never approves) but KEPT interactive-question handling (ExitPlanMode/AskUserQuestion -> awaiting_input + option buttons -> POST /select).
- Frontend built via WORKFLOW with embedded design direction, NOT the front-end skills (workflow agents cannot invoke skills). It is a solid base to POLISH with skills, not to redo.
- httpx2: build agent hallucinated it as a runtime dep; investigated as a possible typosquat but it is LEGIT (real Pydantic pkg; the new starlette TestClient backend). Moved to dev-dep, dropped old httpx.
- Git author rewritten on all commits -> Jefferson Felizardo <jeffer1312@gmail.com> (local repo config only; global work identity untouched). Branch renamed master->main.

## Limitações conhecidas
- Frontend NOT yet polished with design skills (first cut only). Brief: docs/frontend-design-brief.md.
- Network/deploy (Plan 3) NOT done: no Caddy/TLS/firewall/iPhone cert. main.py startup_guard refuses a non-loopback bind while the token is the default 'change-me'.
- End-to-end NEVER run live (backend serving + frontend + real phone). Backend Task 11 Step 8 live smoke was intentionally skipped.
- TranscriptTailer startup race (initial read <-> awatch arming us gap) self-heals on next write; accepted for v1.
- Non-blocking Minors (final review): auth uses == not compare_digest; terminal_input ValueError -> HTTP 500 not 400; select() has no upper bound. All auth-gated / single-user LAN.
- SDD ledger/briefs/reports live in .superpowers/sdd/ which is GITIGNORED -> they do NOT travel via git. This handoff + git log are the record.

## Erros / armadilhas
- (resolved) httpx2 supply-chain scare; verified legit before re-adding as dev-dep.
- (resolved) PII: backend/tests/fixtures/pane_idle.txt had the user email captured from the live Claude screen -> scrubbed to dev@example.com before the public push.

## Arquivos criticos
- docs/superpowers/specs/2026-06-25-claude-pocket-design.md (N) - spec; read the "Revisao pos-spike" note at top.
- docs/superpowers/plans/2026-06-25-claude-pocket-backend.md (N) - the executed backend plan (read this to continue).
- backend/docs/spike-results.md (N) - REAL spike findings (spinner/widget markers, selection mechanic).
- docs/frontend-design-brief.md (N) - frontend design brief (basis for the polish pass).
- backend/app/sse.py backend/app/api.py backend/app/state.py backend/app/transcript.py backend/app/registry.py backend/app/terminal_input.py (N) - core backend.
- frontend/src/ (N) - Svelte app (screens/, lib/, components/) to be polished.

## Próximo passo
```
# On the OTHER machine:
git clone https://github.com/jeffer1312/claude-pocket && cd claude-pocket
cd backend && uv run pytest -q                 # expect 46 passed, 0 warnings
# To run it (needs tmux + claude): start claude in tmux first ->  tmux new -s cc  (run `claude` inside)
CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main
# frontend (new shell):
cd ../frontend && npm install && npm run dev
# THEN the design-polish pass: invoke the front-end skills (impeccable / frontend-design /
#   taste-skill / ui-ux-pro-max) on frontend/src WITH the app running in the browser.
# Resume this context:  /handoff resume   (after git pull)
```
