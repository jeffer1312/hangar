# Onboarding & Networking — decisions (Plan 3)

Decisions made on how a phone reaches and authenticates to claude-pocket. These
are the **target for the deploy/onboarding phase (Plan 3)** — not yet implemented.
Recorded here so the live-e2e session's conclusions survive.

## Access model

- Personal, single-user, LAN-only tool. It runs `claude` **as you** (bypass), so an
  exposed instance is effectively remote-command-execution-as-you. The protection bar
  is a **bearer token** (+ TLS later). For a home LAN that is deemed *sufficient* — no
  user/password database.
- "External" here means **another device on your own LAN** (your phone on the same
  Wi-Fi). That is the supported case.
- True remote (cellular / away from home) = **VPN back into the LAN** (Tailscale /
  WireGuard). **Never** port-forward to the public internet.

## Binding

- For the phone to reach it, the server must listen on the machine's **LAN IP**.
  Loopback is unreachable from other devices — this is a networking fact, not a
  security toggle.
- Default bind stays `127.0.0.1` (a bare run / the tests are never exposed by accident).
- **Phone mode = `CP_LAN_BIND_IP=auto`** → backend auto-detects the primary LAN IP
  (UDP "connect" to 8.8.8.8, no traffic sent) and binds it. An explicit IP still
  overrides. `0.0.0.0` covers multi-interface / DHCP churn (broader exposure).
- Keep `startup_guard`: refuse a non-loopback bind while the token is still the default
  `change-me` (stops exposing with no password). With QR there is always a real token,
  so the user never hits this.

## Auth / pairing UX

- Keep the **bearer token** — it is the password. No separate login/user-pass: that is
  the same shared-secret with more moving parts and no gain for a single-user tool.
- **QR pairing (target UX):** on startup in phone mode, the backend builds
  `http://<lan-ip>:<port>` + token and renders a **QR** (terminal, and maybe a `/pair`
  page). Phone scans → URL + token auto-filled → connected. "Scan = you're in." The
  token is never typed.
- The app already persists creds per device (localStorage + `cp_token` cookie). Generate
  the token once and persist it server-side so the QR is stable across restarts.
- Optional niceties: mDNS hostname (`pocket.local`) instead of a raw IP; relabel the
  "Token" field as "Senha".

## UX frictions found during the dev e2e (fix in polish)

- Dev currently requires leaving **"URL do servidor" empty** (vite proxy → same-origin);
  not obvious to a user. QR / auto-config removes this entirely.
- The Login placeholder shows port `8000`; the backend is `8765`. Fix the placeholder.
