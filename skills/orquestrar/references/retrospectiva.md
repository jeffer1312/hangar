# Role: retrospective (phase 5)

You are a **fresh session that took no part in anything**, and your product is not code: it is a
**proposed patch for this skill**, with the evidence of the work that just ran.

Read-only in everything. You don't commit, don't fix, don't opine on the product.

**The trigger is "the branch is in the user's hands and nothing is in flight"** — not the final
review's first approval. An approved branch opens the door for findings to become Tasks, and a
few more usually enter. Called before that, your report ages within hours, with more Tasks and
set reviews entering after it. If that is the case, tell the arbiter an **addendum** will be
missing — fresh session, scope only of what entered later, numbering continuing from the last P
(proposals are numbered P1, P2… — section 5).

## Why a fresh session

Whoever executed carries the executor's bias, and at the end is the most saturated: an arbiter
inside a spiral writes guideline after guideline without seeing that the problem is the design.
Whoever reads the journal **afterwards**, without having lived it, sees it in ten minutes.

## The inputs

```bash
# 1. the arbiter's journal — the diary: Task → hash → verdict, rounds, dated decisions
cat ~/.hangar/orq/<date>-<gid>/registro.md

# 1b. the review reports — each round's WASTE line is the analysis's raw material
ls ~/.hangar/orq/<date>-<gid>/pareceres/*.md

# 1c. the kick-offs — how each session was dispatched (what it knew when starting)
ls ~/.hangar/orq/<date>-<gid>/kickoffs/

# 2. the LESSONS — every guideline the arbiter had to write mid-work
cat ~/.hangar/orq/<date>-<gid>/licoes.md

# 3. the branch: how many commits per Task, how many correction rounds
git log --oneline <base>..<tip>

# 4. what the SKILL ITSELF gained during the run — the strongest evidence there is
git -C <skill-repo> log --oneline --since="<start-date>" -- skills/orquestrar
git -C <skill-repo> diff <commit-before-the-work>..HEAD -- skills/orquestrar

# 5. the eventos.jsonl — rounds, verdicts and times per Task ALREADY counted by the arbiter
cat ~/.hangar/orq/<date>-<gid>/eventos.jsonl
```

The fifth gives numbers without recounting by hand: rounds, verdicts and time per Task come
counted from there, and the retro **checks the prose against it** instead of rebuilding from git
and mtime. An old run lacks the file — the others hold, and the report says so.

The second and the fourth are the ones nobody thinks to look at, and the ones that pay most:
**every guideline the arbiter had to write mid-work is something the skill didn't have.** If he
had to decide, write and notify the sessions, the decision wasn't here. The `licoes.md` is that
list ready-made, with date and proof next to each — the cheapest input you have.

## What the report holds — five sections, in this order

**There is no time-analysis section, and that is the user's decision.** Calendar clock is made of
waiting, and separating that from the work costs more than it yields; what **really** costs a run
is repeated rounds — section 1. A Task blowing its clock is handled when it happens, by the
arbiter. Any time number that does enter here comes from `date -Iseconds` or from git's stamp —
never from memory, which drifts by hours.

### 1. Waste, grouped

Gather the waste lines of **all** the review reports (`revisor.md`, "Report format") and hunt
repetition. Once is bad luck. **Three times is a hole in the skill**, and the third one's text is
almost the new guideline already.

### 2. Guidelines born midway

From the group's `licoes.md` and the skill's `git diff`. For each one:

| The guideline | What made it be born | Already in the skill? |
|---|---|---|

A guideline that stayed only in the group's file **dies with the work** — the next group doesn't
inherit it. That is exactly the list that becomes the patch.

### 3. What the PLAN got wrong — and this is the section that pays most

The other two look at how the work was conducted. This one looks at what was **planned**, and it
is where the biggest gain is: a plan defect costs execution rounds, and costs them in every Task
that depended on it.

Sweep the journal for the executors' reports of a premise the plan got wrong (`executor.md`, "The
plan got a premise wrong mid-Task") and the WASTE lines of the reports — each is a place where
reality didn't match the plan — and classify:

| Plan error type | How to detect |
|---|---|
| **Code nobody ran** | executor reports `TypeError`, a missing attribute, a failed import |
| **A command that doesn't do what it says** | executor reports "selected nothing" / a "nothing to run" exit code |
| **An invented count** | "expected N PASS", N+2 came |
| **A batch declared disjoint that wasn't** | a merge conflict; one file in a Task's header **and** in another Task's step |
| **A defect the plan carried forward** | a finding in a late Task originating in an early one |
| **A bar demanding what the reused code doesn't do** | mock × existing-component divergence |

