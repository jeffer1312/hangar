# Executor — the visual Task

This page belongs to the Task whose diff touches pixels: `.svelte`/`.tsx`/`.vue`, CSS,
templates, anything that draws. Its gate is mandatory even when the plan does not ask. Diff
draws nothing: go back to `executor.md`.

A green test, a zeroed type gate, the DOM, CSS and the accessibility tree do not replace
seeing the screen.

## 1. Open it

Check, do not presume, in this order: a browser skill on your skill list (`agent-browser` and
the like); a Chrome MCP among your tools (`chrome-devtools`, `claude-in-chrome`); an automation
CLI (`command -v agent-browser`, `playwright`, `puppeteer`).

- "I have no browser" counts only after looking, and goes in the report with what you tried.
- A kick-off, contract or recipe saying "there is no browser" is not a fact about your tools:
  check your own list. Faking verification is forbidden; verifying never is. In doubt, open it,
  and say in the report that you did.

## 2. Exercise it

For each thing the Task put on screen:

- click what is clickable and confirm the effect (panel opened, field appeared, request went
  out, message shown);
- walk the states the Task affects: empty, loading, with data, error, disabled;
- check it looks like the rest of the app: same height, border and spacing as its siblings.

A click that does nothing visible is a defect: chase the reason (console, network, handler)
before reporting.

**Code crosses a process, a port or a device:** the proof is the artifact the TARGET loaded.
Read a marker of your commit in what that side downloaded; confirming the build finished on your
side is not proof. How the stage rises (directory, port per Task, who holds the device) is the
plan's; missing from the plan, block for the arbiter.

A command that follows a process (`tail -f`, follow-mode logs, a foreground server) locks the
turn: use the flag that exits, `timeout N`, or background file logging.

## 3. Capture

- Confirm the tab is yours before each capture round: `location.href` returns your port. It
  returned another: reopen your URL. Taken again: report the conflict to the arbiter.
- How many screenshots is your call; the plan says which STATES must be proven.
- Stopping point: 1h or 60 navigation commands per Task. Hit it: stop and report with what you
  have; if the sweep is big, propose to the arbiter a separate capture session with the state
  list in its kick-off. A new state discovered midway goes to the arbiter's list, not into your
  loop.
- One screenshot per state, at an absolute path in the durable directory (default
  `~/.hangar/orq/<date>-<gid>/visual/`), never `/tmp`. Fixed something afterwards: recapture.
- Check the four invalidators before the first capture: (1) viewport equals what the contract
  fixed; (2) same language on both sides; (3) an element ending at the PNG's edge is scrolling,
  not drawing: recapture scrolled or declare the point not compared; (4) the screenshot frames
  the state's proof together with its effect.
- Every claim about color, sign or state (`✓` / `✗` / `·`, enabled, disabled) is written from a
  300–400% crop of the detail, and the caption cites the color with the sign.
- Write each caption looking at that file. "idem" is forbidden.
- Per round, join the states into one panel (`folha <shots in state order>`, numbered, up to 6
  per sheet) and the reference's same states into another. The panels go to the reviewer only;
  the single screenshots stay for a detail check. The arbiter gets no screenshot.
- The proof of a behavior Task ends at the outcome the user asked for ("connected", "saved",
  "opened"), not at the state right before it.

## 4. Look at the screenshot

Read the image yourself first, by absolute path. Delegate only when the read fails (the tool
refuses the file, a hook blocks it, the model does not take images), in this order:

1. a vision command on the machine (`command -v see`; `see <image> "<question>"`);
2. a subagent whose model sees images, given the absolute path;
3. neither: tell the arbiter before sending the round; whoever on the team has vision does that
   part, and the arrangement goes into the contract.

Never describe a screenshot from context or from the file name. Never answer "I can't see
images".

Ask a specific question, never "does it look good?": *does the button right of the selector
have a frame and the same height?*, *does the active item stand out?*, *is an opaque rectangle
covering the background?*, *does the text fit uncut at this width?*

**Size is measured in the DOM, not eyeballed.** Before deciding any layout divergence:

```js
[...document.querySelectorAll('.your-class')].map(e => e.getBoundingClientRect().height)
```

Measure the real neighbor (the sibling tab, the list next door, the existing component).

## 5. Compare blind against the bar

The plan gives every pixel-touching Task a bar: a named screen, opened and captured in the same
state and width as yours. Capture both and ask a fresh subagent, without saying which is which:

> Two images: `<durable-dir>/visual/A.png` and `<durable-dir>/visual/B.png`. Same screen, two
> renderings. **Which of the two looks more finished?** Answer `A` or `B`, then the **biggest
> hole** of the loser, in one concrete sentence (what is misaligned, cut off, low-contrast, or a
> different height from its siblings).

- Neutral file names (`A`/`B`); alternate which letter is your work between rounds.
- A fresh subagent, never the arm that drew.
- A binary choice, never a score.
- Lost: fix the biggest hole, recapture, run again. Cap of 2 bar rounds; then send the round
  with the result in the report, even losing, as a known risk for the arbiter.
- The cap counts bar rounds only; a round rejected for a screen defect (wrong width, trapped
  focus, touch target under 44px) does not spend it.
- A correction round that touches no pixel (`git stash show --stat <object>` proves it) redoes
  no comparison.
- You cannot see images: same delegation as step 4; you command and read the answer.
- Contract says `Bar: none — user's decision`: skip this step, send with steps 1–4. Do not
  invent a bar.
- Diff touches pixels and the contract has neither bar nor waiver: stop and report to the
  arbiter before sending the round.
- The blind choice answers "which looks more finished", never "which does more". A Task that
  replaces an existing surface still owes the inventory of what the old one did (the arbiter's
  page); a blind win and an inventory rejection in the same round is normal.

## What goes in the report

The two panels' paths. Per state: the screenshot's path, what you clicked and what happened, the question asked of
whoever sees (if delegated) and the answer, what you changed because of it.

With a bar: who won each blind round and which letter was yours, the biggest hole named, what
you fixed, the final screenshot's path. Lost both rounds: say so, with the hole that remains.

Without this the reviewer blocks the Task.

## Report line

```
Invalidators: viewport <value> · language <value> · edge <recaptured | point not compared> · framing <ok | state that failed>
```
