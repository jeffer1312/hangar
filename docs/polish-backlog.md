# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

## Structural debt in the session list (2026-07-16)

> **Items 1–3 DONE (2026-07-17).** All three extractions shipped in full. Real numbers below.

Measured while building the kanban board. Nothing is broken — this is about the shape of
the code, and it already cost real bugs this session. In order of value per risk:

1. ✅ **DONE (2026-07-17) — Extract the multi-server SSE aggregation into a store.** The
   `slots`/`recompute`/`connect` trio now lives in `lib/sessions.ts` (pure dedup/order/classify,
   7 unit tests) + `lib/sessionsStore.svelte.ts` (a refcounted singleton: one `openSessionsStream`
   per server for the whole app, `retain`/`release` per consumer, Board's parse strategy — try/catch
   + `onServersChanged`). The three drifting copies (`Sidebar`, `SessionList`, `Board`) are gone;
   `Canvas` is a fourth consumer that reuses the same store instead of a fourth copy.

2. ✅ **DONE (2026-07-17) — `ConfirmDialog.svelte`.** Extracted as a chassis
   (`.confirm-backdrop`/`.confirm-card`/`.confirm-actions`); the two non-plain confirms (resume with
   a candidate list, add-server with an input) pass their body via a `{#snippet}` children slot, with
   that body's CSS kept in `Sidebar`. The shared `withServer` helper moved to `lib/auth.ts`.

3. ✅ **DONE (2026-07-17) — `SessionContextMenu.svelte`.** The row's context menu is now its own
   component, owning `menuMuted`/`branchView`/`chainView`; it also uses the shared `withServer`
   from `lib/auth.ts`.

Real result: `Sidebar.svelte` went from **1859 lines / 44 `$state`** to **1557 lines / 37 `$state`**
(the backlog's ~1100/~25 estimate was optimistic — the three items were done in full; the rest of
what remains is legitimate list template/CSS, not duplication).

**The bigger fish, still deliberately NOT done:** `Sidebar.svelte` (1859) and
`SessionList.svelte` (1423) are *the same feature written twice* — the session list, one for
desktop and one for mobile, 3282 lines combined. CLAUDE.md already flags the risk ("make the
change in BOTH views and verify BOTH — they drift apart easily"). Unifying them is the
largest win available, but it means rewriting both views, and they just absorbed a merge.
Worth its own session with the repo at rest.

Not worth touching: the kebab (30 lines), the hover preview (already a component), and
resize/collapse (5 states, cohesive with the sidebar chrome).

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
