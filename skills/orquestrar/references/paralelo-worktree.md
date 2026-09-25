# Exception: Tasks in parallel, one worktree each

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

- The default is serial: one writer per tree, the gate closing each Task before the next opens.
- Parallelize only when the Tasks are truly independent and the work is big enough to pay the setup.
- Prefer arms inside one tree first: the executor runs one subagent per disjoint file set,
  verification once after the join (`executor-subagentes.md`). A worktree only for a Task that
  justifies a whole session of its own.

## The trigger — four conditions, all together

- The planner audits them in phase 1, with the user, by exit-gate item 3 (`planejamento.md`):
  files per Task × `git merge-tree`, output pasted; files from the steps' text, or from the repo
  via subagent. A written declaration of independence does not replace the audit. The arbiter
  does not deduce it later.
- New repo: audit condition 3 on the design (who creates what, who consumes), not on the disk.

1. **Disjoint files.** No file in two Tasks of the batch. Check in the steps' text, not the Task
   header. Single exception: a purely additive shared file with a declared insertion discipline
   (below).
2. **No symbol crosses.** Nothing Task A creates or modifies is consumed by Task B. Ask who
   consumes each file, not only who writes it.
3. **No shared state.** Store, module singleton, registry, cache, table: two Tasks mounting hosts
   of the same state are not independent, whatever the files.
4. **Isolated verification.** Each Task's verification runs alone, in its worktree.

- One fails → that Task returns to the serial queue.
- Batches of two or three.
- Trigger passed → weigh this repo's setup cost (below) before deciding.

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
- The port table per Task goes in the plan. A visual Task in parallel, in doubt: serialize.
- 2+ visual Tasks in a batch: the plan declares either a browser instance per executor (separate
  profile/port) or visual proof as a critical section (one captures at a time, through
  `orq screen take`/`release`, `executor.md`). The executor checks the tab before every capture regardless (`executor-visual.md`,
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

- Merge conflict = the Tasks were not independent. Stop; don't resolve. The losing Task becomes a
  new serial Task on the merged base, with its executor. Exception: a positional conflict in a
  file the plan declared additive — the arbiter resolves it at the merge by merge strategy and
  proves it by content (key counts on each side before and after, zero values changed); never
  returns it to the executor.
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
| "The plan is big, so parallelize" | Size is not independence. The four conditions, or serial. |
| "The files are disjoint, so they're independent" | Condition 3. |
| "Only `types.ts` is touched by both" | One shared file leaves the batch. |
| "I'll resolve this little conflict" | Read-only. A conflict is a new serial Task. |
| "Both passed, merge both and verify at the end" | Verification after each merge. |
| "It has its `APROVA`, no need to re-verify after the merge" | `APROVA` means right alone. |
| "I'll leave the worktree, clean up later" | Trail check, then remove, before the batch closes. |