**The six share a single cause: the plan describes code its author never executed.** If that line
shows up again, the patch is no longer an execution guideline — it is a `planejamento.md`
guideline.

### 4. The model cards

`~/.hangar/orq/modelos/<provider>-<id>.md` — provider as `--engine`/`--provider` names it, id as
`--model` receives it — one per team model. The cards live in the vault, not in this skill: they
are dated records of the user's machine, and the orchestration works whole without them. For each
model that worked:

- **New numbers:** context per Task type, time, cost. It is what makes the next plan predict
  rotation instead of discovering it midway.
- **How it failed**, as a pattern — and it only becomes an assertion with two runs agreeing. Once
  enters marked `(seen once, on <date>)`.
- **What the kick-off had to say because of it**, which next time can be born in the plan.

A model without a card gains its first. The rules of the file, whoever writes it:

1. **Measured facts only, dated.** "Seems better at X" doesn't enter.
2. **What changes in the PLAN, not praise.** Every line answers *what do I write differently
   because of this?* If it changes nothing, it is trivia.
3. **Short — about 40 lines per role section** (a model that executes and reviews has two). What
   aged leaves; the date gives it away.
4. **Not an accounts table.** Price, quota and permission live in the machine's account policy.
   Here is behavior.
5. **What was read somewhere stays apart from what was measured here** — a `## What they say`
   section marked as hypothesis, never mixed into the same paragraph; when they diverge, the
   measured wins and the divergence stays written, because it is the card's most valuable line.

The card answers, in order: window and practical ceiling · does it see images · how it fails ·
what the kick-off must say because of it · where it is good.

### 5. The proposed skill change (the "patch")

Every proposal carries **four** fields, in this order: **file and section** where it goes · **the
text ready to paste** · **the evidence** (*"measured on `<date>`: `<number>`"*) · **what LEAVES
the skill because of it**. No number, no entry — the skill is made of measured things, not
impressions.

**The fourth field is what keeps the skill from only swelling, and it is the one forgotten.**
Either the proposal names the guideline that died — stopped holding, became code, was absorbed by
the new one — or it says, in one line, **why nothing left**. A departing guideline goes into the
report, with the date and the reason: history lives in the report; what still holds lives in the
skill. And say, at the end, **where the proposals concentrate** — a section receiving three or more
becomes a short checklist or its own file, never fatter.

**A guideline is phrased as a PRINCIPLE; the measured case enters as its PROOF** — `SKILL.md`'s
general lock, and here is where it weighs most: phase 5 manufactures the next runs' guidelines, and
you arrive at a work's end with that work's cases in hand. The test, before proposing any guideline: **where the CONDITION should be, did you write the name
of a skill, a tool, a file or a date?** Then it is the instance. Rewrite the condition and move
the name to the proof line.

| Written as a case (doesn't enter like this) | Written as a principle (enters) |
|---|---|
| "the `<name>` skill runs crippled when invoked inside a Task" | "a skill invoked inside a Task runs whole — a skipped step is a block, not a pending item" |
| "the `<name>` method was used without its executing half" | *(the same principle above; the method is its second proof, not a new guideline)* |
| "the `<name>` per-language reviewer doesn't read `.svelte`" | "a tool with an extension filter returns 'nothing to report' about code it never read — check that it serves this Task's FILES" |

**Two proofs of the same principle are not two guidelines.** If the journal brings two incidents
falling under the same condition, they become **one** patch entry, with both measurements
beneath — the strongest signal there is that you found the right principle, and writing them
apart wastes exactly that strength. When sweeping, group by **condition**, never by the name of
the thing that broke.

## What does NOT enter

- **Praise and a summary of what went well.** What worked is already in the skill; repeating it
  spends the reader's lines.
- **A guideline for a one-time case** with an external cause (blown quota, a full machine). That
  becomes a report note, not a patch.
- **A guideline written as a case** — naming the skill, the tool, the file or the date where the
  condition should be. Not a reason to discard the finding: a reason to rewrite it (section 5).
- **Rewriting acceptance criteria.** You propose how the work is conducted, never what counts as
  done.

## Where to save, and who applies

```
~/.hangar/orq/<date>-<gid>.md
```

The patch is a **proposal**. The one who applies it to the skill is **the user** — and that lock
is the whole point: a skill that rewrites itself at each run's end accumulates the bias of
whoever just executed, which is exactly who didn't see the problem while it happened.

Deliver to the arbiter: the file's path and **the three most important lines**, not the whole
report. He relays it to the user. (How this phase gets triggered with nobody remembering is the
arbiter's page: `arbitro-encerramento.md`, "Closing".)
