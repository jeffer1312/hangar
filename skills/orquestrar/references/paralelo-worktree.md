# Tasks in parallel, one worktree each

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

- The default is parallel: every Task passing the four conditions with the others of its wave
  runs at once, one writer per worktree, each with its own gate.
- Inside one tree, the executor still spreads disjoint file sets over subagents
  (`executor-subagentes.md`).

## The trigger — four conditions, all together

- The planner audits them in phase 1, with the user, by exit-gate item 3 (`planejamento.md`):
  files per Task × `git merge-tree`, output pasted; files from the steps' text, or from the repo
  via subagent. A written declaration of independence does not replace the audit. The arbiter
  does not deduce it later.
- New repo: audit condition 3 on the design (who creates what, who consumes), not on the disk.

1. **Disjoint regions.** Two Tasks of a wave share a file only in regions the plan names per
   Task (function, section, block), with untouched lines between them; checked in the steps'
   text, not the Task header. Insertions into a shared list go at distinct declared anchors,
   never both at the end (below).
2. **No symbol crosses.** Nothing Task A creates or modifies is consumed by Task B. Ask who
   consumes each file, not only who writes it.
3. **No shared state.** Store, module singleton, registry, cache, table: two Tasks mounting hosts
   of the same state are not independent, whatever the files.
4. **Isolated verification.** Each Task's verification runs alone, in its worktree.

- One fails → that Task goes to a later wave, after the Task it collides with.
- No fixed wave size: the arbiter releases the whole wave while the team's accounts have quota;
  the plan may set a lower `Lote máximo` with the user.
- Trigger passed → the plan carries the setup below per worktree.

## What does not change

- One writer per tree: N trees, one writer each.
- The per-Task gate: the reviewer judges the round at that Task's branch tip.
- The arbiter stays read-only in code.
- Untouchables, staging by explicit path, no `--amend`, the six-field recipe.

## The recipe

```bash
BASE=$(git rev-parse HEAD)          # the SAME base for all — record it in the contract
git worktree add /path/wt-t2 -b <work>-t2 "$BASE"
git worktree add /path/wt-t3 -b <work>-t3 "$BASE"
```

- One executor session per worktree (`arbitro-lancamento.md`, "Opening a session"). Each kick-off
  carries its worktree's path as the repo, its branch, and `Expected HEAD` = `$BASE`.
- Each batch executor closes its Task with `orq commit --task <N> --hash <hash> --repo <its
  worktree>`.
- The contract records the batch: Tasks, `$BASE`, worktree and branch per Task, merge order.
- The contract also records, with the batch: phase 4's final review over `$BASE..tip`, in a fresh
  session, is the first place the Tasks meet.

## The cost

- Each worktree carries its own environment (dependencies installed per tree).
- The port table per Task goes in the plan.
- Each wave with visual proof declares its screen in the plan:
  - `Tela: própria` — a browser per session (the Hangar desktop app's embedded browser, one per
    session, or separate `agent-browser` sessions), each worktree serving on its own port: the
    proofs run at once, with no lock.
  - `Tela: compartilhada` — one window serves all (a native app on one display): the proofs
    queue through `orq screen take`/`release` (`executor.md`).
  - The executor checks the tab before every capture regardless (`executor-visual.md`,
    "3. Capture").
- A global device resource (the device, its port forwarding, the app's storage) is a critical
  section: forwarding redone right before every capture; executors may negotiate time slots among
  themselves; whoever holds releases before closing their own work; the arbiter checks who holds
  what whenever a session idles with no apparent reason.
- Shared additive file (i18n catalog, exports index): the plan writes the insertion discipline —
  each Task in its own block, in a declared order, never at the end.
- Git hooks are shared: a worktree never runs `git merge main`; the arbiter integrates in the main
  checkout; an installer never runs from inside a worktree. Disabling a hook is the user's decision.

## Integration — the arbiter's, mechanical

One branch at a time, only after that Task's `APROVA`:

```bash
git merge --no-ff <work>-t2
# the merged Tasks' verifications, here, now
```

- Merge conflict → the regions touched. Don't resolve it yourself: the losing Task gets a
  correction round on the merged base, same executor — a new worktree from the merged tip, its
  approved diff as reference, only its own region redone. Exception: a positional conflict in a
  file the plan declared additive — resolve it at the merge by merge strategy and prove it by
  content (key counts on each side before and after, zero values changed).
- The merged Tasks' verifications after each merge. Red → back to that Task's executor even
  with its isolated `APROVA`: fix on the main line, reviewer judges before the commit — dirty
  tree, frozen round, `APROVA`, then the commit.
- While any round is open on the main line (a post-merge fix, or a serial Task beside the batch),
  stop merging. Git refusing the merge on a dirty tree is the rule. Only the reviewer's `APROVA`
  closes a gate.
- Batch done: trail check first — `grep -rl "<worktree path>" ~/.local/bin <agent config dirs>
  <service unit dir>` — then `git worktree remove` on each. No orphan worktree.

## Rationalizations — all mean STOP

| Excuse | Rule |
|---|---|
| "The plan is big, so parallelize" | Size is not independence. The four conditions, or a later wave. |
| "The files are disjoint, so they're independent" | Condition 3. |
| "Only `types.ts` is touched by both" | Condition 1: regions named in the plan, or a later wave. |
| "I'll resolve this little conflict" | Read-only. A conflict is a correction round for the losing Task. |
| "Both passed, merge both and verify at the end" | Verification after each merge. |
| "It has its `APROVA`, no need to re-verify after the merge" | `APROVA` means right alone. |
| "I'll leave the worktree, clean up later" | Trail check, then remove, before the batch closes. |
