# Reviewer — the report's catalog

This page belongs to the **moment you already hold the diff** and are deciding what to look for.
It is not a task list: it is the set of questions whose absence has already let a defect through
a gate. The procedure — what to read, where the report goes, the format and the recipe — is in
`revisor.md`, and the screen Task in `revisor-visual.md`.

## What the report must cover

Type gate, build and tests passing is the **floor**, not the report. Beyond it:

- **the full flow**, in the UI or the real command, not only the touched unit;
- **sibling callers**: does whoever else uses the changed symbol have the same cause?
- **concurrency**: delayed response, double click, target switched mid-flight, unmount;
- **final state**: what remained on disk/storage/URL afterwards — not only the return value;
- **An orchestration Task (tmux, CLI, process, account): RUN the smoke test against the real
  source, yourself.** A green suite of fakes is no proof of flow: a module once arrived with
  over three thousand green tests and the flow dead — 405 lines of new tests proved the code's
  own wrong assumption, and the reviewer reproducing against the real tmux caught the 10
  blockers. And **check the suite's COUNT against the base**: a count that dropped with no note
  in the report is a blocker by itself (in that same Task, one silent unit less hid 7 deleted
  tests of an approved Task). **And a test that swaps the whole library for a double proves the
  button calls the function, never where the function goes** — see `executor-fluxo.md`, the
  rule's two halves and the two outcome rules (the content of both ends; the evidence carrying
  what distinguishes the two paths).
- **the empty case**: code that **deletes**, that matches by similarity, or that decides from a
  list of the living — what does it do when the set comes **empty**? A pruning where "I don't
  know who is alive" became "nobody is alive" once deleted 8 of 8 live session files, queue
  included: the querying function returns empty without raising, so the author's `except` never
  fired. Short rule: **an empty list of the living is a reason NOT to delete.**
- **the same rule written twice**: two sides that must agree (backend and front, two components,
  two copies of the same client) agree **today** and nothing guarantees tomorrow. It happened
  with a floor duplicated on both sides: one gained a new notion, the other kept only the floor,
  the rules diverged and nobody was told.

**A branch whose base is not the current `main`: suite arithmetic lies.** Compare **names** — an
inventory of the parent's test names against the commit; none may vanish. On a branch born
fifteen commits back the count matched by coincidence, and the only valid check was the
inventory.

The group contract says what this work demands on top (review skills per Task type, visual
verification, a load harness). Read it before the first report.

### Declare the unit — the defect lives one level above where they sent you to look

This skill's most expensive failure mode is not reading too little: it is reading **at the wrong
unit**. Before closing the report, say in one line what your reading unit was — and go one level
up:

| You received | Your minimum unit is |
|---|---|
| a diff | the **whole function** it landed in |
| a fixed function | the **file**: what the same-kind siblings do (guard, in-flight flag, cleanup on switch) |
| a module that talks to the network | the **whole route**: WHICH destination each function talks to, and what the screen shows when each fails |
| a fix that changes **flight time** | **everyone who runs alongside that flight** — the new race shows in no line of the diff |
| a ported pattern | the **destination route**: a number that came along (cap, timeout, threshold) is a measurement of the origin and must be justified again here |
| the fix of a family defect | the **branch**: `git grep` of the symbol, with the count in the report |

Six rounds have fallen to this shape. **Cost of the remedy: a four-second `git grep`.**

**A CORRECTION round has two fixed questions, and they are mirrored:** (1) *what did this round
change of identity or lifecycle — and what started RE-EXECUTING because of it?* If the answer is
"a destructive cleanup/teardown", the required test is the one that re-executes **during the
operation**, not the happy path. (2) *What stopped re-executing?* When the fix removes something
from a condition or dependency list, demand the proof of the **path that thing existed to
serve** — the whole outcome, not "the reference points at the new place". The first catches the
fix that breaks; the second, the fix that turns things off. And in choosing the recipe,
**removing the trap beats erasing the symptom**: a destructive routine cannot depend on its
caller's identity. A fix with the cause reproduced, mutation proven and seven green gates once
**broke the very feature it fixed** — a new callback on every state update re-fired an effect's
destructive cleanup, the feature died with the screen saying it worked, and no test in the batch
re-executed during the operation. The mirrored question, at the next gate, yielded four tests;
one of them failed **by excess** before the fix.

