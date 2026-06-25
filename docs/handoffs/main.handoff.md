---
branch: main
saved_at: 2026-06-25T16:00:00-03:00
saved_commit: c6bc675a83f4838724e0b0ea178a4d4b2fca6065
plan: 2026-06-25-claude-pocket-backend.md
status: in_progress
---

## TL;DR
claude-pocket: drive a live Claude Code tmux session from the phone, LAN/VPN-only. Backend+frontend core was already done. THIS session: validated the full phone→claude loop in a real browser, fixed 6 bugs, then did Plan 3 (deploy/onboarding) — QR pairing + Tailscale HTTPS + in-app QR scanner — all verified, plus a firewall setup script. The phone can now scan a QR and land in the app over a trusted HTTPS cert from anywhere on the tailnet. Committed + pushed. Remaining: installable-PWA offline (serve the build, not vite dev) + a UI polish pass.

## Task atual
Plan 3 essentially done. Open: (1) serve the built PWA (dist) instead of vite dev so the service worker registers (real installable/offline PWA) — see polish-backlog; (2) the mobile UI polish pass (docs/polish-backlog.md). Then iterate.

## Concluído nesta sessão
- Live e2e in a real browser (agent-browser) on a real claude tmux session: login, history, live SSE, working/idle state, send-from-composer→reply, interrupt, statusline, command-meta filtered.
- 6 bugs fixed (committed c6bc675): projects_dir→CLAUDE_CONFIG_DIR, SSE→model_dump_json, cookie auth_token→cp_token, classify temporal live-vs-frozen spinner, dedupe-by-id live append, command-meta filter. + raw statusline surfaced to a bottom status bar; loopback bind default; vite /api proxy.
- Plan 3 (this batch, being committed now): CP_LAN_BIND_IP=auto + detect_lan_ip; QR pairing (backend prints a QR of the PWA URL+token at startup; frontend auto-logs-in from ?token= and strips it); Tailscale HTTPS via `tailscale serve`; in-app QR scanner (qr-scanner) on the Login screen; scripts/lan-setup.sh (firewall) + scripts/show-qr.sh.
- Verified e2e: QR → auto-login over Tailscale HTTPS (trusted cert, secure context). 57 backend tests, frontend build clean.

## Decisões
- Onboarding = QR pairing (QR encodes PWA URL + token). CP_PUBLIC_URL overrides the QR base (set to the Tailscale https URL).
- TLS = Tailscale (`tailscale serve` → vite:5173). Chosen over Caddy/self-signed: trusted cert on the iPhone with zero manual trust, works anywhere on the tailnet. (User enabled HTTPS in the tailnet admin console — one-time.)
- iOS reality: installed standalone PWA has SEPARATE storage from Safari, and iOS won't deep-link an https URL into an installed PWA. So the installed app must be paired ONCE from inside it → that's why the in-app QR scanner exists.
- Dev auth = vite proxy same-origin + cp_token cookie. Creds are per-ORIGIN — use ONE canonical URL (the ts.net one), not the LAN IP, to avoid "saved under another URL" confusion.

## Limitações conhecidas
- Installable/offline PWA: the service worker does NOT register under vite dev (verified SW count=0). Add-to-Home-Screen still works (manifest + apple meta tags present), but offline needs serving the built dist (e.g. tailscale serve a static server, or Caddy). Not done.
- UI needs a real mobile polish pass; also: separate the statusline from the state pill; surface context usage better; model switching. See docs/polish-backlog.md.
- iOS PWA install = Safari only (Apple). Chrome on iOS can't install standalone.
- #5 (minor): GET /api/sessions always state="idle" (real state only via SSE).
- Run recipe is manual (env + tailscale serve + vite --host + firewall). No single start script yet.

## Erros / armadilhas
- CLAUDE_CONFIG_DIR=/home/.../.claude-work on this machine → transcripts under ~/.claude-work/projects.
- Start claude as the DIRECT tmux pane command (not via interactive shell — p10k wizard eats keys). Fresh cwd → trust + external-imports prompts; Enter to accept.
- Foreground `sleep` is blocked by the harness.
- backend stdout is block-buffered when redirected to a file → the startup QR only shows with flush=True (done) or in a real tty.
- Tailscale: HTTPS must be enabled in the admin console (DNS → HTTPS) — one-time, no CLI. `tailscale serve`/`cert` need root or `tailscale set --operator=$USER`.

## Arquivos criticos
- backend/app/config.py (R) — detect_lan_ip, resolve_bind_ip, pairing_url, front_port, public_url, CLAUDE_CONFIG_DIR dir, loopback bind.
- backend/app/main.py (R) — print_pairing (QR) + resolve_bind_ip on startup.
- backend/app/state.py (R) — temporal spinner detection + status_line(). backend/app/transcript.py (R) — command-meta filter.
- frontend/src/screens/Login.svelte (R) — ?token= auto-login + "Escanear QR". frontend/src/components/QrScanner.svelte (N) — camera QR scan (qr-scanner).
- frontend/src/screens/Chat.svelte (R) — dedupe + bottom dock. frontend/src/components/StatusBar.svelte (N).
- scripts/lan-setup.sh (N) firewall; scripts/show-qr.sh (N) print QR on demand.
- docs/onboarding-and-network.md (N), docs/polish-backlog.md (N).

## Próximo passo
```
# Bring the stack up (one terminal each), then scan the QR on the phone:
cd backend && CP_AUTH_TOKEN=<tok> CP_PUBLIC_URL=https://<you>.ts.net uv run python -m app.main  # prints QR
cd frontend && npm run dev -- --host          # vite on LAN (allowedHosts .ts.net)
sudo tailscale serve --bg 5173                # HTTPS on the tailnet
sudo ./scripts/lan-setup.sh 5173              # open the firewall (one-time)
# Phone (Tailscale online): scan the QR (or ./scripts/show-qr.sh) -> auto-login.

# Remaining work:
# 1. Serve the BUILT pwa (npm run build -> dist) so the service worker registers
#    (installable/offline). E.g. tailscale serve a static server of dist + /api proxy.
# 2. Mobile UI polish pass (docs/polish-backlog.md): separate statusline vs state pill,
#    context usage, model switch, general layout.
# resume: /handoff resume  (after git pull)
```
