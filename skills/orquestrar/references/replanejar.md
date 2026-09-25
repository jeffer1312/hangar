# Replanning midway

A procedure, not a role: phase 1 runs again, smaller, only over what remains. It is the only door
for switching methods and for escalating the route.

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

## Triggers

- The user ordered it.
- A central premise fell: a recorded plan decision proved false in execution, and more than one
  future Task depends on it.
- A lame method: the executing half does not exist on the machine, or the user decided to switch
  methods.
- A Task stuck before the gate: over 3 hours since its `task_inicio` with no `entrega` line in
  `eventos.jsonl`. Late signal: two consecutive Tasks past 2× the estimate for the same cause.
- Route escalation `audit` → `full` (`planejamento.md`).

The arbiter proposes ("replan, or keep patching? cost so far: X"); the user decides. The arbiter
never replans on his own and never rewrites his own plan.

## Who rewrites: a replanner — fresh session, with the user

- A new session, or the user in a planning session. The product counts only with the user's "go
  ahead".
- The arbiter delivers the inputs and freezes the group: no new Task opens; a Task in flight
  finishes its round or stops as a frozen round (`executor.md`) — never a dirty tree in limbo.
- The replanner reads, in this order:
  1. What is on the base: the branch's `git log`. Merged Tasks are facts.
  2. What is in flight: a commit without merge, a worktree with an uncommitted diff, a frozen
     round. Each becomes an explicit decision in the new plan: adopt, review, discard.
  3. The contract, the lessons and the journal (`regras-<gid>.md`, `licoes.md`,
     `orq read journal --last 1000`): guidelines already fixed enter the new plan as a starting
     point.
  4. The review reports: each round's waste line.
  5. `eventos.jsonl`: real clock and rounds per Task; where missing, `git log` and transcripts.
  6. The old plan, last: inherit what still holds, don't defend it.

## The new plan

- Covers only the remaining work. Merged Tasks go in a `## Base (previous phase)` section, with
  hashes, and are not renumbered.
- Born whole in the contract's method. Method switch → 100% in the new method; no mixed format.
  `Method: none` → a new orchestration plan (`planejamento.md`) pointing at the user's plan.
- A new file, `<work>-fase-final` (or `v2`), next to the old plan. The old plan gets a notice at
  its top pointing at the new one and is never deleted.
- Passes the same phase 1 exit gate (`planejamento.md`), every item, with the previous phase's
  real numbers as the estimate's base.
- The team becomes a question again: propose with the history in hand; the user chooses.
- The branch becomes a question again (`planejamento-equipe.md`, phase 2); "we stay where we are" is a
  valid recorded answer.
- The route may escalate here (`audit` → `full`), never downgrade.

## The contract follows — rewritten, not patched

1. The arbiter (the previous one, or the replanner taking over — the user's decision, recorded;
   taking over is an arbiter succession, `arbitro-encerramento.md`) rewrites `regras-<gid>.md`
   from scratch from the phase-2 skeleton, pointing at the new plan: common part ≤ 8k
   characters, each Task's specifics in a `## Task N` section at the end, read through
   `orq read contract --task N`. A live guideline of the previous phase enters; a dead Task's
   guideline becomes one `orq log` line.
2. The journal stays the same: one `orq log` line marks the replanning — reason, what died of
   the old plan, the base's hash.
3. Every kick-off from then on points at the new plan and rules. A continuing session gets a new
   kick-off; one that doesn't fit the new team is retired with the usual rite (transcript read,
   work recovered).

## Replanning foreseen in the plan — the miniature

- A plan may declare that a Task's recipe closes mid-execution ("Task N depends on what Task N-1's
  measurement proves").
- The planner session (or a fresh planning session, with the spec and the measurement document)
  closes the recipe — never the arbiter. The arbiter delivers the inputs, receives the closed
  recipe and excerpts it into the kick-off.
- The closed recipe follows the phase-1 step format: the three request outcomes and the WHEN of
  each call (`planejamento.md`).
- The miniature does not repeat the exit gate: only the right owner and the step format.

## What replanning is not

- Not an audit of the execution: that is the retrospective, phase 5.
- Does not reopen approved Tasks: a defect in a merged Task is a review finding that becomes a new
  Task in the new plan.
- No license to decide team, account or branch: all three stay the user's.
- Not routine: two replannings in one work → stop and discuss the work itself with the user.
