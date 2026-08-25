# hangar — Mobile Redesign Proposal (+ critique)

Produced by the deep+competitive UI/UX research workflow (9 agents, ~632k tokens),
then verified against the real code. Source decisions: docs/ui-redesign-requirements.md.

> Apply on implementation: ui-ux-pro-max + impeccable + design-taste-frontend + emil
> (workflow subagents can't invoke skills, so they ran with the principles embedded; the
> skills get applied in the main loop when building).

---

## PROPOSAL

The load-bearing move: **stop rendering `<pre>{status_line}</pre>`**. Parse the statusline
into typed fields once, then place each field where it belongs. Reuses every existing token
in `app.css`, the existing `sendInput`/`interrupt`/`createSession` plumbing. Backend adds one
parser, one fs-scan endpoint, one commands endpoint; everything else is frontend reshaping.

### 0. The parser (precondition)
Add `src/lib/statusline.ts` → `parseStatusLine(s)` → typed `StatusFields` (model, effort,
ctxUsed/ctxTotal/ctxPct, tokensTurn, elapsedMs, costUsd, fiveHourPct/weeklyPct + resets,
branch, raw). Defensive: normalize k/M/commas, clamp ctxPct 0–100, HOLD last good value on a
bad poll (never animate the ring to a bogus 100%), return null → indeterminate UI.
Extend `StateEvent` with `status` (parsed), `model`, `effort` (keep `status_line` raw as fallback).

### (a) Layout — 3 zones (account-level+slow = top bar; per-turn+actionable = composer; dated/precise/long = sheets)
```
NAVBAR  ‹  hangar ▾        5h 88%·s 38%   ← slim top bar
────────────────────────────────────────────
            MESSAGE LIST (unchanged)
────────────────────────────────────────────
◇ Pronto                 12.3k tok·1:20·$0.04   ← composer status row
┌────────────────────────────────────────┐
│ Mensagem para Claude…                   │     ← textarea
└────────────────────────────────────────┘
[◔38%] [Opus·med ▾] [ / ]            [ ▶ ]      ← composer control row
```
Field map: model+effort → composer pill; ctxPct → ContextRing; tokens/elapsed/cost → status
row (right, tabular-nums); 5h/weekly% → NavBar chips (tap → UsageSheet w/ resets); state+label
→ status row (left); raw → UsageSheet fallback. **StatusBar.svelte retired.**

### (b) Composer — one cohesive card (replaces StatusBar+Composer siblings)
- Zone 1 status row: state dot+label (left), `tok · mm:ss · $` mono (right). Context% NOT here (avoid Codex "two percentages").
- Zone 2 textarea: keep auto-grow, 16px, max 120px.
- Zone 3 control row (44px): ContextRing (22px+label, tap→popover) · model·effort pill (tap→ModelEffortSheet) · slash `[/]` button … send/STOP button (morphs; **deletes the big Interromper button**).
- ModelEffortSheet: model list + effort segmented (sticky across model switches). Select → `sendInput('/model sonnet')` / `sendInput('/effort high')`. ALWAYS full-arg form (bare `/model` opens a blind TUI).
- State machine idle/working/awaiting_input/dead drives row, textarea-enabled, send↔stop, pill, ring. Keep composer mounted during awaiting_input (free-text reply) + keep OptionButtons.

### (c) Sessions + project scanner
- Sessions home: sort by activity+urgency; SessionCard = basename title + muted cwd; pulse only when working; filter input when >6.
- In-chat switcher: NavBar title → `hangar ▾` → sheet of other live sessions + `+ Nova`.
- **Folder scanner** (core new feature): backend `GET /api/fs/scan?root&path` → subfolders [{name,path,is_git,has_claude_md,mtime}]; **allowlist of permitted roots in config.py**, realpath, reject `..`/symlink escape. Frontend: roots chips + search + single tappable drill-in column (row tap=select cwd, chevron=drill), skeleton/empty/denied states. Open-vs-New dedupe vs live tmux cwd. Feeds the same `createSession(name, cwd)`.
- Slash commands: `GET /api/sessions/{name}/commands` (built-ins JSON + cwd scan of `.claude/commands`, skills, plugins). Two surfaces: inline `/` strip above textarea + `[/]` CommandSheet (grouped, source badges). Zero-arg → send; valueSet → submenu; opensPicker → bare cmd → awaiting_input/OptionButtons. Flag destructive (`/clear`,`/compact`,`/quit`).

