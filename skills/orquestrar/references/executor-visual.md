# Executor — the visual Task

This page belongs to the **Task that changes what shows on screen**, and to it alone. If your
diff doesn't touch pixels, go back to `executor.md`. If it does, this gate is yours and is not
optional — **even if the plan doesn't ask**.

It holds for every Task whose diff touches `.svelte`/`.tsx`/`.vue`, CSS, templates, or anything
else that draws pixels.

## A green test is not a working screen

**And that is not an opinion.** A selector can pass hundreds of tests, a zeroed type gate and an
independent review — and reach the user **invisible** (a CSS rule losing the cascade); a click
can become nothing, silently. No test, gate or diff reading catches that class of error — only
the pixel does.

**DOM, CSS and the accessibility tree don't replace seeing.** They say the element exists, not
that it is legible, aligned, inside the app's theme, or that it hasn't become an opaque
rectangle over the wallpaper.

## The protocol, in five steps

It holds for every executor; the one who can't see images has one extra step, marked ahead.

### 1. Truly open it. The tools exist — look before saying they don't

Order of preference, and **you check, you don't presume**:

| Path | How to know it's there |
|---|---|
| a browser skill (`agent-browser` and the like) | it's on your skill list |
| the Chrome MCP (`chrome-devtools`, `claude-in-chrome`) | it shows in your tools |
| an automation CLI (`agent-browser`, `playwright`, `puppeteer`) | `command -v agent-browser` |

**"I have no browser" only counts after looking, and it goes in the report with what you
tried.** A kick-off, contract or recipe asserting there is no browser **is not a fact about your
tools** — it is a sentence someone wrote before knowing your session. An executor that reads
"there is no browser" in a kick-off and backs off out of obedience to the text, with a browser
tool sitting on its own list, sends the broken screen to the user.

The rule that resolves the ambiguity: **FAKING verification is forbidden; verifying never is.**
An instruction that seems to stop you from opening the screen is talking about inventing
results, not about using a tool you have. In doubt, open it — and say in the report that you
did.

### 2. Exercise it, don't just look

A screenshot of a still screen proves it drew, not that it works. For each thing the Task put on
screen:

- **click** what is clickable and confirm the effect — the panel opened, the field appeared, the
  request went out (network/log), the success or error message showed;
- walk through the **states** the Task affects: empty, loading, with data, error, disabled;
- check that **what you created looks like the rest of the app** — same height, same border,
  same spacing as the siblings next to it. A new component that looks like loose text glued to
  the edge is wrong even though it compiles.

A click that does nothing visible is a **defect**, not "probably works": chase the reason
(console, network, the handler) before reporting.

### When the code crosses a process, a port or a device

**The proof is the artifact the TARGET loaded, not what your machine serves.** Whenever the code
passes through a server, a port or a device before becoming what you will judge, the question is
**which build that side is running** — and it is answered by reading a marker of your commit in
the artifact it downloaded, never by confirming on your side that the build finished. A green
local request and a device serving another worktree's bundle coexist with no error on screen.

This is a rule of **evidence**, which is why it lives here: this family's defect doesn't block
you — it stays green. You don't ask the arbiter for help because you believe you proved it.

How to raise that stage — from which directory the server rises, which port belongs to each
Task, who holds the device — is the plan's, not yours: it is the phase-1 gate's owned external
precondition. Missing from the plan, it is a block for the arbiter, not your improvisation.

**A command that FOLLOWS a process locks the whole turn** — logs in follow mode, `tail -f`, a
server in the foreground. Use the flag that exits, `timeout N`, or background file logging.

### 3. Capture

**First, confirm the tab is YOURS.** The automation browser (`agent-browser` and the like) can be
**one per machine** — in another batch, another session navigates the SAME tab as you. Before
each capture round: `location.href` must return **your** port. It returned another → the tab was
taken; reopen your URL. Taken again → **report the conflict to the arbiter** instead of
insisting — hours of questions to another Task's page cost what a one-second command avoids.

**How many screenshots to take is YOUR call, on the spot** — the user's decision. Neither the
plan nor the arbiter imposes a number: the one who knows how many screens this Task ended up
having is you, executing. The plan says which **states** must be proven; the file count is your
problem.

