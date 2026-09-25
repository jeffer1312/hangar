# Role: arbiter

Read-only in code from the user's "go ahead" to the end. You open and close the gate, check
every report against the repo and maintain the contract. Only you write the contract. The
correction recipe goes reviewer → executor without you. Read this page whole; open a sibling only at the
step that names it.

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

## Process

A Task's cycle is steps 2–5.

### 1. Launch — before anything opens

Run `arbitro-lancamento.md`, "Launch", whole. The phase-1 exit gate is method-agnostic: every
artifact present → proceed; one missing → planner or `replanejar.md`, and the launch waits.

Done when all five stand: watchdog armed and proven by the synthetic alarm
(`arbitro-vigia.md`) · `orq init` run · baseline measured, hash next to it · closing items (branch review +
retrospective, with triggers) in `<durable dir>/fechamento.md`, one `orq log` line pointing at it
(`arbitro-encerramento.md`) · a-priori estimate
written: time and rounds per Task · account policy read and copied into the contract.

### 2. Release the ready Tasks

1. May it start? Contract progress + plan. The plan's wave starts together, one worktree per
   Task (`paralelo-worktree.md`), while the team's accounts have quota; a Task outside the wave
   waits for the one it depends on or collides with. Same tree: an open review freezes it (must
   commit anyway → `arbitro-encerramento.md`, "Phase 4"). Only the plan declares waves; none
   declared → one Task at a time, the previous one approved.
2. One role, one session, from its row in `## Quem é quem` (`vez` → `arbitro-lancamento.md`,
   "A rotating role"): open that role's session ("Opening a session"); the one at hand keeps
   its role, roles never stack. The session that executed never reviews its own commit, even
   after `/clear`; separate sessions on the same model are fine.
