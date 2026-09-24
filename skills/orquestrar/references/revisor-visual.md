# Reviewer — the visual gate

This page belongs to the Task that touches pixels. Diff draws nothing: go back to `revisor.md`
and `revisor-catalogo.md`.

## Proof of seeing, or a BLOCKER

- The round carries the absolute paths of the per-state screenshots, the visual question asked
  of each, and the answer. DOM, CSS and the accessibility tree do not substitute.
- A screenshot from before the fix does not count.
- Task with a bar: the `Visual:` line points at the executor's visual report, which carries per
  round who won, which letter was their work, the biggest hole named and what they fixed.
  "I compared and it looks good" blocks.

## The bar: one pass, not a redo

Do not redo the executor's blind protocol. Make one pass at the end over the final screenshot
and the bar, hunting:

- the bar swapped midway: check the four invalidators of `executor-visual.md`, step 3
  (viewport, language, edge-cut element, effect framed with the state);
- they won and it is still wrong: an opaque rectangle over the wallpaper, cut-off text, a state
  nobody captured.

## Bar vs screen defect

- Bar = "is it faithful to the mock?". With the round cap met the bar CLOSES: nobody redoes it,
  the remaining aesthetic divergence is NOTED. Lost both rounds and sent anyway is not an
  automatic blocker; judge the hole that remains.
- Screen defect = "is it broken?" (overlap, illegible text, a notice that does not show, a
  small touch target, wrong width, focus trapped or lost outside the modal). No cap; full
  blockers until closed; they do not spend the bar's cap.
- A correction round that touches no pixel (`git stash show --stat <object>`) redoes no
  comparison.

## No bar, or a waived bar

- Pixels touched and no bar at all in the contract: `DEVOLVIDO` to the arbiter: "Task N draws a
  screen and the contract carries neither a bar nor a waiver; the bar is the user's decision".
  Do not propose one, pick one, or judge as if it existed.
- Contract says `Bar: none — user's decision`: judge normally without the blind comparison
  (per-state screenshots, missing state is a finding) and enforce no bar.

## How to look without burning context

- Do not follow screenshot by screenshot while the work moves. At the end, open ALL the
  screenshots at once and check each shows what its caption says.
- A symbol or color claim is checked on the zoomed crop, never by eye on the whole image.
- Hunt: a screenshot that does not prove its caption, a state captured at the wrong moment
  (before the fix, mid-transition), a state nobody captured.
- The capturer's description is input; the conclusion is yours. You cannot see images and the
  Task is visual: tell the arbiter.

## Report line

```
Screens: <N opened together at the end> · invalidators: <ok | which failed> · bar: <result | "waived by <who>">
```
