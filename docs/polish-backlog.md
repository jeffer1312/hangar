# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

## From phone testing (2026-06-25)

- **Mobile UI needs real adjustment** (general). The current layout is a working first
  cut on a phone but not refined — spacing, touch targets, widths, scrolling. Do a proper
  design pass with the front-end skills on a running phone session.
- **Separate the raw statusline from the state badge.** Right now `StatusBar` combines the
  verbatim terminal statusline AND the working/idle "thinking" pill in one bottom bar. They
  are different concepts (Claude Code's own status text vs. our live state) — split them
  visually so the "thinking/Pronto" indicator reads clearly on its own.
- **Surface context usage clearly.** The context/token info (💬 in the statusline) is there
  but gets truncated / scrolled off on mobile — give context usage its own readable display
  instead of relying on the wide raw statusline.
- **Model switching from the phone** (feature). Can't change the Claude model / session
  controls from the web yet. Explore surfacing Claude Code session controls (model, etc.).

## Notes
- These are UX/feature items; the core loop (chat, live state, input, statusline) works.
- Plan 3 (deploy/onboarding: auto-detect IP, QR pairing, Caddy/TLS) is proceeding in
  parallel; see docs/onboarding-and-network.md.
