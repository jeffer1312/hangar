# Task 1 — Spike results

Environment: tmux 3.6b · Claude Code v2.1.191 · Opus 4.8 (1M) · login claude.ai (Max).

## Assumption A — `tmux send-keys` submits a prompt: VALIDATED ✅

`tmux send-keys -t spike -l -- '<text>'` then `tmux send-keys -t spike Enter` submitted
the prompt to the live `claude` TUI. Response `● PONG` appeared in the pane and the
assistant `text:"PONG"` event was written to the session JSONL. The two-call literal+Enter
pattern works.

## Spinner format — assumption was WRONG, corrected ✅

The plan originally guessed the working marker was `esc to interrupt`. The REAL live spinner
line is: **a spinner glyph + a gerund word + `…`** (or `for <N>s`). Observed:

- `✽ Elucidating…`
- `· Elucidating…`
- `✻ Baked for 6s`
- `✻ Crunched for 8s`
- `✻ Cogitated for 8s`

The gerund word is random and changes constantly (do NOT match on the word). The animated
glyph cycles through `✻ ✽ ✶ ✺ ✢ · ∗` (and similar). `esc to interrupt` did NOT appear in
these captures. **Detection signal:** a stripped line whose first char is a spinner glyph
followed by a space. The assistant-message bullet `●` is NOT a spinner glyph — exclude it.

**User request:** surface this live text ("Elucidating…") as the state label, so the phone
shows what Claude is actually showing — not a generic "thinking".

## Permission behavior — user runs BYPASS, no approvals wanted

This config auto-approves: in bypass mode AND even after cycling to normal mode,
`echo`, `date +%s`, and a `Write` all ran with **no approval box** ("Ran 1 shell command",
"Wrote 1 lines"). The user confirmed: **keep bypass permissions, never wants to approve.**

→ **Decision: DROP phone permission-approval from v1.** No `awaiting_approval` state, no
Sim/Não buttons, no `/approve` endpoint.

The per-tool approval box ("Do you want to proceed?") could not be triggered in this config.
For reference (captured the equivalent widget from the first-run trust prompt): the option
widget is `❯ N. Label` lines with `Enter to confirm · Esc to cancel`. If ever needed (v2),
"yes" = Enter (confirms the highlighted ❯), "no" = Esc.

## "Approve" reinterpreted

The user meant: when **Claude needs a response from him** (a question), not a permission
grant. Claude asking a question is plain assistant text in the JSONL → renders as a chat
bubble; the user answers in the composer (which sends `send-keys`). When Claude finishes and
waits, there is no spinner → state `idle` ("aguardando tua resposta"). No special widget
detection needed for v1.

## States (simplified)

`working` (label = live spinner text, e.g. "Elucidating…") · `idle` · `dead`.

## Other notes

- The input box may show **dimmed ghost suggestions** (Claude Code suggests follow-up
  commands as ghost text inside `❯ `). Harmless — content comes from JSONL, not the input line.
- JSONL has meta event types besides user/assistant: `mode`, `permission-mode`, `ai-title`,
  `system`. The parser correctly returns `None` for these.

## Fixtures captured

- `tests/fixtures/pane_idle.txt` — real idle pane (input ready, no spinner).
- `tests/fixtures/pane_thinking.txt` — real working pane (`✽ Elucidating…`).
- `tests/fixtures/jsonl_samples.jsonl` — real transcript tail (incl. assistant `PONG`, meta types).