### Does the test prove the scenario, or prove itself?

The question is not answered by reading the test. It is answered by **breaking the code on
purpose and watching the test fall**:

1. Copy the subproject outside the repo (the repo stays untouched — a regex mutation in the
   working tree once deleted `role`/`aria-live` in a run, see `executor.md`).
2. Remove **the fix's line**, one at a time, and run the suite.
3. Only the new test fell → it proves the scenario. Nothing fell → **that point has no test**,
   and that is a finding (a `NOTED` gap, not a blocker).

In one run, removing a guard felled exactly the new test's assertion, and removing the sibling
shortcut's guard left the whole suite green — the second point became a gap note. Another
mutation returned **880 green tests with the defect fully back**. This is not a suggestion: it
is the only thing separating a scenario-proving test from a decorative one.

**And the mutation belongs to the GATE, not to the executor.** Asking them to run the mutation
before checking the step off is cheap and helps — and it doesn't replace you running it on
**every new test a step or recipe demanded**: a test born with the right name that doesn't
exercise what it promises passes any reading, including its author's. Across one work's five
occurrences, the one who killed the defect was the reviewer mutating; the equivalent rule
charged to the executor prevented none.

**A harness closes deterministic races; it doesn't close external boundaries.** A defect that is
**the order of effects inside our own code** (loading before `ready`, a poll, an out-of-order
event) is proven by a test reproducing the sequence — the harness exercises the whole cause. A
defect that depends on **something OUTSIDE our code emitting the event** (the platform, the
browser, the OS: a native component's network error, permissions, keyboard, camera) is **not**
proven by a mock: clicking a mock's button proves the mock — there the proof is in the real
environment, and producing the failure outcome is usually cheap (airplane mode, a downed
service). The rule paid for itself on first use: the on-device smoke it forced found a
load-start event erasing the message the error handler had just written — the error appeared and
vanished, and no test in the batch would see it.

**And the fixture cannot be the world where the defect is invisible.** The test proving "the
dead one disappears" uses a **different living one**, never a world with no living: with the
world empty, "the dead disappears" and "everything is deleted" give the same output, and the
suite signs off on the defect. Six test calls once passed "nobody is alive" as the fixture, and
it was the data-loss path.

**Nor a world the server NEVER produces.** It is the previous one's opposite shape and costs
more: there the test couldn't fail; here it **asserts** an impossible state, and the green suite
signs off. Every fixture supporting an assertion is checked against real data — a `curl` on the
route, a row from the table — before counting as proof. A fixture once arrived with a field
filled that the service returns null in **all** real records, with a test asserting the label
only that state produces; it appeared in three files across two Tasks, and the second occurrence
rejected a round. **A corrected fixture enters the SAME commit as the fix it locks** — otherwise
the lock is illusory.

#### Before accepting a sabotage battery, four checks

This section has grown because each of its failure modes cost a round. The four below are the
ones forgotten in the moment, so they stay together, as a list, not prose:

1. **Does the cut isolate the path the claim NAMES?** The invalidating question: *is there
   another explanation for these tests falling?* A cut landing in shared code — an error mapper,
   a common `catch`, a helper — fells all paths together and is compatible with opposite
   hypotheses: it proves something there is exercised, not the sentence written next to it. A
   cut in a mapper shared between save and delete once supported both "the new test closes a
   hole" and "it is a copy of its sibling"; redone disabling only the claimed path, exactly the
   two right tests fell, with the rest as control. **A true claim with incomplete proof:
   complete the evidence, don't reject** — rejecting there charges the executor for the proof's
   design, not the work.
2. **Does each cut say WHICH TEST FILE accused?** "It felled 7 tests" doesn't separate *the new
   lock bit* from *an old test already bit and the new lock takes the credit*. Two batteries of
   the same Task went out without the field; redone with it, **10 of 10** biting cuts had one of
   the new locks as accuser — the suspicion was the right one to hold, and it was only known
   because the field existed. **And the denominator is single across the battery**: cuts
   measured against different-sized suites don't compare. A cut that crashes the process (stack
   overflow) runs **filtered to the test claimed as accuser** — unfiltered, the log comes out in
   megabytes and the accuser only shows by reading stacks; filtered, the command itself names
   it.
