# Native AskUserQuestion stepper (multi-question) in the webui

**Date:** 2026-06-27
**Status:** approved (design), pending implementation

## Problem

When Claude Code calls the `AskUserQuestion` tool with multiple questions, the TUI renders
a tabbed multi-question prompt (a tab per question + a Submit tab, navigated with
arrow keys). In the phone app today the only way to answer is the **raw TUI mirror** —
driving the terminal with on-screen arrow keys (`←/→/↑/↓/Enter`). That is slow and
error-prone on a phone.

We want a **native, step-by-step** UI: render each question as a step with tappable
options, then a review screen, then submit — and drive the underlying TUI selection
automatically.

## Goals

- Detect the multi-question `AskUserQuestion` prompt and render it natively, by steps.
- Support single-select and multi-select questions, plus the built-in escape hatches
  ("Type something" free text, "Chat about this").
- Review screen before submitting (user confirms all picks).
- Drive the TUI selection safely — **never submit a wrong answer**: verify against the
  TUI's own review screen before the irreversible Submit; on any mismatch, fall back to
  the TUI mirror.

## Non-goals

- Replacing the existing single-list option menu (`OptionButtons`) — that already works
  and stays as-is. This is only for the multi-question tabbed `AskUserQuestion`.
- Handling AskUserQuestion variants we have not observed (the design is anchored to the
  observed Claude Code v2.1.x rendering; the verify-before-submit guard protects against
  drift).

## Investigation findings (the driving model — reference for implementation)

Observed empirically against Claude Code v2.1.195 by driving a throwaway prompt and
capturing the pane after each key (a ~0.3s settle is required after each key before the
TUI redraw is readable).

**Structured payload is in the transcript jsonl.** The assistant message contains a
`tool_use` block `{type:'tool_use', name:'AskUserQuestion', input:{questions:[...]}}`
where each question is:
```
{ question: str, header: str, multiSelect: bool,
  options: [{ label: str, description: str }] }
```
`header` is the tab label. The escape hatches ("Type something", "Chat about this") are
NOT in the payload — the TUI adds them as the last options of each tab.

**TUI layout:**
```
←  ☐ Cor  ☐ Fruta  ✔ Submit  →        tab bar: ☐ unanswered, ☒ answered, ✔ = Submit tab
Escolha uma cor                        question text of the active tab
❯ 1. A                                 single-select: ❯ cursor on an option
  2. B
  3. C
  4. Type something.
  5. Chat about this
Enter to select · Tab/Arrow keys to navigate · Esc to cancel
```

**Key behavior (confirmed):**
- `↓`/`↑` move the `❯` cursor within the active tab's options.
- **single-select:** `Enter` on an option selects it, marks that tab `☒`, and
  **auto-advances to the next tab**.
- **multi-select:** options render as `[ ]` / `[✔]`. `Space` toggles the checkbox under
  the cursor (cursor stays put). Tab becomes `☒` once ≥1 is checked. `→`/Tab advances to
  the next tab (no auto-advance on Space).
- **Submit:** `→` after the last tab opens a **Review screen**:
  ```
  Review your answers
   ● Escolha uma cor
     → A
   ● Escolha frutas
     → X, Y
  Ready to submit your answers?
  ❯ 1. Submit answers
    2. Cancel
  ```
  `Enter` on "Submit answers" submits; "2. Cancel" (or `Esc`) cancels.
- **Decline risk:** a wrong key (e.g. `Enter` on the wrong option, or `Esc`) can decline
  the entire prompt ("User declined to answer questions"). This is why driving must verify
  before the final Submit.

The Review screen is the key enabler: it lists every selection (`→ answer`), so verifying
the driven state against the user's intent is a simple parse-and-compare.

## Design

### 1. Detection + payload (backend)

- In the state/transcript layer, when the session is `awaiting_input` AND the latest
  assistant `tool_use` in the jsonl is `AskUserQuestion`, parse its `input.questions`.
- Emit a new SSE event `ask_question` carrying the structured questions:
  ```
  { questions: [{ header, question, multiSelect, options: [{label, description}] }] }
  ```
