# Exception: Tasks in parallel, one worktree each

**The default is serial.** One writer per tree, the gate closing each Task before the next
opens. Don't change that because the plan is big — change it because the Tasks are **truly**
independent and the work is big enough to pay for the setup.

Before thinking of worktrees, remember the executor already parallelizes inside **one** tree:
one arm (subagent) per disjoint file set, all at once, verification once after the join
(`executor.md`, "Your arms"). That delivers almost all the time gain with **zero** merge risk,
because a second base never exists. A worktree only pays off when the Task is big enough to
justify a **whole session** of its own.

## The trigger — the four conditions hold together

**Who answers the four is the planner's AUDIT, not the plan.** No method is obliged to deliver
this written — the planner surveys it themselves, with exit-gate item 3's command
(`planejamento.md`): files per Task × `git merge-tree`, output pasted — from the steps' text when
the material declares them, **from the repo, via subagent, when it doesn't**. A written declaration
of independence can be false; the audit is phase 1, with the user, and the arbiter doesn't deduce
it later.

**A new repo doesn't waive the audit — it changes where it looks.** In existing code, `grep`
finds the retained singleton. In a from-scratch project, the shared state isn't on disk yet: it
will be **created by the batch's own Tasks** (the conversation store, the socket connection, the
HTTP client with session renewal). So condition 3 is audited on the **design** — who creates
what and who consumes — not on the repo. Nothing here brakes a new project: the four conditions
pass more easily on a greenfield, and the command exists precisely so they are answered fast.

1. **Disjoint files.** No file appears in two Tasks of the batch. Not "almost" nor "just
   `types.ts`" — one shared file is serialization coming back through the back door, with a
   merge in the middle. **The single exception is a purely ADDITIVE shared file** (a catalog, an
   exports index) whose insertion discipline the plan declares — see "A shared ADDITIVE file",
   below; any other shared file pulls the Task from the batch.
   **Check in the STEPS, not the Task's header.** That is where collisions hide: a Task's header
   doesn't cite the file while one of its steps orders editing it — the "no file in common"
   declaration is written and false.
2. **No symbol crosses.** Nothing Task A creates is consumed by Task B of the same batch. If B
   expects a function A is still writing, B works against a void.
3. **No shared STATE.** Store, module singleton, registry, cache, table: two Tasks mounting
   hosts of the same state are not independent however disjoint the files, and the collision
   **doesn't show at the merge** — it shows as review rounds, where it costs most: one host
   writing into state the other clears, reads or deletes.
4. **Isolated verification.** Each Task's verification command runs alone, in its worktree,
   without depending on what the others did.

One failed → that Task leaves the batch and returns to the serial queue. Batches of two or
three; above that, integration becomes the bottleneck and you lost the gain anyway.

**Passing the four is the trigger, not the end of the account.** What decides is the trigger
**plus** this repo's setup cost (the "The real cost" section, below): an environment per
worktree, ports, the single browser, shared hooks. A batch that passes the four and is cheap to
set up is the right path — the page exists to be used, not avoided.

## What does NOT change

- **One writer per tree still holds** — now there are N trees, each with one writer. The rule
  was never "one writer per work".
- **The per-Task gate is unchanged.** The reviewer reviews a hash at that Task's branch tip. For
  them it changes less than in serial: their tip doesn't move under them.
- **The arbiter stays read-only in code.** A clean merge is mechanical and is his. A conflict is
  not — see below.
- Untouchables, staging by explicit path, no `--amend`, the six-field recipe: all the same.

## The recipe

```bash
BASE=$(git rev-parse HEAD)          # the SAME base for all — note it in the contract
git worktree add /path/wt-t2 -b <work>-t2 "$BASE"
git worktree add /path/wt-t3 -b <work>-t3 "$BASE"
```

One executor session per worktree, created by the usual recipe (`arbitro-lancamento.md`,
"Opening a session"). Each kick-off carries **its worktree's path** as the repo, its branch, and
`Expected HEAD` = `$BASE`. Getting that wrong is one session working in another's tree.

The contract records the batch: which Tasks, which `$BASE`, each one's worktree and branch, and
the merge order.

## Integration is the arbiter's, and it is mechanical

One branch at a time, **only after that Task's `APROVA`**:

```bash
git merge --no-ff <work>-t2
# the plan's full verification, here, now
```

Two rules close the design:

- **A merge conflict = the Tasks weren't independent.** The arbiter **stops and doesn't
  resolve** — resolving a conflict is writing code, and he writes none. The losing Task becomes
  a new Task, serial, on top of the merged base, with its executor. The conflict becomes a
  signal instead of work hidden inside the wrong role. **One exception**, and it is the same one
  condition 1 names: a positional conflict inside a file the plan declared **additive** is
  resolved by the arbiter at the merge, by the plan's rule, proven by content (below).
