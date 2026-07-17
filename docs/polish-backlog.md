# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

## Structural debt in the session list (2026-07-16)

Measured while building the kanban board. Nothing is broken — this is about the shape of
the code, and it already cost real bugs this session. In order of value per risk:

1. **Extract the multi-server SSE aggregation into a store** (`sessionsStore.svelte.ts`).
   The `slots`/`recompute`/`connect` trio (one `openSessionsStream` per server, dedup by
   `jsonl::name`, isolate offline servers) now exists in **three copies**: `Sidebar.svelte`,
   `SessionList.svelte` and `Board.svelte` — the third one because the board's own plan said
   "copy the pattern from the Sidebar". This is the only item that fixes an *active* problem
   rather than just reducing size: the copies already drifted twice (the ordering that lived
   only in `SessionList`, and `sortSessions`, which had to be extracted to `lib/format.ts`
   mid-feature). ~60 lines per consumer.

2. **`ConfirmDialog.svelte`** — `Sidebar.svelte` has **7** dialogs sharing one structure
   (`.confirm-backdrop` + `.confirm-card` + `.confirm-actions`): drop server, delete session,
   switch branch, log out, add server, resume conversation. Seven existing uses is
   duplication, not speculation. Frees ~150 lines of template + ~35 of CSS. Caveat: two of
   them aren't plain confirms (resume carries a candidate list, add-server carries an input)
   — they need a `{#snippet}` for the body, or stay out.

3. **`SessionContextMenu.svelte`** — the row's context menu (~78 lines) owns 4 states of its
   own (`menu`, `menuMsg`, `menuMuted`, `branchView`) and is self-contained.

Doing 1–3 takes `Sidebar.svelte` from **1859 lines / 44 `$state`** to roughly 1100 / ~25 —
each remaining state about the one subject left (the list and its chrome).

**The bigger fish, deliberately NOT recommended yet:** `Sidebar.svelte` (1859) and
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