**What exists is a stopping point, not a limit: 1h or 60 navigation commands per Task.** Hit it,
**stop and report with what you have** — not because you exceeded a quota, but because capture
beyond that is usually a sign of something else (a broken stage, a state that won't reproduce, a
state list bigger than the Task). Capture without a stopping point holds Tasks hostage **for
hours, with no merge**. If, when reporting, it becomes clear the sweep really is big, **propose to the
arbiter a separate capture session** — cheap, disposable, with the state list in its kick-off;
you deliver code, verifications and the sanity screenshot, and the capturer sweeps the rest.
That proposal is yours; its execution is his. A new state discovered midway goes to the arbiter's
list, not into your loop. (The 2-round cap further down belongs to the blind comparison; this one
belongs to the work of capturing — the two coexist.)

One screenshot per state, at an **absolute path** in a **durable** directory — the one the
launch decided (the default is `~/.hangar/orq/<date>-<gid>/visual/`), never `/tmp`, which
vanishes on reboot and takes the retrospective's raw material with it. Fixed something
afterwards? **Recapture.** An old screenshot proves the bug, never the fix.

**Four things INVALIDATE a visual comparison, and none of them produces an error — the proof
comes out pretty and is garbage. Check all four BEFORE the first capture:** (1) **a
size/viewport different from what the contract fixed** — a desktop reference judged against a
phone capture decides nothing; (2) **different languages on the two sides** — the judge compares
`Save/Discard` with `Salvar/Descartar` and judges translation, not parity, and that already cost
a whole round; (3) **an element ending at the PNG's edge is scrolling, not drawing** — it
decides no comparison; recapture scrolled or declare that point not compared; (4) **the
screenshot frames the STATE's proof together with the effect** — a screenshot that only means
something next to a command outside it becomes a word dispute in the next review. These four are
repeated in the kick-off of every visual Task (a guideline buried in a contract doesn't reach a
session born after it — that is exactly how #2 cost the round).

**Every claim about color, sign or state (`✓` / `✗` / `·`, enabled, disabled) is written with
the detail ZOOMED, never by eye on the whole image** — and the caption cites the color together
with the sign. In a 38-screenshot Task, every finding that survived review came from zooming a
detail that looked legible. Cost: one 300–400% crop per claim.

**Each caption line is written looking at that file; "idem" is forbidden.** Two screenshots of
the same state at different widths or languages get two descriptions. The "idem / idem en / idem
mobile" template produces **wrong captions over right pixels** — half the caption sheet can lie
while every screenshot is fine.

**The proof of a behavior Task ends at the outcome the user asked for** — "connected", "saved",
"opened" — not at the state right before it. A screenshot of the enabled button is no proof the
click works. Evidence that stopped at the disabled button forced the gate to run the flow's end
to discover the outcome worked — a review round spent on what the proof should have shown.

### 4. Look at the screenshot. Delegate only if you can't

**Try to read the image yourself first**, by absolute path. Many executors can see — if you are
one of them, look, answer the next step's questions and **done**: delegating there is just
latency, and one more middleman between you and the pixel.

Delegate **only** when the read actually fails — the tool refuses the file, a hook blocks it, or
the model doesn't take images. In that case, and only then:

1. a vision command installed on this machine, if any (`see <image> "<question>"` is the usual
   name — check with `command -v see`);
2. a subagent whose model sees images, passing the **absolute path**;
3. neither existing, **tell the arbiter before committing** — whoever on the team has vision
   (usually the reviewer) does that part, and the arrangement goes into the contract.

When you can **not** see, what reaches you is a file path, not a picture: describing the
screenshot "from context", from the file name or from what the conversation suggests is
**invention**, however plausible it sounds. And answering "I can't see images" is also false —
you can, by delegation. Both exits are closed: either you look, or someone looks for you.

### Size is not eyeballed: it is measured in the DOM

A screenshot answers **what exists, where, in what order**. It does **not** answer size, spacing
or alignment — and that is where whoever judges by image errs with confidence.

In a screen Task the executor compared the result with the mock by screenshot, got back "the
density looks different", decided by argument that the real app ruled, and committed. The
reviewer measured the box: mock and the same panel's sibling tab around 24px, the delivery at
44px, across **seven** elements. It wasn't the real app winning: it was a global min-height
eating the component's CSS, with nobody overriding it. The screenshot showed the difference;
only the number said whose fault it was.

Before deciding any layout divergence:

```js
// in the browser, screen open
[...document.querySelectorAll('.your-class')].map(e => e.getBoundingClientRect().height)
```

And measure **the real neighbor** — the sibling tab, the list next door, the component that
already exists. "The real app wins" is a rule about the **measured** app, not the imagined one.

Ask a **specific** question, never "does it look good?" — it holds both for you looking and for
whoever looks for you. Good ones: *"does the button to the right of the selector have a frame
and the same height, or is it loose text?"*, *"does the active item stand out from the
others?"*, *"is any opaque rectangle covering the background?"*, *"does the text fit uncut at
this width?"*. "Does it look good?" returns "it looks good" and costs no one anything.

### 5. Compare blind against the bar

The plan gives a **bar** to every pixel-touching Task: a named screen, which can be opened and
captured, in the same state and width as your screenshot. Capture both sides and put a **fresh
subagent** to choose — without saying which is which:

> Two images: `<durable-dir>/visual/A.png` and `<durable-dir>/visual/B.png`. Same screen, two
> renderings. **Which of the two looks more finished?** Answer `A` or `B`, then the **biggest
> hole** of the loser, in one concrete sentence (what is misaligned, cut off, low-contrast, or a
> different height from its siblings).

Three things that make this worth anything:

- **Truly blind**: neutral file names (`A`/`B`), and you **alternate** which letter is your work
  between rounds. `new.png` vs `reference.png` is not blind — it is a hint.
- **A fresh subagent, never the arm that drew.** Whoever built it already knows why each choice
  was made, and defends it. It is the same reason the reviewer is never the session that
  executed.
- **A binary choice, not a score.** "Which is better" has an answer; "from 0 to 10, how good"
  returns 7 every time.

Lost → fix **the biggest hole**, recapture, run it again. **Cap of 2 rounds**, then you commit
with the result in the report, even losing. The cap is not laziness: a loop with no spending
boundary is this technique's measured failure mode out there — people burning hundreds of
dollars and throwing away 95% of what came out. Lost both rounds → it goes in the report as a
known risk, and the arbiter decides.

**The cap counts BAR rounds. Code defects don't count.** A round rejected because the screen is
broken — wrong width, trapped focus, a touch target under 44px — is not about fidelity and
doesn't spend the cap. Without that separation the Task blows the cap with a broken screen,
which is the opposite of what the cap exists to avoid. One Task closed in four rounds, **only
the first about the bar**; another round rejected over a side inset pushing the display a few
hundred pixels, which is a real width bug, not finish.

**And a round that touches no pixel doesn't pay the bar again.** A fix commit that only
touches store, tests or backend redoes no comparison — `git show --stat` proves it.

You can't see images? The step is still yours — same protocol as step 4: the vision subagent
(or `see`) looks, you command and read the answer.

**Contract saying `Bar: none — user's decision`: skip this whole step** and commit with steps 1
to 4. Don't invent a bar on your own — the reference is the user's choice, and one picked by you
measures your own guess, not the work.

**Your diff touches pixels and the contract has neither a bar nor a waiver? Stop and report to
the arbiter before committing.** It is a phase-1 decision left blank; he asks the user and
brings you the answer. Committing anyway costs the whole Task, because the reviewer returns it.

### What goes in the report

Per state: the screenshot's path, what you **clicked** and what happened, the question you asked
whoever sees (if delegated) and what came back, and what you changed because of it.

A Task with a bar also carries: **who won each blind round** (and which letter was yours), the
biggest hole named, what you fixed, and the final screenshot's path. Lost at the end of both
rounds → say it plainly, with the hole that remains.

Without this the reviewer blocks the Task. It is not bureaucracy: it is the only evidence
separating "the code compiles" from "the screen works", and the two have already come apart
here.
