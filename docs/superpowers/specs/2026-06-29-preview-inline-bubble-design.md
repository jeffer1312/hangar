# Preview as inline assistant bubble — design

**Date:** 2026-06-29
**Status:** approved, pending implementation plan

## Problem

The live preview of an in-flight assistant block (`preview` SSE event → `previewText` in
`Chat.svelte`) is rendered in `MessageList.svelte` as a separate fixed-height box
(`.preview-bubble`) whose tail scrolls *inside* itself. That box was built to avoid layout jump,
but it reads as a distinct widget bolted to the end of the chat rather than the assistant message
streaming into the conversation the way Claude Code / claude.ai show it.

Goal: render the in-flight preview as a normal-looking assistant bubble that grows inline in the
chat flow, then hand off seamlessly to the canonical JSONL-backed bubble when the block commits —
so the user perceives one continuous streaming message, not a preview box that vanishes.

Out of scope: the source of the preview text (still `capture-pane` polling in `preview.py`,
unchanged — there is no clean live markdown source; established earlier). The session-list polling
→ push redesign is a **separate** feature (its own spec). The existing preview↔JSONL reconciliation
and the working-state `Spinner` are **kept as-is**.

## Why plain text for the preview (kept)

The preview comes from the already-rendered tmux pane: markdown **syntax is gone** (Claude's TUI
converted it to ANSI, which `capture-pane -p` then strips). So the preview is plain text. Rendering
it through the markdown pipeline would do nothing useful and risks flickering half-open `**` /
code-fences mid-stream. The preview therefore stays **plain**; only the committed JSONL bubble is
markdown-rendered. This is the existing behavior and the reason for it — preserved.

## Design

### 1. `AssistantBubble.svelte` — add a `preview` mode

Add a `preview?: boolean` prop (default `false`).

- `preview = false` (default): **unchanged** — current markdown rendering, timestamp, etc.
- `preview = true`: render `text` as **plain text** (skip the markdown renderer), show the
  streaming **caret**, hide the timestamp. Reuse the exact same bubble shell (container, background,
  padding, max-width, alignment) so a preview bubble is visually indistinguishable from a committed
  one except for plain-vs-markdown body.

The default path must not regress: committed assistant messages render exactly as today.

### 2. `MessageList.svelte` — render the preview as that bubble

Replace the `{#if preview}` `.preview-bubble` `<div>` (and its inner caret span) with:

```svelte
{#if preview}
  <AssistantBubble text={preview} ts={undefined} preview />
{/if}
```

Remove the now-unused `.preview-bubble` fixed-height styles and the standalone caret markup/styles
(the caret moves into `AssistantBubble`'s preview mode). The existing rAF-coalesced autoscroll
effect (which already reads `void preview`) keeps the view pinned to the bottom as the bubble grows
— now the bubble grows the layout instead of scrolling inside a contained box, which is the desired
streaming feel.

The `Spinner` (working-state loading indicator) that renders just after the preview is **unchanged**
— it remains the "generating" signal the user wants to keep.

### 3. Seamless swap + crossfade

When the block commits, `Chat.svelte`'s reconciliation already sets `previewText = ''`, dropping the
preview bubble, and the canonical `AssistantBubble` (markdown) for that block appears in the same
slot. No change to reconciliation/dedup (delicate — left untouched).

- **Plain-prose answers:** the plain preview ≈ the rendered markdown → the swap is already
  effectively invisible. This is the core deliverable and works with no crossfade.
- **Markdown-heavy answers:** the body reformats (plain → formatted) at the swap — a one-time
  soft "pop". A short opacity crossfade on the just-committed bubble softens it.

The crossfade is **secondary polish**, implemented as a distinct step after the core inline bubble.
If it proves fiddly to target only the just-committed bubble (without fading every history message
on load), fall back to an instant swap — acceptable, since the pop only affects formatted answers
and is brief.

## Data flow

Unchanged: `preview` SSE event → `previewText` (`Chat.svelte`) → `preview` prop (`MessageList`) →
`AssistantBubble`. Only the **rendering** of `previewText` changes.

## Risks

- The `preview` mode must not alter committed-message rendering (default branch untouched).
- Autoscroll must keep the view pinned as the bubble grows the layout (previously the contained box
  absorbed growth). The existing rAF effect already depends on `preview`; verify stickiness.
- No change to the preview↔JSONL reconciliation / windowing / dedup (CLAUDE.md flags these as
  delicate).

## Testing / verification

No frontend test runner (gate is `npm --prefix frontend run check`; backend has pytest only). Verify:

- `check` passes with no new errors/warnings.
- Manual: during a working turn, the preview grows as a normal-looking assistant bubble inline (not
  a separate fixed box); on commit it becomes the markdown bubble in place — seamless for plain
  prose, a soft swap for markdown. The working `Spinner` still shows. Works at mobile (<820px) and
  desktop (≥820px, in `DesktopShell`'s reused `Chat`).

## Files

- `frontend/src/components/AssistantBubble.svelte` — add `preview` prop + plain/caret mode.
- `frontend/src/components/MessageList.svelte` — render preview via `AssistantBubble`; remove the
  `.preview-bubble` box + standalone caret; (step 2) crossfade on swap.
