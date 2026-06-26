# Future features (backlog — requested 2026-06-25)

Next things to design + build after the current redesign phases. Mobile-first. The backend
drives the live claude via tmux send-keys + reads the JSONL transcript + capture-pane.

## 1. See running agents (subagents + workflows)
A way to view, from the phone, what's executing inside the live claude session: the
running **Agent(...)** subagents and **Workflow** runs (mirrors what the terminal shows —
`Agent(...) Running…`, `+N tool uses`, `ctrl+b to background`). 
- Source: the JSONL transcript already records subagent activity (tool_use entries / agent
  spawns); workflow progress + subagent transcripts live under the session's
  `.../subagents/workflows/<runId>/` (journal.jsonl, agent-*.jsonl). Parse those for a live
  "Agents" panel (name, phase, state, tokens, elapsed). 
- UI: a panel/sheet listing active agents/workflows with live status; tap to see detail.
- **TodoWrite task list** (same panel): Claude Code renders a "N tasks (M done)" block with
  ✔/◻ rows; surface it in the app too. Cleanest source = the **TodoWrite tool_use entries in
  the transcript** (structured: subject + status), not the pane. Render a tasks panel with
  progress. Batched here with agents/workflows (same "ambient activity" surface).
- Open question: how much is reliably parseable from the transcript vs the workflow files;
  whether to show tool-use stream inline in the chat as collapsible cards.

## 2. Attachments — send + view images (audio later)
- **Send images** — ✅ DONE (2026-06-25): `POST /api/sessions/{name}/upload` (raw bytes) saves to
  `<cwd>/.claude-pocket-uploads/`; the composer has a 📎 picker + paste-into-textarea; on send it
  uploads and sends `"<caption>\n📎 imagem: <path>"` and the assistant reads the path. Lazy
  (upload-on-send, no orphan if cancelled).
- **CLEANUP / retention (deferred — needed):** uploaded images pile up in
  `.claude-pocket-uploads/` forever. Add a retention sweep — on backend startup (and/or periodic),
  delete files older than N days (e.g. 7d) or keep the last N. Simple, no extra endpoint. (User
  explicitly flagged this; deferred but must happen.)
- **View images** that appear in the chat: the transcript can carry image content blocks
  (user-attached or tool results); render them as inline image bubbles (currently only text).
  Also: persist/serve the uploaded images so they survive reload (v1 only shows the path marker).
- **Audio** — deferred (the user will tackle later): voice input/output.
- General **attachments** (files) — same upload-to-cwd + reference pattern as images.

## 3. Surface interactive prompts (AskUserQuestion / menus) in the app
The app already renders Claude Code's native selection menus: `state.py classify()` detects
`❯ N.` cursor + numbered options (`_CURSOR_RE`/`_OPTION_RE`) → `awaiting_input` → the app's
`OptionButtons`. So the capability EXISTS.
- Gap: the assistant's `AskUserQuestion` tool widget didn't show on the phone. Most likely it
  was just the **401** (app got no events). Possibly the classifier doesn't match its exact
  render (multi-select? different markers? the question/preview layout?).
- Plan: once auth is fixed, trigger an `AskUserQuestion` in a session the app is viewing and
  check if `OptionButtons` appear. If not, capture the pane and extend `classify()`/the option
  parser to recognize the widget (and carry multi-select + per-option descriptions).
- Why it matters: the user drives sessions from the phone; interactive prompts must be
  answerable there (see memory `claude-pocket-app-interaction`).

## 4. Pending fixes (batch — this session)
- **Cost chip → top row of the composer** (a thin row above the textarea), so the model pill
  gets more room in the control-left. Composer-only change.
- **401 self-heal:** today an invalid/rotated token leaves the app wedged — `isAuthenticated()`
  only checks that a token EXISTS, not that it's valid, so the app keeps 401ing and shows
  `undefined` session. On a 401 from the API, clear creds (`clearCredentials`) and bounce to
  Login so the user can re-pair (in-app QR scanner). Critical for REMOTE use (user can't clear
  site data from the phone easily). Lives in `lib/api.ts` (fetch wrapper) + the router.
- **Keyboard / top-bar bug (iOS):** when the keyboard opens, the NavBar (top bar) disappears
  and an accessory bar with a check shows above the keyboard. NEEDS a screenshot to pin down
  (user is remote → blocked until image-upload exists or user is at the PC). Likely the
  document-lock/`100dvh`/visualViewport-transform pushing the NavBar out of the visual viewport,
  plus iOS's native input-accessory bar.

> **Priority note:** image **upload** (item 2) is now higher priority — the user collaborates
> from the phone (away from the PC) and currently has NO way to send screenshots for debugging
> (e.g. the keyboard bug above). It unblocks the whole remote feedback loop.

## Notes
- These build on the existing infra: SSE stream, transcript parser, send-keys input, the
  HTTPS/secure-context (Tailscale), and the redesigned composer (a natural home for an
  attach button next to the slash/model controls).
- Prior backlog (UI polish, separate-statusline-from-badge, etc.) is largely addressed by
  the redesign; see docs/polish-backlog.md + docs/ui-redesign-proposal.md.
