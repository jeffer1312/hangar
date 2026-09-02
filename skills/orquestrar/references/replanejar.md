# Replanning midway: rewriting the plan and the contract without discarding what already landed

Not a fixed role — a procedure, triggered in the middle of phase 3, that runs phase 1 **again,
smaller, only over what remains**. It exists because a plan can stop deserving trust mid-flight —
an abandoned method, stuck Tasks, guidelines the plan doesn't know — and patching Task by Task
throws rounds away.

## Triggers — the user fires it, or the arbiter proposes

- **The user ordered it.** ("I don't trust this plan to finish")
- **A central premise fell**: a recorded decision of the plan proved false in execution, and more
  than one future Task depends on it.
- **A lame method**: the plan was born in a method whose executing half doesn't exist on the
  machine, or the user decided to switch methods — a method switch happens **only** through here
  (`SKILL.md`), never by patching.
- **A Task stuck BEFORE the gate.** Two consecutive Tasks past 2× the estimate **for the same
  cause** is a signal — and it is the **late** signal, because it counts rounds, and a Task stuck
  before its first round produces no round to count. The trigger that fires in time is simpler:
  **a Task with over 3 hours since its `task_inicio` and no `entrega` line in `eventos.jsonl`.** A
  Task stuck before its first round produces no verdict and no spiral rule to fire.

The arbiter **proposes** ("replan, or keep patching? cost so far: X"), the user decides. The
arbiter doesn't replan on his own — and **doesn't rewrite his own plan**: whoever plans again is
a session with context clean of the conductor's bias.

## Who rewrites: a REPLANNER — a fresh session, with the user

Like phase 1: the replanner works **with the user**, and the product only counts with their "go
ahead". The session is new (or the user themselves in a planning session); the arbiter delivers
the inputs and **freezes the group** meanwhile (no new Task opens; a Task in flight finishes or
is suspended with its state committed).

The replanner reads, in this order:

1. **What is already on the base** — the branch's `git log`: merged Tasks are facts, not
   options.
2. **What is in flight** — a commit without a merge, a worktree with an uncommitted diff: each
   becomes an explicit decision in the new plan (adopt, review, discard), never limbo.
3. **The contract and the lessons** (`regras-<gid>.md`, `licoes.md`, the journal) — the
   guidelines the run fixed enter the new plan as a starting point, not as a discovery to
   repeat.
4. **The review reports** — each round's waste line is the map of what the old plan got wrong.
5. **The `eventos.jsonl`** — real clock and rounds per Task, already counted; where missing,
   whatever `git log`/transcripts give.
6. The old plan, last — to inherit what still holds, not to defend it.

## What the new plan is

- **It covers ONLY the remaining work.** Merged Tasks become a `## Base (previous phase)`
  section — facts, with hashes — and **are not renumbered**: the progress bar and the old
  reports cite the old numbers.
- **It is born in the contract's method** (default `superpowers`) — whole. If the replanning is a
  method switch, the new plan is born 100% in the new method; a mixed format is the defect the
  `Method:` line exists to prevent. Contract with `Method: none` → what is born anew is the
  **orchestration plan** (`planejamento.md`), pointing at the user's plan as always.
- **A new file**, named after the work + `fase-final` (or `v2`), next to the old plan — which
  gains, at its top, a notice pointing at the new one and **is never deleted**: the reports cite
  it.
- **It passes the SAME phase 1 exit gate** (`planejamento.md`, the checklist) — replanning is no
  shortcut: every item again, now with the previous phase's REAL numbers as the estimate's base.
- **The team becomes a question again.** Like an off-plan Task (`arbitro.md`): the remaining work
  may be of another nature than the original team. Propose with the history in hand; the user
  chooses.
- **The branch becomes a question again** (`planejamento.md`, phase 2) — including "we stay
  where we are", a legitimate, recorded answer.

## The contract follows — rewritten, not patched

New plan approved:

1. The arbiter (the previous phase's, or the replanner taking over — the user's decision,
   recorded; taking over is an arbiter succession and runs its rite, `arbitro-encerramento.md`)
   **rewrites `regras-<gid>.md` from scratch** from the phase-2 skeleton, pointing at
   the new plan. A live guideline of the previous phase enters; a dead Task's guideline becomes
   one line in the journal.
2. **The journal remains the same file** (`~/.hangar/orq/<date>-<gid>/registro.md`): a dated entry marks the replanning
   — reason, what died of the old plan, the base's hash — and the diary continues.
3. **Every kick-off from then on points at the new plan and rules.** A live session of the
   previous phase that continues receives a new kick-off; one that doesn't fit the new team is
   retired with the usual rite (transcript read, work recovered).

## Replanning FORESEEN in the plan — this procedure's miniature

A plan may declare that a Task's recipe only closes mid-execution ("Task N depends on what Task
N-1's measurement proves"). That is legitimate — and it is **planning, not conducting**: the one
who closes the recipe is the **planner** session (or a fresh planning session, with the spec and
the measurement document attached), never the arbiter by gravity. The arbiter delivers the
inputs, receives the closed recipe and excerpts it into the kick-off, as with any Task. And the
recipe closed midway passes the phase-1 step format (the three request outcomes, the
who-types trigger — `planejamento.md`), because it IS plan.

An arbiter closing that recipe without the planning context leaves exactly the planning-shaped
gaps — a WHEN nobody declared, an outcome nobody named — and those become the most serious class
of blocker. The miniature doesn't demand the 13-item gate again: it demands only the right owner
and the step format.

## What replanning is NOT

- **Not an audit of the execution** — that is the retrospective (phase 5), still at the end.
- **It doesn't reopen approved Tasks.** A defect in a merged Task is a review finding (set or
  final), which becomes a new Task in the new plan through the normal cycle.
- **Not a license to decide team, account or branch** — all three remain the user's.
- **It doesn't repeat as routine.** Two replannings in the same work = the problem is not the
  plan; stop and discuss the work itself with the user.
