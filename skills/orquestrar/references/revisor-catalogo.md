# Reviewer — the report's catalog

Read with the diff already in hand. The review is adversarial: try to break the final state,
not to confirm the plan was followed. The procedure and the format are in `revisor.md`, the
recipe in `revisor-receita.md`; the screen Task in `revisor-visual.md`.

## What the report must cover

The Task's focused tests passing are the floor; full suites wait for phase 4. Beyond it:

- the full flow, in the UI or the real command, not only the touched unit;
- sibling callers: does whoever else uses the changed symbol have the same cause?
- concurrency: delayed response, double click, target switched mid-flight, unmount;
- final state: what remained on disk, storage or URL afterwards;
- a Task whose code drives an external process, CLI, account or service: run the smoke test
  against the real source yourself (`executor-fluxo.md` rules apply); compare the suite's COUNT
  against the base, and a silent drop is a blocker;
- the empty case for code that deletes, matches by similarity or decides from a list of the
  living: an empty list of the living is a reason NOT to delete;
- the same rule written twice (backend and front, two components, two copies of a client).

Branch whose base is not the current `main`: compare test NAMES against the parent; none may
vanish.

## Declare the unit, one level above where you were sent

The `Unit:` report line says what your reading unit was:

| You received | Your minimum unit |
|---|---|
| a diff | the whole function it landed in |
| a fixed function | the file: what the same-kind siblings do (guard, in-flight flag, cleanup on switch) |
| a module that talks to the network | the whole route: which destination each function talks to, and what the screen shows when each fails |
| a fix that changes flight time | everyone who runs alongside that flight |
| a ported pattern | the destination route: a number that came along (cap, timeout, threshold) must be justified again here |
| the fix of a family defect | the branch: `git grep` of the symbol, with the count in the report |

A correction round answers two questions: (1) what did it change of identity or lifecycle, and
what started RE-EXECUTING because of it (a destructive cleanup/teardown demands the test that
re-executes during the operation); (2) what stopped re-executing (something removed from a
condition or dependency list demands proof of the path it served, the whole outcome). In the
recipe, removing the trap beats erasing the symptom: a destructive routine cannot depend on its
caller's identity.

## Does the test prove the scenario, or itself?

Break the code on purpose and watch the test fall, on every new test a step or recipe demanded:

1. Prepare a disposable copy of the object as `protecao.md` says, outside the protected
   checkout. Remove it when done.
2. Remove the fix's lines, one at a time, and run the suite.
3. Only the new test fell: it proves the scenario. Nothing fell: that point has no test, a
   NOTED gap.

- Asking the executor to run the mutation does not replace you running it.
- A harness proves a deterministic race inside our code (order of effects, a poll, an
  out-of-order event). A defect that depends on something OUTSIDE our code emitting the event
  (platform, browser, OS: native network error, permissions, keyboard, camera) is proven in the
  real environment, never by a mock.
- The fixture is never the world where the defect is invisible: "the dead one disappears" uses
  a different living one, never an empty world.
- Nor a world the server never produces: check every fixture supporting an assertion against
  real data (a `curl` on the route, a row from the table). A corrected fixture enters the same
  round as the fix it locks.

### Before accepting a sabotage battery

1. Does the cut isolate the path the claim names? A cut in shared code (error mapper, common
   `catch`, helper) proves nothing specific. True claim with incomplete proof: complete the
   evidence, do not reject.
2. Does each cut say which test file accused? Single denominator across the battery. A cut that
   crashes the process runs filtered to the accusing test.
3. Does each cut come paired with a control that stays green? A control that stays green under
   the cut is the right design.
4. Was a blocker about ABSENCE ("no test", "no caller", "field doesn't exist") measured by two
   paths? Write which question the search asked, redo it by a second path.

A recipe that installs a LOCK (required prop, tighter type, a lint) is delivered only with the
verification red without the fix and green with it, both on disk; read both. When prescribing a
lock, run the verification with the change applied before writing the step count.

## Live proof measures what is served

- Match the identifier of the artifact you built with what the page loaded (bundle hash, file
  date). Building is not proof.
- Find out what the port serves (the command the service runs). A long-lived backend serves the
  code from when it started: check its start against the commit, or bring up your own instance
  on another port; never restart the user's service.
- Check both mechanisms in every report: a port serving a precompiled build, a service worker
  serving its cache. The concrete recipe is the repository's, in the group's rules file.
- Before opening the browser, compare the expressions side by side, term against term.

## Measure in both hosts and both states

- A screen that exists in two hosts (phone and desktop, panel and modal) is measured in both;
  the report says at which width each number was taken.
- Measure in the same state of the neighboring region where the original number was taken
  (list scrolling, neighbor open or closed), and note the state next to the number.
- Behavior proof goes to the outcome ("connected", "saved", "opened"), not the state before it.

## Report line

```
Unit: <the minimum unit from the table, and what you read at it>
Broken on purpose: <each new test: line removed → test that fell | "no new test" | "not run: waived by <who>">
```
