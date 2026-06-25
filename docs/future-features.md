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
- Open question: how much is reliably parseable from the transcript vs the workflow files;
  whether to show tool-use stream inline in the chat as collapsible cards.

## 2. Attachments — send + view images (audio later)
- **View images** that appear in the chat: the transcript can carry image content blocks
  (user-attached or tool results); render them as inline image bubbles (currently only text).
- **Send images** from the phone to claude: pick/take a photo → deliver it to the live
  session. Mechanism is the open question — claude reads files by path, so the backend likely
  needs to save the upload to the session cwd (or a temp dir) and inject a reference
  (`send-keys` a path / an `@file`), OR use claude's image-paste path if drivable. Needs a
  backend upload endpoint (auth, size limit, allowed dir) + a frontend picker (camera /
  library, getUserMedia — secure context via Tailscale already in place).
- **Audio** — deferred (the user will tackle later): voice input/output.
- General **attachments** (files) — same upload-to-cwd + reference pattern as images.

## Notes
- These build on the existing infra: SSE stream, transcript parser, send-keys input, the
  HTTPS/secure-context (Tailscale), and the redesigned composer (a natural home for an
  attach button next to the slash/model controls).
- Prior backlog (UI polish, separate-statusline-from-badge, etc.) is largely addressed by
  the redesign; see docs/polish-backlog.md + docs/ui-redesign-proposal.md.
