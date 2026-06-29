# Preview inline assistant bubble Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render the live in-flight preview as a normal-looking assistant bubble that grows inline in the chat, then hand off to the canonical JSONL bubble seamlessly — instead of the current separate fixed-height `.preview-bubble` box.

**Architecture:** Add a `preview` mode to the existing `AssistantBubble.svelte` (plain text + caret, no markdown, same bubble shell). `MessageList.svelte` renders the `preview` prop through that bubble instead of the `.preview-bubble` box. The preview↔JSONL reconciliation, the windowing/dedup, and the working `Spinner` are untouched. A second task makes the preview→committed swap seamless (suppress the entrance animation on the bubble that replaces a live preview + a short crossfade), with an explicit fallback to instant swap.

**Tech Stack:** Svelte 5 (runes), TypeScript, Vite. Existing `renderMarkdown` (`lib/markdown`).

## Global Constraints

- Frontend gate is `npm --prefix frontend run check`; baseline is exactly `1 ERRORS 7 WARNINGS` (one pre-existing `Lottie.svelte` "Cannot find module 'lottie-web'" error + 7 pre-existing warnings). No new errors/warnings. There is NO frontend test runner — do not add tests/TDD; each task's check is `check` + the listed manual verification.
- Preview text stays **plain** (no markdown) — it comes from the already-rendered pane (markdown syntax gone) and markdown-rendering it mid-stream would flicker half-open `**`/code-fences. Only the committed JSONL bubble is markdown-rendered.
- Do NOT change the preview↔JSONL reconciliation, windowing, or dedup logic in `Chat.svelte`/`MessageList.svelte` (flagged delicate). This feature only changes how `previewText` is rendered.
- The working-state `Spinner` that renders after the preview stays as-is (it is the "generating" affordance).
- Commit messages in English, no `Co-Authored-By` trailer.

---

## File Structure

- `frontend/src/components/AssistantBubble.svelte` — gains a `preview` mode (plain body + caret). Owns the caret markup/animation (moved in from MessageList).
- `frontend/src/components/MessageList.svelte` — renders the preview via `AssistantBubble`; loses the `.preview-bubble` box, its internal-scroll effect, and the standalone caret CSS. Task 2 adds the seamless-swap flag here.

---

## Task 1: Inline preview bubble (core)

Make the in-flight preview render as a normal-looking assistant bubble that grows inline, replacing the fixed-height `.preview-bubble` box.

**Files:**
- Modify: `frontend/src/components/AssistantBubble.svelte` (props, derived, template, styles)
- Modify: `frontend/src/components/MessageList.svelte` (preview render, remove box + internal-scroll effect + caret CSS)

**Interfaces:**
- Produces: `AssistantBubble` accepts `preview?: boolean` (default `false`). When `true`, renders `text` as plain text with a blinking caret, no markdown, no timestamp, no file/media refs.

- [ ] **Step 1: Add the `preview` prop + plain-render branch to `AssistantBubble.svelte`**

In `frontend/src/components/AssistantBubble.svelte`, change the `Props` interface and destructure (lines 6-11), and gate the derived work so preview mode does no markdown/ref parsing:

```svelte
  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
    preview?: boolean;
  }
  let { text, ts, sessionName = '', preview = false }: Props = $props();

  const html = $derived(preview ? '' : renderMarkdown(text));
  // Anexos por caminho citado na minha msg (img/video/html/pdf que eu "mandar").
  const fileRefs = $derived(!preview && sessionName ? parseFilePaths(text) : []);
  // Midia remota (URL http) -> preview inline; nao depende do backend/sessionName.
  const mediaRefs = $derived(preview ? [] : parseMediaUrls(text));
```

- [ ] **Step 2: Branch the template for preview mode**

Replace the body markup (lines 28-36) with a preview branch (plain text + caret) and the unchanged committed branch:

```svelte
<div class="assistant-msg">
  {#if preview}
    <!-- Preview ao vivo: texto PLANO (markdown so no snap final canonico, pra nao piscar **/code-fence
         meio-aberto) + caret. Mesma casca da bolha real -> swap quase invisivel. -->
    <div class="prose plain">{text}<span class="caret" aria-hidden="true"></span></div>
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="prose">{@html html}</div>
    {#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}
    {#if mediaRefs.length}<FileAttachment {sessionName} refs={mediaRefs} />{/if}
    {#if ts}
      <span class="ts">{formatTime(ts)}</span>
    {/if}
  {/if}
</div>
```

- [ ] **Step 3: Add the plain-body + caret styles to `AssistantBubble.svelte`**