- **Full verification after EACH merge**, not only at the end. Red goes back to that Task's
  executor, **even with its isolated `APROVA` in hand**. Approved in isolation means "right
  alone", and that is exactly the gap parallel opens.

Post-merge red **returns to the full cycle**, just like a conflict: the executor fixes on the
main line and the **reviewer judges the fix before its commit**, as in any round — dirty tree,
frozen object, APROVA, only then the commit.

**While a fix is open on the main line, you STOP merging.** The fix keeps the tree dirty until its
APROVA, and a merge entering in that meantime mixes the two: `git merge` of non-overlapping files
passes without conflict, and the full verification you run after it would be running over
**unreviewed** code from another Task — a green that proves nothing, a red charged to the wrong
executor. A merge paused a few minutes is cheaper than a meaningless verification; the only thing
that closes a gate in this pipeline is the reviewer's `APROVA`.

Batch done: **the trail check first**, then `git worktree remove` on each. The trail check hunts
the worktree's path in every global configuration: `grep -rl "<worktree path>" ~/.local/bin
<agent config dirs> <service unit dir>` — once removed, the trail points at a path that no longer
exists and the damage goes silent. An orphan worktree is the next session working in a checkout
nobody explains.

## The gate parallel creates

In serial, each Task is reviewed on top of what the previous one did — the interaction enters
the gate for free. In parallel that vanishes: **phase 4's final review is the FIRST place the
Tasks meet**, over `$BASE..tip`, in a fresh session that took part in nothing. Record it in the
contract together with the batch, not later.

## The real cost, before you think it is free

Each worktree pays for its own environment (dependencies installed per tree), and parallel
visual Tasks raise servers that fight over the same ports. **The port table per Task goes in the
PLAN** — which ports each repo uses is the project's datum, not this skill's. A visual Task in
parallel, in doubt: serialize.

**And the ports are not enough: a single-tab automation browser is one per machine.** Two
executors capturing at the same time steal the page from each other, with no error at all — an
executor whose tab silently swapped to a neighboring Task's URL can spend hours asking the wrong
page whether its screen came back. A batch with 2+ visual Tasks: either **a browser instance per
executor** (separate profile/port, if the machine can take it), or **visual proof as a critical
section** — one executor captures at a time, the arbiter grants the turn. The plan declares which
of the two; the executor checks the tab before every capture regardless (`executor-visual.md`,
"3. Capture").

**A shared ADDITIVE file (an i18n catalog, an exports index) is the one exception to condition 1
— and the plan says HOW it is touched.** The conflict there is **positional by construction**
(both sides append at the end) and git doesn't resolve it even when one side contains the other.
So the plan writes the insertion discipline (each Task in its own block, in a declared order —
never at the end), and whatever positional conflict remains **is the arbiter's, at the merge**: he
proves it by **content** (key counts on each side before and after, zero values changed) and
resolves it by merge strategy — **never returns it to the executor**, who cannot resolve it on the
origin branch.

**A GLOBAL DEVICE resource is a critical section like the browser**: the device itself, the port
forwarding (which is the device's, not the session's) and the app's storage. The port table goes
in the plan; the forwarding is redone immediately before every capture; the executors may
negotiate the device among themselves, in time slots, without the arbiter in the middle. But **a
session that dies holding the resource becomes a silent deadlock** — a reviewer sits stalled
indefinitely waiting for a device held by a session that died. Two rules from that: **whoever
holds releases BEFORE closing their own work**, and the arbiter **looks at who holds what**
whenever someone goes idle with no apparent reason.

**Git hooks are shared, which is why a worktree does NOT run `git merge main`.** `.git/hooks`
holds for the main checkout and for every worktree, so a `post-merge` hook runs with the toplevel
being **the worktree** — and anything it installs globally (units, symlinks, builds) now points
into a tree that will be removed. **The one who integrates into `main` is the arbiter, in the main
checkout**, where the hook runs in the right place; and **an installer NEVER runs from inside a
worktree** — machine setup runs from the main checkout, always. Disabling a git hook is the user's
decision, not the team's way out.

## Rationalizations — all of them mean STOP

| Excuse | Reality |
|---|---|
| "The plan is big, so parallelize" | Size is not independence. The four conditions, or serial. |
| "The files are disjoint, so they're independent" | Shared state is not a file. Condition 3. |
| "Only `types.ts` is touched by both" | One shared file is a merge in the middle. It leaves the batch. |
| "I'll resolve this little conflict and move on" | You are read-only. A conflict is a new Task, serial. |
| "Both passed, I'll merge both and verify at the end" | Verification after each merge. Otherwise you don't know which one broke. |
| "It has its `APROVA`, no need to re-verify after the merge" | `APROVA` means "right alone". The interaction nobody has seen yet. |
| "I'll leave the worktree, clean up later" | An orphan worktree is the next session in the wrong checkout. |
