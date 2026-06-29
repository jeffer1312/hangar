# Sidebar multi-server (desktop) — design

**Date:** 2026-06-29
**Status:** approved, pending implementation plan

## Problem

The desktop layout (`DesktopShell`, ≥820px) reuses the mobile `Chat.svelte` unchanged, so the
chat area is at parity with mobile. The **session list** is not: desktop uses `Sidebar.svelte`,
mobile uses `SessionList.svelte` + `SessionCard.svelte`. Recent mobile work never reached the
sidebar. Concretely the sidebar lacks:

1. **Multi-server unified list** — `Sidebar` calls `getSessions()` (active server only); the user
   switches the active server to see another's sessions. Mobile `SessionList` aggregates **all**
   servers at once, with incremental per-server render and per-session origin.
2. **Mobile visual cues** — animated Lottie "pensando" working indicator, compact state chip,
   `cwd` line. Sidebar shows only a pulsing CSS dot + short id.

Goal: bring the sidebar to **feature parity** (multi-server + incremental) and adopt the
**applicable mobile polish** in the sidebar idiom (narrow 270px collapsible column).

Out of scope: swipe-to-delete (mobile gesture; desktop keeps hover `×`). Glass is already present
on the sidebar (`--glass-bg-solid` + liquid-glass on Chromium) — no change.

## Design

### 1. Data layer — aggregate all servers, incrementally

Replace the single-server `getSessions()` load with multi-server aggregation, reusing the exact
incremental pattern already shipped in `SessionList.svelte`:

- Fire `fetchSessionsForServer(srv)` for every configured server in parallel (each already has a
  4s timeout). Each resolution updates a per-server slot and triggers a recompute — a slow/offline
  server never stalls the others.
- A `loadGen` counter guards against a stale poll's late response overwriting a newer poll.
- Output is `groups: { server, sessions, error }[]` in configured-server order (not a flat list).
- **Dedup** globally by `${jsonl ?? cwd}::${name}`, first server wins — so the same backend reached
  via two server URLs does not list a session twice.
- Poll every 5s (unchanged).
- An offline/errored server keeps its group with an `error` marker (header shows offline), the
  rest render normally.

### 2. Layout — grouped by server

Each server renders a **group**: a header row (colored origin dot + server label; offline note if
errored) followed by its session rows. Chosen over a flat list because origin is obvious at a
glance for the typical 2–3 servers without repeating the label on every row.

Each **session row** adopts the mobile cues in the sidebar's dense idiom:

- **Lottie "pensando" indicator** (reuse `SessionCard`'s logic): animated when `working`, static
  frame 0 otherwise, color tint by state. Replaces the current CSS pulse `.dot`.
- Name + a **cwd line** (truncatable prefix + never-shrinking basename, same split as `SessionCard`).
- A **compact state chip** (`em execução` / `pronto` / `aguardando` / `encerrado`), smaller than the
  mobile chip to fit 270px.
- Keep existing sidebar behaviors: inline rename (long-press / `✎`), delete (`×` on hover),
  `sem id` badge + blocked open for untracked sessions.

**Collapsed sidebar (56px):** group headers and all text labels hide; only the per-session status
indicator shows (preserves current collapsed behavior).

### 3. Server switcher (footer)

Unchanged in markup, but its role narrows: it no longer drives which sessions are listed (the
grouped list shows all). It still selects the **active server** — the create target for "Nova
sessão" — and manages servers (rename / remove / add via QR). The active server's group may be
visually marked (e.g. its header dot filled), but ordering stays configured-server order.

### 4. Reuse

The Lottie working-indicator logic is identical in `SessionCard` and the new sidebar rows.
Optionally extract a tiny `StateIndicator.svelte` (`state`, `size` props) used by both. If the
extraction adds friction, replicate inline — the duplication is small. The full session-list
component is **not** shared between mobile and desktop (rejected: the two layouts — full-screen
cards vs narrow column — are genuinely different).

## Testing / verification

No frontend test runner exists (project gate is `npm run check`; backend has pytest only). Verify:

- `npm run check` passes (no new errors/warnings).
- Manual at ≥820px with 2 servers configured, one offline:
  - grouped list renders; the online server's group paints immediately, the offline one shows its
    error after ≤4s (incremental, not blocked).
  - working session shows the animated indicator; others static.
  - create (on active server), inline rename, delete, and collapse/expand all still work.
  - mobile (<820px) is unaffected (still `SessionList`).

## Files

- `frontend/src/components/Sidebar.svelte` — the change (load + template + styles).
- `frontend/src/lib/api.ts` — `fetchSessionsForServer` already exported; no change expected.
- Optional new: `frontend/src/components/StateIndicator.svelte`.