In the `<style>` block, after the `.prose :global(...)` rules, add the plain-preview body rule and the caret (moved from `MessageList`):

```css
  /* Preview plano: preserva quebras de linha do pane (sem markdown -> sem blocos). */
  .prose.plain { white-space: pre-wrap; }

  /* Caret piscando no fim do preview ao vivo (familia Respiracao "Digitando"). */
  .caret {
    display: inline-block; width: 7px; height: 1.05em; vertical-align: -2px;
    margin-left: 2px; border-radius: 1px; background: var(--accent);
    animation: caret-blink 1s steps(1) infinite;
  }
  @keyframes caret-blink { 50% { opacity: 0; } }
```

- [ ] **Step 4: Render the preview via `AssistantBubble` in `MessageList.svelte`**

`AssistantBubble` is already imported (used for committed messages). Replace the `{#if preview}` box block (the `<div class="preview-bubble" bind:this={previewEl}>{preview}<span class="caret"...></span></div>`, around line 176-181) with:

```svelte
    {#if preview}
      <AssistantBubble text={preview} ts={undefined} preview />
    {/if}
```

- [ ] **Step 5: Remove the now-dead internal-scroll effect + `previewEl` binding in `MessageList.svelte`**

The old box scrolled its own tail via `previewEl`. With no internal box, the container-level rAF autoscroll (the effect that already reads `void preview`, ~line 110) keeps the view pinned as the bubble grows. Remove the `previewEl`-specific effect (the small `$effect` around lines 45-49 that does `if (previewEl) previewEl.scrollTop = previewEl.scrollHeight;`) and the `let previewEl ...` declaration (~line 29). Do NOT touch the main container autoscroll effect.

If `check` reports `previewEl` or any symbol as unused after this, delete that leftover too.

- [ ] **Step 6: Remove the dead `.preview-bubble` + `.caret` CSS from `MessageList.svelte`**

Delete the `.preview-bubble { ... }` rule, the `.preview-bubble::-webkit-scrollbar { ... }` rule, the `.caret { ... }` rule, and the `@keyframes caret-blink { ... }` (lines ~297-323) — the caret now lives in `AssistantBubble`. Leave all other styles (`.to-bottom`, etc.) intact.

- [ ] **Step 7: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: a line ending `1 ERRORS 7 WARNINGS` (baseline unchanged). If it rose, read the new problem and fix before committing.

- [ ] **Step 8: Manual verification**

Frontend dev server is live (systemd user service, HMR). During a working turn (send a prompt to a tracked session), confirm:
- The in-flight reply now grows as a **normal-looking assistant bubble** inline (full-width, same text style as a committed message), not a separate bordered/dimmed fixed box, with a blinking caret at the tail.
- The view stays pinned to the bottom as the bubble grows (autoscroll).
- The working `Spinner` still shows under it.
- On commit, the bubble becomes the markdown-rendered message (for plain prose, near-seamless; for markdown, a brief reformat — Task 2 softens this).
- Works at mobile width (<820px) and desktop (≥820px in `DesktopShell`'s `Chat`).

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/AssistantBubble.svelte frontend/src/components/MessageList.svelte
git commit -m "feat(frontend): render in-flight preview as an inline assistant bubble (replaces fixed preview box)"
```

---

## Task 2: Seamless preview→committed swap (entrance suppression + crossfade)

When the block commits, the canonical bubble currently plays the `msg-in` entrance (420ms spring slide), which reads as a jump right where the preview already sat. Suppress that entrance for the bubble that replaces a live preview and crossfade it in. **Fallback is explicitly allowed:** if reliably targeting only the just-committed bubble proves fiddly, leave the instant swap from Task 1 (the spec accepts it — the pop only affects markdown answers and is brief). Document the choice in the report.

**Files:**
- Modify: `frontend/src/components/AssistantBubble.svelte` (add `noEntrance` prop → swap the entrance animation)
- Modify: `frontend/src/components/MessageList.svelte` (detect the preview→commit handoff, pass `noEntrance` to the last committed bubble)

**Interfaces:**
- Consumes: `AssistantBubble`'s `preview` prop (Task 1).
- Produces: `AssistantBubble` accepts `noEntrance?: boolean` (default `false`) — when `true`, the bubble skips the `msg-in` slide and fades in quickly instead.

- [ ] **Step 1: Add `noEntrance` to `AssistantBubble.svelte`**

Extend the `Props` interface and destructure (from Task 1) to add `noEntrance`:

```svelte
  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
    preview?: boolean;
    noEntrance?: boolean;
  }
  let { text, ts, sessionName = '', preview = false, noEntrance = false }: Props = $props();