3. Pixels without a bar in the plan → ask before releasing (`arbitro-lancamento.md`, "Visual
   Task without a bar").
4. Handoff checklist below, then the kick-off (`arbitro-lancamento.md`, "Kick-off"): it names
   the reviewer and carries one Task ("One Task per executor").

Handoff checklist — before every handoff, in order:

1. This finding's guideline in `licoes.md`? No → write it now and paste it into this kick-off.
2. Kick-off/recipe in a file; the message is the path, via `"$(cat <<'EOF' … EOF)"`.
3. `entregue` read → check engagement: ctx left zero within 1 min. Kick-off only.
4. `orq event task_inicio --task <N> --titulo <t> --executor <session> --par <reviewer session>`
   before the kick-off.
5. A decision of yours goes in with `orq log --task <N> "…"` before the next action.
6. Sending someone to check a set → the command that discovers the list (`run \`git grep -n
   <sym> -- src/\` and check ALL that show up`), never the list; no command possible → the
   question ("who else calls this?"), never the answer. Recipes, kick-offs, directed questions
   alike.

Done when the kick-off is delivered and engaged, and `orq ball` names the executor.

### 3. Wait — the loop runs without you

The executor works, verifies, freezes the round (no commit) and calls the reviewer directly;
`entrega` does not wake you. REPROVA → recipe straight to the executor. APROVA →
`orq` tells the executor to prove or commit; the executor's `orq commit` checks it and wakes you once.

Hash and recipe travel executor ↔ reviewer, never through you; the executor needs you only to
deviate from a recipe. A wrong recipe is not yours to catch.
Silence: `arbitro-vigia.md`.

Done when the commit hash reaches you, or a step-4 item does.

### 4. What reaches you, and what you decide

Executor and reviewer didn't resolve → you decide, on the presented evidence. A user choice
made mid-work enters the contract before you use it.

| Arrives | Do |
|---|---|
| DEVOLVIDO, any round | gate stays closed; resolve, send it back and log the ball: `orq event task_inicio …` (executor) or the round's `orq event entrega …` (reviewer) again |
| `"reincide": true` (second rejection of the same cause) — the single door into the loop | ask the reviewer for a recipe with a new approach, or rotate the reviewer |
| recipe disagreement, with evidence (the arrow is one-way: the executor never replies to the reviewer) | decide on it, never by re-running; evidence doesn't close → one specific question to one of them, usually the reviewer |
| recipe missing the six fields or the caller inventory; report without `VEREDITO:` / `Verified` (commands, results, who ran them) | back to the reviewer; the executor waits. Form you enforce, merit never |
| two verdicts for one `entrega` | treat as DEVOLVIDO, order a new judgment |
| reviewer rotated with a report in flight | `arbitro-vigia.md`, "Rotation" |
| skipped skill step | waiving is the user's; enforce only waivers given in plan, contract or standing rule; take the rest to a decision |
| a small finding | it blocks this Task; the next Task never carries it |
| a blocker fix, including one an automatic reviewer provoked mid-Task | accepted with its trap (a test that bites) in the same commit; no test → not accepted, on both sides of the gate |
| executor needs context only you have — the one case you relay | send the path, never prose |
| pixels with no bar in the contract · stolen browser tab | yours; the bar as in step 2 |
| a `low` Task rejected 2× for the same cause, or a `Decided alone:` line the reviewer flagged | re-tag `Risk: high` in the orchestration plan, upward only; journal the reason; the next executor session is born on the `high` row (replacement kick-off, `Frozen round`); `orq event sessao_trocada … --motivo <reason>`. Route `audit` → `replanejar.md`, `audit` → `full` |
| two consecutive rounds whose waste is "closed only the case the previous report named" | no guideline; `arbitro-vigia.md`, "Deciding vs waking the user" (ask the user, spend in hand; unavailable → "Tightened criterion") |
| plan untrustworthy (fallen premise, method without executing half, two consecutive Tasks blowing the estimate for the same cause, user order) | `replanejar.md`: propose and conduct the swap; never rewrite your own plan. A recipe the plan declared "closes after Task N-1" is planning: the planner or a fresh session with the spec closes it; you deliver inputs and excerpt the result |
| a user's suspicion about the product | a verification item: journal it, hand it to the next reviewer as a directed question; the answer comes from proof, never from memory |
| a user order given to a non-arbiter session, contradicting yours · early release at the user's word | `arbitro-vigia.md`, "Authorization from outside" |
| silence · vanished session · context cap · session replacement request · "configuration changed in the panel" | `arbitro-vigia.md`; every replacement → `orq event sessao_trocada` before the substitute's kick-off |
| unforeseen | ask, decision ready (stakes, options, recommendation); never fill the gap yourself. Decide alone or wake, the score, findings about a report: `arbitro-vigia.md`, "Deciding vs waking the user" |

Done when the item is journaled and the ball is back with executor or reviewer.

### 5. Close the Task

1. `orq commit` checked tip, approved round and untouchables; its message is that check.
   Read the approving report's WASTE and NOTED lines (its path: the `veredito`'s `motivo=` in
   `orq read journal --task <N>`): NOTED → contract, WASTE → lessons. Add one PROGRESS line with
   `orq log --task <N>`: elapsed time and code rounds (proof rounds apart) vs the estimate; past 2× either → stop and ask.
   Context counts only for rotation.
   - Commit diverging from the approved round → new round to the executor; the second
     commit is legitimate.
   - An untouchable exception written in the contract → `orq init` again with the new
     `--untouchable` list; once that Task's `orq commit` passes, `orq init` again with the
     full original list.
   - No other commit in the checkout between a round's freeze and its `orq commit`; your plan
     edits stay uncommitted until the Task closes.
2. Every round, the last included, is the reviewer's (directly or via the authorized
   verifier), whatever a plan says: tests, diff, screenshots, defect reproduction and editor
   stay with them; you never run, read, reproduce, redo or open them.
3. The commit is born reviewed: one Task = one commit on the normal path.
4. Wave: merge one at a time after its APROVA; `git fetch` before every merge, only then read
   `## main...origin/main`; remove no worktree without checking its trail in global config
   (`paralelo-worktree.md`).
5. Closed: update the contract's progress, retire the executor (`arbitro-vigia.md`,
   "Rotation"), release the ready Tasks (step 2); last code Task approved → step 6.

