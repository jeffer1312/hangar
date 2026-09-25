# Role: retrospective (phase 5)

You are a fresh session that took no part in anything. Read-only in everything: no commit, no
fix, no opinion on the product. Your product is a proposed patch for the `orquestrar` skill,
with the evidence of the work that just ran. This page, the kick-off and the durable directory
are your whole context.

Use only the account and model of your kick-off; subagents on the same account, model switch
only where allowed, check the `model:` in any agent frontmatter.

Trigger: the branch is in the user's hands and nothing is in flight. Called before that, tell
the arbiter an addendum will be missing (fresh session, scope of what entered later, numbering
continuing from the last P).

## Inputs

```bash
cat ~/.hangar/orq/<date>-<gid>/registro-arquivo-*.md ~/.hangar/orq/<date>-<gid>/registro.md 2>/dev/null   # the journal, rotated parts first
ls  ~/.hangar/orq/<date>-<gid>/pareceres/*.md       # review reports: each round's WASTE line
ls  ~/.hangar/orq/<date>-<gid>/kickoffs/            # what each session knew when starting
cat ~/.hangar/orq/<date>-<gid>/licoes.md            # every guideline written mid-work
git log --oneline <base>..<tip>                     # commits per Task, correction rounds
git -C <skill-repo> log --oneline --since="<start-date>" -- skills/orquestrar
git -C <skill-repo> diff <commit-before-the-work>..HEAD -- skills/orquestrar
cat ~/.hangar/orq/<date>-<gid>/eventos.jsonl        # rounds, verdicts, times per Task
cat ~/.hangar/orq/<date>-<gid>/medicao/relatorio.json   # consumption per role (consumo.md)
```

- Take rounds, verdicts and time per Task from `eventos.jsonl`; check the prose against it. An
  old run without the file: say so.
- Check the consumption sources and intervals before comparing; a missing or incomplete
  measurement is declared, never reconstructed or treated as zero.
- Every guideline in `licoes.md` and every line the skill gained mid-work is a candidate patch.

## The report: five sections, in this order

No time-analysis section. Any time number comes from `date -Iseconds` or git's stamp, never
from memory. Snapshot duration only delimits the measurement; no productivity-per-hour ranking.

### 1. Waste, grouped

Gather the WASTE lines of all review reports and group repetitions. Three occurrences of one
cause is a hole in the skill; the third's text is the candidate guideline.

### 2. Guidelines born midway

From `licoes.md` and the skill's diff:

| The guideline | What made it be born | Already in the skill? |
|---|---|---|

### 3. What the PLAN got wrong

From the executors' "premise wrong" reports in the journal and the WASTE lines, classify:

| Plan error type | How to detect |
|---|---|
| code nobody ran | executor reports `TypeError`, a missing attribute, a failed import |
| a command that does not do what it says | "selected nothing" / "nothing to run" exit |
| an invented count | "expected N PASS", N+2 came |
| a wave declared disjoint that was not | a merge conflict; a shared file without named regions |
| a defect the plan carried forward | a finding in a late Task originating in an early one |
| a bar demanding what the reused code does not do | mock × existing-component divergence |
| a decision the plan left open | the `Decided alone:` lines of the reports; three on one subject is a plan template hole |

The first six share one cause (code the plan's author never executed): then the patch targets
`planejamento.md`, not an execution page.

### 4. The model cards

`~/.hangar/orq/modelos/<provider>-<id>.md` (provider as `--engine`/`--provider` names it, id
as `--model` receives it), one per team model, outside this skill. Per model that worked: new
numbers (context per Task type, time, cost); consumption per role from the report (uncached
input, cache read/created, output, sources and period; fewer tokens with more rejections is not
improvement; the model given at collection does not attribute each token when the session
switched models); how it failed, as a pattern (one run enters marked `(seen once, on <date>)`;
two agreeing runs make an assertion); what the kick-off had to say because of it. A model
without a card gains its first.

Card rules: measured facts only, dated; every line answers "what do I write differently in the
plan?"; about 40 lines per role section, aged lines leave; no price, quota or permission (those
live in the account policy); what was read elsewhere goes in a `## What they say` section
marked hypothesis; when they diverge the measured value wins and the divergence stays written.
Order: window and practical ceiling · sees images · how it fails · what the kick-off must say ·
where it is good.

### 5. The proposed skill patch

Every proposal carries four fields, in this order: file and section · the text ready to paste ·
the evidence (`measured on <date>: <number>`) · what LEAVES the skill because of it (a guideline
that stopped holding, became code or was absorbed), or one line saying why nothing leaves. No
number, no entry. Say at the end where the proposals concentrate; a section receiving three
or more becomes a short checklist or its own file, never fatter.

**Writing rule for every proposed text, without exception:**

- Imperative sentence saying what to do. No reason, case, incident, example, date or
  measurement in the skill text; those go in the evidence field.
- Positive form: name the action to take ("stage by explicit path"), and keep a prohibition
  only as a hard guardrail beside its positive action.
- A role page is a `## Process`: numbered steps in the order the role acts, each ending in
  `Done when <checkable criterion>`. Reference (templates, tables, checklists, rules for one
  branch) lives in a sibling file named by the step that uses it, read only when that branch
  fires. A proposal adds a rule to the step it belongs to, or to that step's sibling.
- Written as a PRINCIPLE: where the CONDITION should be, no name of a skill, tool, file or date.
  Test: "and when it is not that case?" Two incidents under one condition are ONE entry with
  two measurements. Group by condition, never by the name of what broke.
- Belonging test, all three: it is about coordinating (role, gate, handoff, proof, rotation,
  journal), not the work (tool, stack, file, build command, environment); the orchestration
  fails without it; it holds in another repository and language. Failed one: it changes
  address (the plan, the project's `CLAUDE.md`, a domain skill, the work's lessons). The test
  applies to text already in the skill too: a rule that fails it leaves.
- Size ceiling per file: `SKILL.md` 8,000 characters; every page in `references/` 13,000.
  A proposal that adds text to a file at its ceiling names the text it shortens or merges in
  the same file. Repetition between two pages: keep one, point from the other.
- Each role page stays self-sufficient: executor, reviewer, branch review and retrospective
  read only their page and its named siblings, never `SKILL.md`.
- `scripts/checar-orquestrar.sh` (hangar repo) must still pass after the patch; a proposal that
  would fail it says which check and why.

| Written as a case (does not enter) | Written as a principle (enters) |
|---|---|
| "the `<name>` skill runs crippled when invoked inside a Task" | "a skill invoked inside a Task runs whole; a skipped step is a block" |
| "the `<name>` per-language reviewer doesn't read `.svelte`" | "a tool with an extension filter reports nothing about code it never read; check it serves this Task's files" |

## What does not enter

- Praise or a summary of what went well.
- A guideline for a one-time case with an external cause (blown quota, a full machine): a
  report note.
- A guideline written as a case: rewrite it.
- Rewriting acceptance criteria.

## Where to save, and who applies

```
~/.hangar/orq/<date>-<gid>.md
```

The patch is a proposal; the user applies it. Deliver to the arbiter the file's path and the
three most important lines, not the whole report.