```

Apply it as a class on the root element. Change the root `<div class="assistant-msg">` to:

```svelte
<div class="assistant-msg" class:no-entrance={noEntrance}>
```

And in the `<style>` block, add after the `@keyframes msg-in { ... }` rule a variant that replaces the spring slide with a quick fade:

```css
  /* Swap a partir de um preview ao vivo: sem o slide de entrada (a bolha ja estava ali como preview);
     fade curto pra amaciar o reformat plano->markdown. */
  .assistant-msg.no-entrance { animation: msg-fade 150ms var(--ease-out) both; }
  @keyframes msg-fade { from { opacity: 0; } to { opacity: 1; } }
```

- [ ] **Step 2: Detect the preview→commit handoff in `MessageList.svelte`**

Add a falling-edge detector on `preview`: when `preview` goes from non-empty to empty, the next-rendered committed assistant bubble (the last one) is the swap target for one render. Add near the top of the `<script>` (after the existing `preview` prop is in scope):

```ts
  // Quando o preview ao vivo some (bloco commitou), a ULTIMA bolha de assistente que aparece
  // tomou o lugar dele -> entra sem o slide (no-entrance), pra a troca nao "pular". One-shot.
  let swapNext = $state(false);
  let prevPreview = $state('');
  $effect(() => {
    const now = preview;
    if (prevPreview && !now) swapNext = true;   // falling edge: preview -> vazio
    prevPreview = now;
  });
```

- [ ] **Step 3: Pass `noEntrance` to the last committed assistant bubble**

The committed messages render in the `{#each ...}` over `events`. Find the assistant render line (from Task 1 context it is `<AssistantBubble text={ev.text} ts={ev.ts} {sessionName} />`, ~line 170). Pass `noEntrance` only to the LAST event when `swapNext` is set. First confirm the each header has an index — if it is `{#each events as ev (key)}`, add `, i`: `{#each events as ev, i (<existing key expr>)}` (keep the existing key expression unchanged). Then replace the assistant render line with:

```svelte
        <AssistantBubble
          text={ev.text}
          ts={ev.ts}
          {sessionName}
          noEntrance={swapNext && i === events.length - 1}
        />
```

- [ ] **Step 4: Clear the one-shot after a committed render consumes it**

Add an effect that resets `swapNext` on the next animation frame once there is at least one event to consume it (so it applies to exactly one committed render, never to history on load):

```ts
  // Consome o one-shot: assim que ha evento no fim com swapNext setado, limpa no proximo frame.
  $effect(() => {
    if (swapNext && events.length) {
      const id = requestAnimationFrame(() => { swapNext = false; });
      return () => cancelAnimationFrame(id);
    }
  });
```

- [ ] **Step 5: Type-check**

Run: `npm --prefix frontend run check 2>&1 | tail -3`
Expected: `1 ERRORS 7 WARNINGS` baseline. Fix any new problem before committing.

- [ ] **Step 6: Manual verification**

During a working turn that produces a **markdown** answer (e.g. ask for a bulleted list or a bold word), confirm at the commit moment:
- The bubble does NOT slide up/re-animate; it stays in place and the formatted markdown fades in over the plain preview (soft, not a hard pop).
- A plain-prose answer still commits with no visible change.
- History messages on initial load still play their normal `msg-in` entrance (the no-entrance only applies to a just-swapped bubble, never on load).

If the one-shot targeting proves unreliable (e.g. the wrong bubble suppresses, or history flickers), revert Task 2's `MessageList` changes (keep Task 1) and note in the report that the instant-swap fallback was taken — this is acceptable per the spec.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/AssistantBubble.svelte frontend/src/components/MessageList.svelte
git commit -m "feat(frontend): seamless preview->committed swap (suppress entrance + crossfade)"
```

---

## Self-Review notes

- **Spec coverage:** §1 `AssistantBubble` preview mode → Task 1 Steps 1-3. §2 `MessageList` renders preview via the bubble + removes the box/caret → Task 1 Steps 4-6. §3 seamless swap + crossfade (secondary, fallback allowed) → Task 2, with the spec's instant-swap fallback written into Step 6. "Keep `Spinner`/reconciliation/dedup untouched" → enforced in Global Constraints and not modified by any step.
- **Plain preview preserved:** Task 1 renders preview text plain (no `renderMarkdown`), per the constraint.
- **Type consistency:** `preview?: boolean` and `noEntrance?: boolean` props are defined in Task 1 Step 1 / Task 2 Step 1 and consumed in `MessageList` with matching names.
- **No new deps, no new files, mobile + desktop share `Chat`/`MessageList`/`AssistantBubble` so both get the change.**
