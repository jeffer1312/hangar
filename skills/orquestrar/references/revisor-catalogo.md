# Reviewer — the report's catalog

This page belongs to the **moment you already hold the diff** and are deciding what to look for.
It is not a task list: it is the set of questions whose absence has already let a defect through
a gate. The review is adversarial — you try to **break** the final state, not to confirm the plan
was followed; a report that only confirms plan, types and build is the gate not existing. The
procedure — what to read, where the report goes, the format and the recipe — is in `revisor.md`,
and the screen Task in `revisor-visual.md`.

## What the report must cover

Type gate, build and tests passing is the **floor**, not the report. Beyond it:

- **the full flow**, in the UI or the real command, not only the touched unit;
- **sibling callers**: does whoever else uses the changed symbol have the same cause?
- **concurrency**: delayed response, double click, target switched mid-flight, unmount;
- **final state**: what remained on disk/storage/URL afterwards — not only the return value;
- **a Task whose code drives an external process, CLI, account or service: RUN the smoke test
  against the real source, yourself.** A green suite of fakes proves no flow — the fakes assume
  what the code assumes (`executor-fluxo.md`: the rule's two halves and the two outcome rules).
  And **check the suite's COUNT against the base**: a count that dropped with no note in the
  report is a blocker by itself.
- **the empty case**: code that **deletes**, that matches by similarity, or that decides from a
  list of the living — what does it do when the set comes **empty**? A querying function that
  returns empty without raising turns "I don't know who is alive" into "nobody is alive". Short
  rule: **an empty list of the living is a reason NOT to delete.**
- **the same rule written twice**: two sides that must agree (backend and front, two components,
  two copies of the same client) agree **today** and nothing guarantees tomorrow.

**A branch whose base is not the current `main`: suite arithmetic lies.** Compare **names** — an
inventory of the parent's test names against the commit; none may vanish.

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

The remedy is a `git grep`.

**A CORRECTION round has two fixed questions, and they are mirrored:** (1) *what did this round
change of identity or lifecycle — and what started RE-EXECUTING because of it?* If the answer is
"a destructive cleanup/teardown", the required test is the one that re-executes **during the
operation**, not the happy path. (2) *What stopped re-executing?* When the fix removes something
from a condition or dependency list, demand the proof of the **path that thing existed to
serve** — the whole outcome, not "the reference points at the new place". The first catches the
fix that breaks; the second, the fix that turns things off. And in choosing the recipe,
**removing the trap beats erasing the symptom**: a destructive routine cannot depend on its
caller's identity — a fix with the cause reproduced, mutation proven and every gate green can still
break the very feature it fixed, with the screen saying it worked.

### Does the test prove the scenario, or prove itself?

The question is not answered by reading the test. It is answered by **breaking the code on
purpose and watching the test fall**:

1. `git worktree add --detach <tmp>/mut-<x> <object>` (the repo stays untouched — a mutation in
   the working tree leaves residue that rides into the commit, see `executor.md`); `git worktree
   remove --force` at the end.
2. Remove **the fix's line**, one at a time, and run the suite.
3. Only the new test fell → it proves the scenario. Nothing fell → **that point has no test**,
   and that is a finding (a `NOTED` gap, not a blocker).

This is not a suggestion: a mutation has returned a fully green suite with the defect fully back,
and it is the only thing separating a scenario-proving test from a decorative one.

**And the mutation belongs to the GATE, not to the executor.** Asking them to run the mutation
before checking the step off is cheap and helps — and it doesn't replace you running it on
**every new test a step or recipe demanded**: a test born with the right name that doesn't
exercise what it promises passes any reading, including its author's.

**A harness closes deterministic races; it doesn't close external boundaries.** A defect that is
**the order of effects inside our own code** (loading before `ready`, a poll, an out-of-order
event) is proven by a test reproducing the sequence — the harness exercises the whole cause. A
defect that depends on **something OUTSIDE our code emitting the event** (the platform, the
browser, the OS: a native component's network error, permissions, keyboard, camera) is **not**
proven by a mock: clicking a mock's button proves the mock — there the proof is in the real
environment, and producing the failure outcome is usually cheap (airplane mode, a downed
service).

**And the fixture cannot be the world where the defect is invisible.** The test proving "the
dead one disappears" uses a **different living one**, never a world with no living: with the
world empty, "the dead disappears" and "everything is deleted" give the same output, and the
suite signs off on the defect — and "nobody is alive" passed as a fixture is exactly the
data-loss path.

**Nor a world the server NEVER produces.** It is the previous one's opposite shape and costs
more: there the test couldn't fail; here it **asserts** an impossible state, and the green suite
signs off. Every fixture supporting an assertion is checked against real data — a `curl` on the
route, a row from the table — before counting as proof; a fixture asserting a state the service
never produces spreads across files and Tasks and keeps rejecting rounds. **A corrected fixture
enters the SAME round as the fix it locks** — otherwise the lock is illusory.

#### Before accepting a sabotage battery, four checks

Each of these has let a defect through when forgotten, so they stay together, as a list:

1. **Does the cut isolate the path the claim NAMES?** The invalidating question: *is there
   another explanation for these tests falling?* A cut landing in shared code — an error mapper,
   a common `catch`, a helper — fells all paths together and is compatible with opposite
   hypotheses: it proves something there is exercised, not the sentence written next to it. **A
   true claim with incomplete proof: complete the evidence, don't reject** — rejecting there
   charges the executor for the proof's design, not the work.
2. **Does each cut say WHICH TEST FILE accused?** "It felled 7 tests" doesn't separate *the new
   lock bit* from *an old test already bit and the new lock takes the credit*. **And the
   denominator is single across the battery**: cuts measured against different-sized suites don't
   compare. A cut that crashes the process (stack overflow) runs **filtered to the test claimed as
   accuser** — unfiltered, the accuser only shows by reading stacks in megabytes of log.
3. **Does each cut come PAIRED with a control?** The cut answers "does the defect return?"; the
   control answers "does the feature stay alive?". Without the second, a fix that destroys the
   functionality — or that "fixes" by deleting the line — passes as success. **A control that
   stays GREEN under the cut is the right design, not a gap** — a biting control is a second lock,
   and the file loses its only "doesn't over-refuse" measure.
4. **Was a blocker about ABSENCE measured by two paths?** "This point has no test", "this
   function has no caller", "the field doesn't exist" are answers to **one** specific question,
   and different questions about the same repository return opposite answers. Write in the
   report **which question the search asked**, and redo it by a second path before it becomes a
   blocker.

**The other half: a recipe that installs a LOCK demands the inverted proof.** The mutation
answers "does the test prove the scenario?"; the inverted proof answers "does the lock lock?".
Every recipe whose goal is preventing future regression — making a prop required, tightening a
type, adding a lint — only counts delivered with the verification **red without the fix** and
green with it. Ask the executor for both, on disk, and read both. Without it, either the lock is
born forever-red and someone turns it off, or it passes for a lock while locking nothing. **When
prescribing a lock, run the verification with the change applied BEFORE writing the step count** —
a lock turned on reveals the spots the recipe didn't foresee, and counting the spots you already
know is not counting the spots.

### Live proof measures what is SERVED, not what is committed

**Building is not proof. Proof is matching the identifier of the artifact you just built with
what the page actually loaded** — the bundle's hash, the file's date, whatever the platform has.
Building is the first step; the second is checking, and it is the one that counts. Find out
first **what the port serves** — the command the service actually runs: a dev port serving a
static *build* shows the previous commit without telling anyone. **The same holds for a
long-lived BACKEND service: it serves the code from when it started** — check the process's
start against the commit's date, or bring up your own instance on another port (and never
restart the user's service to measure); a process up since before the measured commit is a false
"open blocker" waiting to happen. Two mechanisms produce the same defect — a port serving a
precompiled build, a service worker serving its own cache — so check the pair in every report. The
concrete how-to-check recipe is **the repository's**, not this skill's: it lives in the group's
rules file, with that project's command.

**Before opening the browser, compare the expressions side by side** — a derivation laid term
against term shows in minutes the spot that measurements chase for rounds.

### Measure in both hosts and both states, and say in which

A screen that exists in two hosts (phone and desktop, panel and modal) is measured **in both** —
and the report says at which width each number was taken; a defect can appear in one host and
vanish from both in the middle range.

The axis is not only width: it is **any state of the neighboring region** — a list that scrolls
only because it has dozens of items, a ceiling calibrated with the neighbor open when its normal
state is closed. Short rule: **always measure in the same state where the original number was
taken, and note the state next to the number.**

**Behavior proof goes to the outcome.** "Connected", "saved", "opened" — not the state right
before it. A screenshot of an enabled button is no proof the click works.