- The simple single-list menu path (`awaiting_input` + `options`) is unchanged; the new
  event only fires for the multi-question tool.

### 2. Render — native stepper (frontend)

- New component `AskQuestionSheet.svelte`, auto-opened on `ask_question`.
- One step per question: `header` as the step title, `question` text, each option a
  tappable button showing `label` + `description`.
  - **single-select:** tapping an option advances to the next step.
  - **multi-select:** options are toggle checkboxes + a "Próximo" button to advance.
- Each step also offers: **"✎ Digitar resposta"** (free text input → maps to the TUI's
  "Type something") and **"💬 Conversar sobre isso"** (→ "Chat about this").
- Final step: **review screen** listing each question → chosen answer(s), with
  **Enviar** and **Cancelar**.
- The TUI mirror remains reachable as a manual fallback button.

### 3. Drive + verify (backend, approach C — hybrid)

- New endpoint `POST /api/sessions/{name}/answer`, body:
  ```
  { answers: [ {kind:'option', indices:[int,...]}
             | {kind:'text', value:str}
             | {kind:'chat'} ] }   // one entry per question, in order
  ```
- Driving assumes the **known initial state** (auto-open means the user never touched the
  TUI → cursor at tab 1, option 1). For each question in order:
  - **option, single:** `↓` × `indices[0]`, then `Enter` (auto-advances).
  - **option, multi:** cursor starts at option 0; for each target index (ascending),
    `↓` to it (track cursor position) + `Space`; after the last, `→` to advance.
  - **text:** `↓` to the "Type something" option, `Enter`, type the value (no control
    chars), `Enter`.
  - **chat:** `↓` to "Chat about this", `Enter`.
- After all questions, land on the **Review screen**. `capture-pane`, parse the
  `● question → answer` lines, and compare to the requested answers.
  - **match:** `Enter` on "Submit answers" → return `{ok:true}`.
  - **mismatch / parse failure / unexpected screen:** `Esc` (cancel, do NOT submit) and
    return `HTTPException(409)` so the frontend opens the TUI mirror.
- A ~0.3s settle after each key before reading (the render race is real).
- `ponytail:` drives from the assumed initial cursor; the Review-screen verify is the
  safety net that makes the assumption safe (wrong drive is caught and aborted, never
  submitted).

### 4. Fallback

Any desync, verify mismatch, parse failure, or driving error → the answer is NOT
submitted; the frontend surfaces the TUI mirror so the user can finish manually. The
mirror path is unchanged and always available.

## Validation / security

- The `answer` endpoint is auth-gated like the other session routes.
- `text` values are validated to reject control chars (same rule as `send_prompt`), so a
  free-text answer can't inject TUI actions.
- Indices are bounds-checked against the question's option count.

## Testing

- **Backend detection:** a jsonl fixture with an `AskUserQuestion` tool_use → parser
  yields the structured questions; a non-AskUserQuestion awaiting_input → no `ask_question`
  event.
- **Backend driving (unit):** given answers, assert the exact key sequence sent (mock the
  key sender), for single, multi, text, and chat. Assert the verify path: a mocked Review
  capture that matches → submit; one that mismatches → Esc + 409.
- **Frontend:** stepper renders from a payload; single advances on tap, multi toggles +
  "Próximo"; review lists picks; Enviar calls `/answer`; a 409 opens the mirror.

## Files touched

- `backend/app/state.py` (or the transcript layer) — detect AskUserQuestion + structured
  parse.
- `backend/app/models.py` — `AskQuestion` payload model + the `ask_question` event.
- `backend/app/sse.py` — emit the `ask_question` event.
- `backend/app/terminal_input.py` — the driving routine (key macro per answer + the
  Review-screen verify).
- `backend/app/api.py` — `POST /api/sessions/{name}/answer` + body model.
- `frontend/src/lib/types.ts` + `api.ts` — types + `answerQuestions()`.
- `frontend/src/components/AskQuestionSheet.svelte` (new) + wiring in `Chat.svelte`.
- Tests across the above.