3. **Does each cut come PAIRED with a control?** The cut answers "does the defect return?"; the
   control answers "does the feature stay alive?". Without the second, a fix that destroys the
   functionality — or that "fixes" by deleting the line — passes as success. Measured three
   times: one control separated "the guard works" from "the field is dead"; another, added
   outside the recipe, felled all four proofs at once and showed that removing the line doesn't
   pass; a third kept green the path that already worked. **A control that stays GREEN under the
   cut is the right design, not a gap** — a biting control became a second lock, and the file
   lost the only "doesn't over-refuse" measure it had.
4. **Was a blocker about ABSENCE measured by two paths?** "This point has no test", "this
   function has no caller", "the field doesn't exist" are answers to **one** specific question,
   and different questions about the same repository return opposite answers. Write in the
   report **which question the search asked**, and redo it by a second path before it becomes a
   blocker. An absence recorded for two days once fell at the first search made with another
   word.

**The other half: a recipe that installs a LOCK demands the inverted proof.** The mutation
answers "does the test prove the scenario?"; the inverted proof answers "does the lock lock?".
Every recipe whose goal is preventing future regression — making a prop required, tightening a
type, adding a lint — only counts delivered with the verification **red without the fix** and
green with it. Ask the executor for both, on disk, and read both. Without it, either the lock is
born forever-red and someone turns it off, or it passes for a lock while locking nothing.

In a real application the lock was two lines plus the inverted proof; turned on, it accused
**two** errors, revealing a second spot missing the prop that the recipe didn't foresee — the
recipe said two lines and it was three. **When prescribing a lock, run the verification with the
change applied BEFORE writing the step count** — counting the spots you already know is not
counting the spots.

### Live proof measures what is SERVED, not what is committed

**Building is not proof. Proof is matching the identifier of the artifact you just built with
what the page actually loaded** — the bundle's hash, the file's date, whatever the platform has.
Building is the first step; the second is checking, and it is the one that counts. Find out
first **what the port serves** — the command the service actually runs: a dev port serving a
static *build* shows the previous commit without telling anyone. **The same holds for a
long-lived BACKEND service: it serves the code from when it started** — check the process's
start against the commit's date, or bring up your own instance on another port (and never
restart the user's service to measure). A process up since before the measured commit once
became a near-false "open blocker".

The same defect has shown through two mechanisms in one day (a port serving a precompiled build,
and a service worker serving its own cache); with the pair checked in every report, no
measurement had to be redone. The concrete how-to-check recipe is **the repository's**, not this
skill's: it lives in the group's rules file, with that project's command.

**Before opening the browser, compare the expressions side by side.** In one branch review, two
measurements were spent chasing a defect down the wrong path; the same derivation's three
expressions, laid term against term, showed the spot in minutes.

### Measure in both hosts and both states, and say in which

A screen that exists in two hosts (phone and desktop, panel and modal) is measured **in both** —
and the report says at which width each number was taken. Three rounds of one run fell to
measuring at a single breakpoint: the new tab appearing on desktop when the Task was the
phone's, and the same tab vanishing from both in the middle range.

The axis is not only width: it is **any state of the neighboring region**. Two other rounds fell
to measuring in the wrong state — the scrolling checked on a tab that scrolled for having dozens
of items, when the original number came from another; and a panel's ceiling calibrated only with
the neighbor **open**, when its normal state is closed. Short rule: **always measure in the same
state where the original number was taken, and note the state next to the number.**

**Behavior proof goes to the outcome.** "Connected", "saved", "opened" — not the state right
before it. A screenshot of an enabled button is no proof the click works: evidence that stopped
at the disabled button forced the gate to run the flow's end.

## And the tone of all this

The review is adversarial: you try to **break** the final state, not to confirm the plan was
followed. A report that only confirms plan, types and build is the gate not existing.