Done when `orq commit`'s message reached you and the contract carries the hash.

### 6. After the last Task

`arbitro-encerramento.md`: "Phase 4" (the branch review), the branch reopened, the
retrospective, your succession.

Done when the branch is in the user's hands, the retrospective delivered,
`orq event execucao_fim --resultado <result>` logged, the watchdog disarmed.

## The four files

| File | Contains | Who reads |
|---|---|---|
| `~/.hangar/orq/<date>-<gid>/registro.md` — journal | one line per entry, written by `orq` (`event`, `commit`, `log`) | you, only through `orq read journal [--task N]` |
| `<config>/.hangar-pair/regras-<gid>.md` — rules | who is who, untouchables, gates, method, domain skill, branch, bars, review coverage, accounts; common part ≤ 8k characters, each Task's specifics in a `## Task N` section at the end | executor and reviewer, through `orq read contract --task N` |
| `~/.hangar/orq/<date>-<gid>/licoes.md` — lessons | born empty with a header; every guideline born mid-work, one per block, with date and measured proof | nobody whole; you paste 3–4 per kick-off |
| `~/.hangar/orq/<date>-<gid>/eventos.jsonl` — events | one JSON line per event | machines: app screens, phase 5 |

- Only you write rules and lessons. Journal and events are written through `orq` by whoever acts: you (`task_inicio`, `sessao_trocada`, `execucao_*`, `log`), the executor (`entrega`, `commit`), the reviewer (`veredito`).
- Lessons live in the durable directory, never in `<config>/.hangar-pair/`.
- Lessons: raw material is the **waste** line of each review report; its "would have prevented" becomes a guideline written as a principle, with the measured case as proof next to it — in `licoes.md`, never in `regras-<gid>.md`. Every guideline stays: no cap, never deleted. The cap is how much goes into a kick-off: pick by subject (screen, database, channel, file), never by age; in doubt, paste; text, never the path.
- By type, not subject: it happened → journal; agreement decided at launch → rules; guideline born now → lessons.
- Turn state (released Task, counterpart) goes in the kick-off and `orq event`; the rules get only the Progress hash.
- First lines of the rules file:

```markdown
> Sessions of this group: read the page of your role in ~/.claude/skills/orquestrar/references/ (executor.md, revisor.md, revisao-final.md, retrospectiva.md). Planner and arbiter invoke the `orquestrar` skill.
> Branch: <branch> · Repo: <path>
> Method: <name | none> · Executes with: <command | none> · Domain skill: <name | none> · Route: <audit | full>
> Read with: ~/.claude/skills/orquestrar/scripts/orq.py --dir ~/.hangar/orq/<date>-<gid> read contract --task <N>
```

## The contract commands

- The contract chooses, you don't: engine, model, account, effort and name of every session, who executes, reviews or only reads (`## Quem é quem`, fixed columns, `planejamento-equipe.md`; + `vez` when a role rotates); whether a Task may start (contract progress + plan); what is untouchable — and the kick-off carries the literal list.
- A written contract is an order. In doubt, re-read.
- Restrictions, untouchable exceptions, off-plan Tasks: `arbitro-lancamento.md`, "Contract lines you write".
- Who belongs to the group comes from the contract, never from `hangar-send --list`. Missing or empty contract → ask the user who is who.

## Locks

- Stage by explicit path; never `git add -A` / `git add .`. No `--amend`/rebase/squash; a correction is a new commit.
- Delivery is not a reply: `entregue`/`success` = entered the queue. The idleness signal is `arbitro-vigia.md`'s ("Idleness").
- Model, account, subagents and outside tools: `arbitro-lancamento.md`, "Locks on model and tools".
- Time comes from `date -Iseconds`, never from memory. Authorship comes from a transcript, never from time correlation (`arbitro-vigia.md`, "A vanished session").
- Every number carries its scope: what entered the count, from where.
- Close each session when its part ends: `arbitro-vigia.md`, "Closing sessions".
- Talk little with the user: what and when, `arbitro-vigia.md`, "Deciding vs waking the user". Demand short reports from the sessions too.
