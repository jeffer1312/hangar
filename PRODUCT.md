# claude-pocket — Product Context

**Register:** product (app UI — design serves the tool, not the marketing).

## Users
Single power user (developer) with **more than one machine**. Drives live Claude Code
sessions running in tmux on his own machines, over LAN/VPN. Not multi-tenant, not
public: LAN/VPN-only by design.

**Mobile and desktop are equally important surfaces.** Neither is primary. Some
features only make sense on one of them (touch sheets on mobile; keyboard shortcuts,
side panels, terminal on desktop) — that's fine, but both must work equally well.

## Product purpose
Read Claude Code's JSONL transcript and drive the session (send prompts, pick options,
answer AskUserQuestion, run git/workflows) from a phone or a desktop browser.

What "good" means for this product, in the user's own priorities:
- **Full control away from the PC** — nothing should force going back to the terminal.
- **Structured session organization** — easy switching between contexts *and between
  machines* (multi-PC is a real, daily scenario).
- **Speed and comfort** — fast to open, fast to act, comfortable to read long replies.
- **Surface what the terminal hides** — features/stats that are buried in the TUI
  (statistics made easy to understand, agent control) become first-class UI here.
- **A real work workflow** — not a remote viewer, but the place where work happens.

## Tone
Bonito, calmo, profissional, técnico, amigável, rápido. The UI stays out of the way and
surfaces state (working / idle / awaiting input) at a glance. Portuguese (pt-BR) UI
copy, terse and concrete. No marketing voice — this is a tool for one person who
already knows what it does.

**Reference:** Claude.ai is the feel reference the user likes (already followed:
neutral user bubble, chat rhythm). When in doubt about chat UX, look there first.

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
- Two first-class surfaces: a change on one must never degrade the other. Shared
  components gate desktop behavior behind `min-width: 820px` / the DesktopShell;
  mobile surfaces (SessionList, sheets) keep working untouched.
- Reduce trips to the terminal: when a Claude Code capability is hidden in the TUI,
  prefer exposing it in the app over documenting a terminal workaround.

## Accessibility
No specific declared needs. Defaults hold: AA contrast (≥4.5:1 body text — already
enforced in `app.css` tokens), global `prefers-reduced-motion` handling, touch targets
comfortable on mobile.
