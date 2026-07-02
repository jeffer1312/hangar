# claude-pocket — Product Context

**Register:** product (app UI — design serves the tool, not the marketing).

## Users
Single power user (developer). Drives a live Claude Code session running in tmux on
their own machine, from a phone over LAN/VPN, as a mobile chat. Also used on desktop
(persistent sidebar layout). Not multi-tenant, not public: LAN/VPN-only by design.

## Product purpose
Read Claude Code's JSONL transcript and drive the session (send prompts, pick options,
answer AskUserQuestion, run git/workflows) from a phone or a desktop browser. The phone
is the primary surface; desktop is a wider, keyboard-capable second surface that should
feel native to the desktop, not a stretched phone.

## Tone
Calm, fast, technical. The UI stays out of the way and surfaces state (working / idle /
awaiting input) at a glance. Portuguese (pt-BR) UI copy, terse and concrete. No marketing
voice — this is a tool for one person who already knows what it does.

## Anti-references
- No SaaS hero-metric dashboards, no gradient text, no glassmorphism-for-decoration
  (glass is used only on navbar/composer/sidebar chrome, purposeful).
- No "stretched phone app" on desktop: desktop must earn its width (keyboard shortcuts,
  breadcrumb, status strip, side panels instead of mobile sheets).
- No colored side-stripe borders on cards/rows.

## Strategic principles
- Never scrape the terminal for chat content; the transcript is the source of truth. The
  tmux pane is only peeked for live *state*.
- State legibility first: the user must always know if a session is working, idle, or
  waiting on them, without reading text.
- Mobile stays untouched when adding desktop affordances (shared components gate desktop
  behavior behind `min-width: 820px` / the DesktopShell).