### (d) Motion grammar (each state = one motivated motion; `--ease-out`, <300ms except ring fill)
ring arc (600ms stroke-dashoffset, the one slow storytelling motion) · ring color crossfade accent→warning→error (threshold, the only color shift) · working verb (shimmer OR breathe) · token count-up (Tween 400ms) · elapsed mm:ss (setInterval, the honest liveness anchor) · send↔stop crossfade+`:active scale(0.97)` · working→idle spring settle · awaiting_input finite pulse · dead one-time desaturate. Reduced-motion: KEEP the mm:ss ticker + color states (global app.css zeroes durations → would erase all cues); shimmer→static fill; stop breathing→static outline.

### (e) Components + phased order
New: `statusline.ts`, `ContextRing`, `LiveMetrics`, `ModelEffortSheet`, `BottomSheet` (extracted), `CommandSheet`+`SlashSuggest`, `UsageSheet`, `FolderScanner`, `SessionSwitcherSheet`. Rebuilt: `Composer`, `Chat`, `NavBar`, `SessionCard`, `SessionList`, `CreateSessionSheet`. Retired: `StatusBar`. Backend: `parse_status_line`, `/api/fs/scan`+roots, `/api/sessions/{name}/commands`.

1. **Phase 1 — kill the broken statusline:** parser + rebuild Composer (status row + LiveMetrics + ContextRing + morphing send/stop), retire StatusBar/Interrupt. Fixes the headline mobile-break; delivers req 1,2,5. Independently shippable.
2. **Phase 2 — model+effort+slash:** BottomSheet, ModelEffortSheet, CommandSheet+SlashSuggest + commands endpoint (req 1,4).
3. **Phase 3 — sessions+scanner:** NavBar switcher, card/list reshape+filter, FolderScanner + /api/fs/scan + roots (req 3).
4. **Phase 4 — top bar + polish:** rate chips + UsageSheet, full motion grammar, optional swipe-to-switch.

---

## CRITIQUE (verified against the real code)

Structure is sound and reuses tokens/plumbing faithfully; the kill-`<pre>` move is correct;
Phase 1 is genuinely shippable alone. But it **overpromises the metrics it can read** and
**ignores the reliable structured source already in the repo**.

### P0 — most of req-2's metrics are NOT reliably in the statusline
`status_line()` returns the user's OWN custom statusline script output (verbatim). Per-turn
tokens/cost/rate-limits only exist if that script prints them (a default install does not).
**The statusline JSON `rate_limits` is the script's stdin, not readable from capture-pane.**
**Fix:** source metrics from the **transcript JSONL `message.usage`** (input/output/cache
tokens) + `message.model` — which `transcript.py` already reads and currently throws away.
Derive ctxUsed/ctxPct/tokensTurn/model from it; cost = usage × a static price map. Keep
statusline parsing only for what the user's line genuinely renders.
NOTE: THIS user's statusline IS rich (it shows 💬 ctx, 💵 cost, ⚡5h, 📅7d, model, effort) —
so parsing it works for them; the transcript path is the PORTABLE/robust source.

### Other findings
- **effort is write-only** — no read-back anywhere → the pill is optimistic local state (drifts if user runs /effort in terminal). Mark it as such.
- **elapsed timer** = client-local approximation (resets on the 3s SSE reconnect). Liveness cue, not authoritative time.
- **awaiting_input free-text is risky** — send_keys literal into an arrow-key TUI menu can mis-select. Default to OptionButtons; only enable free-text on prompts that accept it.
- **shimmer working-label is itself AI-slop** (gradient-text cliché, decorative). Cut it; keep the mm:ss ticker + breathing dot (two honest cues already).
- **"Auto-compactação em ~46%"** ring copy is invented — drop the number.
- fs-scan, commands endpoint, send/stop morph, open-vs-new dedupe — all feasible as described.

### Open decisions (user must choose)
1. **Cost source:** transcript usage × hardcoded price table (prices drift) OR drop cost unless the user's statusline prints it?
2. **Rate-limit windows (5h/weekly):** only from the user's custom statusline (chips conditional/often hidden) OR out-of-scope v1?
3. **Context total:** static per-model map keyed off transcript `message.model` for ctxPct?
4. **awaiting_input composer:** mount-but-restrict free-text (safer) vs always-on?
5. **fs-scan roots:** which absolute parent roots in the allowlist? editable in Settings or hardcoded v1?
6. **Slash built-ins:** maintained static JSON (drifts) vs scan-only?
7. **Phase 1 scope:** ship with metrics from transcript ONLY (model/context/tokens), defer cost+rate-limits?

**Net:** bones are good; the one structural correction that matters — **the transcript, not the
statusline, is the reliable metrics source** — re-anchor req 2 on it before building the ring/LiveMetrics.
