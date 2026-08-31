# Reviewer — the visual gate

This page belongs to the **Task that touches pixels**. If the diff draws nothing, it isn't
yours: go back to `revisor.md` (procedure) and `revisor-catalogo.md` (what the report covers).

## Without proof of seeing, it is a BLOCKER

A Task that changes what shows on screen only passes with evidence that someone **saw**: the
absolute paths of the per-state screenshots, the visual question asked of each, and what came
back. DOM, CSS and the accessibility tree do **not** substitute — they prove the element exists,
not that it is legible, aligned, or that it hasn't become an opaque rectangle over the
wallpaper.

The protocol for an executor without vision is in `executor-visual.md`. A screenshot from before
the fix doesn't count: if they fixed it, they must have recaptured.

**A Task with a bar: the blind verdict comes along, or it is a BLOCKER.** The plan names, for
every pixel-touching Task, what the result is compared against — a screen that can be opened, in
the same state and width. The executor's report must carry, per round: who won, **which letter
was their work**, the biggest hole named and what they fixed. A report saying only "I compared
and it looks good" is the same "does it look good?" in other clothes — it blocks.

**You do NOT redo the executor's blind protocol.** They already ran it, with a fresh subagent
and a round cap; redoing it is paying again the most expensive part of your window for an answer
you already have. What is yours is **one pass**, at the end, over the final screenshot and the
bar, hunting the two things theirs doesn't catch:

- **The bar swapped midway** — they compared against a different state, another width, or a
  version of the reference screen that already changed. A comparison against the wrong bar is
  false evidence, not weak evidence.
- **They won and it is still wrong** — the bar is the floor, not the ceiling. Winning the blind
  comparison doesn't excuse an opaque rectangle over the wallpaper, cut-off text, or a state
  nobody captured.

In a run where the reviewer redid the blind comparison across six rounds, the result was six
divergences and **zero** blockers; that work's 24 blockers all came from the code.

**The bar is "is it faithful to the mock?"; a screen defect is "is it broken?".** That is the
question separating the two before you write the finding, and each has a different ending:

- **With the round cap met, the bar CLOSES** — and closed means nobody redoes it, you included.
  Whatever aesthetic divergence remains becomes `NOTED`. They lost both rounds and committed
  anyway (which is what `executor-visual.md` orders, with the risk declared): it is **not** an
  automatic blocker — you judge the hole that remains.
- **A screen defect has no cap**: overlap, illegible text, a notice that doesn't show, a small
  touch target, wrong width, focus trapped or lost outside the modal. They remain full blockers
  until closed, and **don't spend the bar's cap**, because they are not about fidelity. Without
  that separation the Task blows the cap with a broken screen, the opposite of what the cap
  exists to avoid.

In one screen Task the bar was closed at round 2 by the arbiter's decision and rounds 3 to 5
still found **five blockers**, none about fidelity; another closed in four rounds, **only the
first about the bar**.

**A round that touches no pixel doesn't pay the bar again.** A fix commit that only
touches store, tests or backend redoes no comparison — `git show --stat` proves it, and your
window goes entirely to the code.

**A Task that touches pixels with no bar at all in the contract: `DEVOLVIDO`.** It is not a code
blocker — it is a phase-1 decision nobody made, and a process problem doesn't become a technical
finding. Return it to the arbiter saying *"Task N draws a screen and the contract carries
neither a bar nor a waiver; the bar is the user's decision"*, and stop there: you don't propose
the bar, don't pick one, and don't judge as if it existed. The two things the missing bar would
do silently — the executor skipping the blind comparison and you approving without enforcing —
are exactly what this `DEVOLVIDO` pulls out of the silence.

**Contract saying `Bar: none — user's decision`: judge normally.** The Task passes the visual
gate without the blind comparison (per-state screenshots, you look at the set at the end, a
missing state is still a finding) and **you enforce no bar**. The user's recorded choice is an
order, not a gap — enforcing a bar after they waived it is reopening a made decision.

**How to look without burning context:** don't follow screenshot by screenshot while the work
moves. Whoever captures, describes — the executor and your verification session can both see (a
local vision command or a vision subagent; on a machine with the `see` helper, that is it). Let
the two work and, **at the end, open ALL the screenshots at once** and check that each shows
what you needed. One pass of yours, at the end, over the set — not a read of yours per image.

And **a symbol claim is checked zoomed**: in the final pass, sign and color cited in a caption
are checked against the crop, not the whole image — two naked-eye reads once called a green `✓`
an `✗`.

What that final pass hunts: a screenshot that doesn't prove what its caption says, a state
captured at the wrong moment (before the fix, mid-transition), and above all **a state nobody
captured** — a missing state is a finding. The capturer's description is input; the conclusion
is yours, and the only way it counts is you having looked at the set. If **you** can't see
images either and the Task is visual, tell the arbiter: a blind reviewer judging a screen is the
gate not existing.
