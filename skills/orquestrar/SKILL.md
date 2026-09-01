---
name: orquestrar
description: |
  Use when the user asks to run a large piece of work with independent review and little interaction from them after planning - "executa esse plano sem eu ficar em cima", "monta o time e toca", "quero revisao independente por commit", "portao entre as Tasks", "uma sessao pra planejar e outra pra executar", "abre uma sessao pra revisar" - or when a large/risky plan is about to become an MR or a push. ALSO use when a kick-off message tells you to invoke this skill and states your role, and when such a work is already in progress and you need to know what to do now. One repo or several - what defines it is the Tasks, the gate between them and the independent reviewer; for multiple repos it agrees the interfaces first and opens one session per repo. Works with a plan in any format, or none (it writes a short orchestration plan pointing at the user's material). Do NOT use for - a small task one session can solve, or a one-off review of a diff (dispatch a review subagent directly).
allowed-tools: Bash(hangar-send:*), Bash(git status:*), Bash(git log:*), Bash(git diff:*), Bash(git show:*), Bash(git branch:*), Bash(git grep:*), Bash(tmux display:*), Bash(date:*)
---

# Pipeline: research → plan → autonomous execution with a gate

A large piece of work crosses five phases, each in a session with the right context. The user
decides everything in phase 1; after that the pipeline runs **on its own** and only wakes them for
what the plan left open.

| Phase | Who | Writes code? | Done when |
|---|---|---|---|
| 0. Research | read-only session/subagent | no | findings in a file the plan cites |
| 1. Spec + plan | **with the user** | no | plan approved, decisions and team settled |
| 2. Launch | the phase-1 session → becomes the **arbiter** | **no, never again** | team created, contract written, one "go ahead" |
| 3. Execution | executor + reviewer, **separate sessions** | only the executor | every Task with `APROVA` |
| 4. Branch review | fresh session that took no part | no | the whole set approved |
| 5. Retrospective | fresh session that took no part | no | proposed patch for **this skill**, in the user's hands |

**The work does not end at phase 4.** An approved branch is finished code; phase 5 is what makes
the next run better than this one. It is short (one session, three input files) and it is the only
phase whose product is not code — it is `references/retrospectiva.md`.

Push and MR always belong to the user.

**In phase 3, the commit comes AFTER the review, and the arbiter leaves the transport.** The
executor stops with a dirty tree, freezes the round and calls **the reviewer directly**; the
executor↔reviewer loop runs without the arbiter, and every round leaves one line in
`eventos.jsonl`, which nobody has to read in the moment. On APROVA, the reviewer authorizes the
executor to commit and notifies the arbiter — only then is the commit born, already reviewed.
**One Task = one commit on the normal path**, even after four rounds: a rejected round leaves no
trace on the branch.

Leaving the transport **is not leaving the authority**. What stops passing through the arbiter: the
hash on its way to the reviewer, the recipe on its way to the executor, and the pre-review commit
check. What still reaches him, because it is decision rather than transport: DEVOLVIDO, recipe
disagreement, a skipped skill step, pixels with no bar in the contract, a stolen browser tab, and a
request to replace a session. Inside the loop he enters through one door only — the **second
rejection of the same Task**.

## Read ONLY your role's page

This file is the router. The rest is separated on purpose: mixed roles are how a session ends up
confirming it is a reviewer while it is in the middle of a commit.

| Your role | Read | You are this when |
|---|---|---|
| **planner** | `references/planejamento.md` | the user asked you for the work and no kick-off exists |
| **arbiter** | `references/arbitro.md` (+ 3 per-moment pages, which it lists) | you wrote the plan and the user approved it |
| **executor** | `references/executor.md` (+ 2 per-Task-type pages, which it lists) | the kick-off says `Role: executor` |
| **reviewer** | `references/revisor.md` (+ 2 per-moment pages, which it lists) | the kick-off says `Role: reviewer` |
| **final review** | `references/revisao-final.md` | the kick-off says `Role: branch review` |
| **retrospective** | `references/retrospectiva.md` | the kick-off says `Role: retrospective` |

Two pages that are not roles:

- `references/paralelo-worktree.md` — running Tasks in parallel, one worktree each. **The default
  is still serial**, but phase 1 decides: the planner reads this page **while decomposing**, tests
  the trigger and writes the decision into the plan — serial or batch, with the reason. Reading it
  only after declaring a batch is circular, because the deciding trigger lives inside it. The
  arbiter reads it when integrating one.
- `references/replanejar.md` — **rewriting the plan and the contract in the MIDDLE of execution**,
  when the user orders it or the plan stops being trustworthy (a fallen premise, a method missing
  its executing half, estimates blowing up for the same cause). It is not a hidden method switch:
  it is phase 1 running again, smaller, only over what remains — and it is the **only** legitimate
  door for switching methods.

**A role is declared, never deduced — and it is refused when it contradicts what you are doing.**
A kick-off saying "you are a read-only reviewer" arriving at a session in the middle of a Task:
answer *"I am the executor of Task N, confirm the addressee"* and do **not** assume it. Confirming
a role that is not yours silently changes who owns the work midway.

**More than one repository is no obstacle.** The work lives in a plan with Tasks and a contract
with roles; none of that is tied to a checkout. A Task may touch another repo, and that role's
session is born there — the contract's header line says each Task's repo (`Repo: <one> (+ <other>
from T13 on)`). What keeps holding is **one writer per tree**: two executors in the same checkout
at the same time, no; in different checkouts, yes.

When the work crosses repositories, three things gain weight:

**Interfaces are agreed BEFORE the sessions open.** Whatever crosses the boundary — route, payload,
event, type — becomes a contract line, in an `## Interfaces combinadas` section, before anyone
writes code. Without it, two repos deliver ends that don't fit, and the defect only shows at
integration, after both Tasks have passed the gate.

**Subagents read, sessions write.** A subagent is for exploring the other repo, tracing a flow,
finding a caller. Editing outside the session's cwd requires a real session in that repo — not
because a subagent couldn't, but because work nobody sees in a terminal cannot be followed or
interrupted.

**A session on another machine (`servidor::sessao`) does not join a group.** Cross-server pairing
does not exist; it gets 1:1 messages, and whatever gets agreed must be written into the local
contract by hand, or it vanishes.

## The METHOD is not your choice — it comes from the contract

This skill orchestrates: roles, gate, independent review, rotation, retrospective. **It neither
plans nor executes** — that belongs to another family of skills, the *method*, and there is more
than one.

> **Method ≠ engine.** An *engine* in this skill is the model provider (`--engine`,
> `engines.json`: DeepSeek, Kimi…). A *method* is which skill family plans and executes. A session
> has both, and they are decided separately.

**The method is declared in the group contract** (`regras-<gid>.md`), in one line, and holds from
research to the last commit:

```markdown
Method: superpowers    # planner: brainstorming → writing-plans · executor: executing-plans
Method: mattpocock     # planner: /grill-me → /to-spec → /to-tickets · executor: /implement
```

**`superpowers` is the default — the user's decision.** Another method enters only on
their explicit request, and the **single** check before accepting is: **does the executing half
exist in the ACCOUNT that will execute?** Skills and plugins are per config directory — check the
path, not your memory. An artifact the method doesn't produce does **not** disqualify it: the
phase 1 exit gate is method-agnostic, and the planner produces by hand whatever is missing. Each
method's operational detail (commands, `disable-model-invocation`, known gaps) lives in
`references/planejamento.md`, "Before phase 0".

No role picks a method, and **none switches methods midway**. A plan born in one method and
executed in another is the defect this section exists to prevent: the two write the work in
different formats (Task with Steps on one side, ticket with criteria on the other), and whoever
reads later — the executor, the arbiter excerpting the Task, the app's progress bar — starts
reading something that doesn't exist.

Three rules, all three the arbiter's:

1. **The `Method:` line is mandatory** in the contract, written at launch, before the first session.
2. **Every kick-off repeats the method**, because the contract is read once and a kick-off arrives fresh.
3. **Contract without the line** → the method is `superpowers`, this skill's historical default —
   and the arbiter **writes the line** before proceeding, instead of leaving it implicit.

A method you don't know, or a request to switch midway: **stop and ask the user**. It is their
decision, like model and account. A switch they approve is not done by patching: run
`references/replanejar.md`, and the plan for the remaining work is born **whole** in the new
method — never half in each.

## Two words: Task and step

The work has two layers, and this skill talks about both all the time. They belong to no method:

- **Task** — the unit of work: it has a name, a set of files, a verification and whatever blocks
  it. It is what the gate opens and closes, and what becomes **one** commit. In `superpowers` it's
  a Task; in `mattpocock` it's a ticket; in a hand-written plan it's an item.
- **step** — the smallest **checkable** thing inside a Task. In `superpowers` it's a Step; in
  `mattpocock` it's an acceptance criterion (its template already uses `- [ ]`); in a hand-written
  plan it's whatever the planner writes in the orchestration plan.

Everything this skill hangs on the step — tracking progress, predicting the context-rotation
point, requiring a smoke test, declaring owned preconditions, splitting parallel arms, triggering
the reviewer's mutation test — works the same in all three forms. **The single exception is
literal and isolated:** the phone's progress bar matches the word `Step` by regex
(`planejamento.md`, "The app's progress bar"), and the bar is optional.

A method that brings no bottom layer disqualifies nothing: the planner writes the steps in the
orchestration plan, which is theirs — the same procedure as any item the method doesn't produce.

## The DOMAIN SKILL, when one exists, is what commands the work

Don't confuse it with the method, which is the section above. The **method** is who plans and who
executes (`superpowers`, `mattpocock`). A **domain skill** is one that describes *the work
itself*, step by step, because someone has done it dozens of times: porting a screen, creating a
module, building a migration. It neither plans nor executes — it says what has to happen, and in
what order.

**When one exists, the plan does not repeat it: it instantiates it.** Each Task cites the skill
step it executes, in the skill's own order, and the executor re-reads the skill before starting.
This is not our invention: a real domain skill in this repository carries, inside it, the
description of how a plan should instantiate it — it was written expecting this.

The contract gains one line, written at launch, and it is mandatory even when the answer is
"none":

```markdown
Domain skill: portar-tela    # steps 1–9; on conflict with the plan, the skill wins
Domain skill: none
```

**Two checks, before Task 1** (the phase 1 exit gate enforces them, in
`references/planejamento.md`):

1. **Does any Task do what the skill already does internally?** If so, that Task **does not
   exist** — what exists is the demand for that step's evidence inside the Task that contains it.
   A skill step promoted to its own Task at the end of the plan makes the gate stop enforcing it
   ("there is already a Task for that"): everything before it passes unchecked, whatever depended
   on the step cannot start, and the one who notices is the user, many Tasks later.
2. **Is any skill step left without an owner?** A step no Task cites is a step nobody will run.
   Also look at what the skill does **not** have: a screen-porting skill may end at its last step
   without ever asking for the whole screen to be tested — work assembled only from its steps is
   born without set-level verification, and that is a hole in the plan, not in the skill.

**The domain skill is not altered to fit this work.** Other people use it. What gets adjusted is
the plan and the contract; the skill is read as it is. A step that doesn't apply is the user's
decision, never the arbiter's — the same case as "an invoked skill runs whole", in the locks
below.

## Kick-off — the message points, it doesn't copy

A new session is born with zero context but with the **same `~/.claude`**: this skill is already
there, by name. The kick-off is an address, not a manual.

```
Invoke the orquestrar skill and read your role's page.
Role: <executor | reviewer | branch review>.
Method: <superpowers | mattpocock | none — the plan is the user's>.
Domain skill: <name | none>.
Repo/branch: <path> / <branch>.   Expected HEAD: <hash>.
Group rules: <path to regras-<gid>.md>.
The current Task: <path to its file>.
Untouchables: <paths, one by one — not "the ones in the contract">.
Lessons that apply to this Task: <pasted here, 3 or 4, not the file path>.
Reviewer for this Task: <session>.        ← executor kick-offs only
Frozen round: <hash> · the dirty tree is YOURS.   ← only when you replace an executor midway
Your turn now: <Task N | wait for the first round>.
When done, send the round to <reviewer-session> and STOP.

Read ONLY these two files besides the skill. The whole plan, the journal and the lessons file
are NOT yours.
```

**Lessons go pasted, not as a path** — the only thing in a kick-off that is copied instead of
pointed at, for a reason: the whole file does not serve this Task, and the arbiter is the one who
knows which lessons do. Sending the path would make the session read everything, which is exactly
the cost the three-file separation exists to avoid.

The last line is an **instruction**, not a comment: without it the session goes after the full
plan and the journal on its own — and pays tens of thousands of tokens of closed history before
its first commit.

`Expected HEAD` and the literal untouchables list exist because a new session, without them,
derives both from `git status`/`git log` and may find a HEAD nobody explained.

The same text, re-sent, puts a `/clear`-ed session back on its feet: it carries no state, it
carries paths. No line of it says "Task 2 already passed" — that belongs to the contract, where it
stays true tomorrow.

## Three files, each with one reader: the journal, the rules and the lessons

**Only the arbiter writes to all three.** A session that logs its own authorization legitimizes
its own deviation, and the arbiter only finds out by re-reading the file.

| File | Contains | Who reads |
|---|---|---|
| `~/.hangar/orq/<date>-<gid>/registro.md` — **the journal** | the execution diary: Task→hash→verdict progress, what each round broke, burned sessions, dated decisions | **the arbiter only** |
| `regras-<gid>.md` — **the rules** | the work's agreement, which barely changes: who is who, untouchables, gates, method, branch, bars, what the review covers, accounts | executor and reviewer, **whole** |
| `~/.hangar/orq/<date>-<gid>/licoes.md` — **the lessons** | the guidelines the execution keeps fixing, one per block, with date and proof | **nobody reads it whole** — the arbiter pastes into each kick-off only the ones that serve that Task |

There is a fourth, which no session reads: `eventos.jsonl`, one line per event, feeding the app's
screens and the retrospective. It belongs to the arbiter and is described in
`references/arbitro.md` — which is why that page speaks of **four** files and this one of three.

> **The journal and the lessons live in the work's durable directory, which nothing manages.**
> `<config>/.hangar-pair/` belongs to the backend: it deletes `grupo-<gid>.md` together with the
> group. The **rules** stay there — it is the path the app shows the team.

The boundary between the three is the content's **type**, not its subject:

- **it already happened → journal** (Task 4 was rejected four times);
- **it is this work's agreement → rules** (the executor is session X, that file is untouchable);
- **it is a guideline born midway that holds from now on → lessons** (that log command hangs
  without the flag that makes it exit).

**The rules barely change after launch; the lessons grow for the whole work.** That separation is
what solves the real problem: a new guideline is the normal product of a run — every rejected
round produces one — and stuffing them all into the file every session reads whole made that file
double in size until someone had to throw things away.

**Lessons are never thrown away, and have no cap.** What has a cap is **how much of them goes into
a kick-off**: the arbiter picks the ones that serve that Task and pastes their text. Managing the
two growing files — the lessons, and the journal's cap/archiving — is the arbiter's job and lives
in `references/arbitro.md`, "You maintain FOUR files".

First line of the rules file, so an amnesiac session can re-anchor itself:

```markdown
> Sessions of this group: invoke the `orquestrar` skill and read your role's page.
> Branch: <branch> · Repo: <path>
> Method: <superpowers | mattpocock | none> · Domain skill: <name | none>
```

The `Method:` line is mandatory (see "The METHOD is not your choice", above) and never changes
midway.

**What changes per Task goes in no file at all**: which Task is released, what the hash is, who
your counterpart is. That goes in the kick-off, which is fresh by definition. A file holding
current-turn state is a file that goes stale between writing and reading.

**The executor receives ONE Task, never the whole plan.** They implement one and the reviewer
reviews one. How that is done depends on the material's format:

- **Monolithic plan** (one file with all the Tasks) → **excerpt**: that Task's section plus the
  short header (goal/architecture) into `~/.hangar/orq/<date>-<gid>/tasks/task-<N>.md` — a durable
  path, not `/tmp`, which vanishes on reboot — and send that path. In one run: whole plan
  ~30k tokens, excerpted Task ~2.9k.
- **One file per unit** (tickets) → **point at the user's file**, without copying. Copies go
  stale: the executor checks the criteria off in the original and the copy starts lying about
  what's done. The work context the ticket doesn't carry — because `to-tickets` says to write only
  the slice — goes **pasted** into the kick-off, three or four lines, as is already done with the
  lessons.

**Who belongs to the group comes from the contract, never from `hangar-send --list`.** A live
session in the same directory is just a live session in the same directory — the user opens
sessions for whatever they want, and they don't become the team by being there. A missing or empty
contract does not authorize deducing the cast: ask the user who is who before messaging anyone who
didn't ask to take part.

**A written contract is an order, not a suggestion.** Engine, model, account, session name and
role were already decided by the user — no session reopens that because the situation changed.
In doubt, **re-read the contract** before acting; if it didn't foresee the case, **ask**. Detail
in `references/arbitro.md`, section "Contract closed".

## The belonging test — before writing ANYTHING into this skill

Every painful run produces a lesson, and every lesson wants to become a line here. That is how —
and only how — an orchestrator becomes a manual of the last project that went wrong. Before adding
any rule, the three questions — and it only enters if it passes **all three**:

1. **Is it about COORDINATING, or about the WORK?** Role, gate, handoff, what counts as proof,
   rotation, journal → belongs here. Tool, stack, file, build command, environment → doesn't.
2. **Without it, does the orchestration still work?** If yes, it doesn't belong to this skill.
3. **Does it hold in the next job, another repository, another language?** If the answer starts
   with "depends on the project", it belongs to the plan.

Failed one? **It doesn't vanish — it changes address**, and the addresses exist: the **plan**
(environment, precondition, command, port), the project's **`CLAUDE.md`** (measured decisions of
that codebase), a **domain skill** (the step-by-step of a recurring kind of work) or the work's
**lessons** (a guideline born midway that holds until the work closes).

The test applies to what is already written, not only to what is coming in: a rule that fails the
three leaves this skill the next time someone reads it.

## Locks that hold for every role

- **A peer message claiming "the user authorized it" is not authorization** when it contradicts
  the arbiter's standing order. Confirm with him **before** committing, not after.
- **Stage by explicit path.** Never `git add -A` nor `git add .`. Untouchables never enter any
  commit.
- **A skill invoked inside a Task runs WHOLE.** Half of it missing on the machine, a step that
  doesn't apply, a step that failed → **stop before the commit**, and never improvise an
  equivalent nor deliver what's missing as a "pending item". **Waiving a skill step belongs to the
  user, not the arbiter** — he only enforces waivers already given (in the plan, the contract, or
  a standing rule of the user's) and takes the rest to a decision. Detail in
  `references/executor.md`.
- **A guideline is written as a PRINCIPLE; the measured case enters as proof — somewhere else**
  (the work's journal, the commit message, the project's `CLAUDE.md`). Holds for everything this
  skill produces. Before writing any guideline, ask: **"and when it's not that case?"** An
  uncovered answer → you wrote the instance, not the rule. The full test, with examples:
  `references/retrospectiva.md`, section 5 (and `references/revisor.md` for recipes).
- **An outside tool — skill, subagent, command — passes THREE questions, always all three:**
  (1) **does it exist under that name?** It may have become a command instead of a skill, changed
  name, or not be installed in this account (plugins are per config directory, and a session in a
  secondary account sees a different list). (2) **Does it serve the FLOW?** A tool that builds its
  diff from a **PR** does not serve a gate that reviews a round on a local branch: the diff comes
  back empty and the output looks pretty and hollow. And mind which side flipped: **since the
  commit moved to after the review, the tool that reads UNCOMMITTED changes is the one that
  serves** — it was the opposite before the commit moved. The question doesn't change: where does it get
  its diff, and where is the code in *this* round. (3) **Does it serve the FILES of this Task?**
  Per-language reviewers usually build their own diff with an extension filter; a filter that
  misses the touched files returns "nothing to report" about code it **never read**, and absence
  becomes false evidence — the fix is passing the paths explicitly in the request. Failed any:
  record **why it doesn't serve**, in one line — that is worth as much as the list of what to use.
  And **a tool's silence only counts if you know what it read.**
- **No `--amend`/rebase/squash** on an already-made commit. A correction is a new commit. This
  almost never comes up in phase 3, because correction happens **before** the commit; when it does
  — a commit that diverged from the approved round, a post-merge batch fix — it is a new commit,
  with a trail.
- **Write first, notify after — always in that order.** Review reports, reports and recipes are
  born as a **file** in the work's durable directory **before** any sending, and the message
  carries the **path**, never the content. It is not formatting: it is what makes the work survive
  the channel — a channel fails in several different ways, and with the file already on disk none
  of them loses work. It is also what makes a mutilated message impossible: text that travels as a
  path has no backticks for the shell to eat.
- **The transport ladder, in order, and the next rung only after the previous one failed:**
  **look at the recipient's pane** (an open overlay/menu refuses typing, and is what the backend
  reports as "session unavailable") → `SendMessage` → `hangar-send --tmux <session>` →
  `tmux send-keys` into the pane. `hangar-send <session>` **refuses** to talk to a Claude session
  on this machine (rc=3, "the native path reaches both ends") and says to use `SendMessage`; with
  `ListAgents` **empty** — it happens — the native path has no address, and `--tmux` is what's
  left. Pi and Codex sessions don't suffer from this: only the Claude→Claude pair. A refusal **by
  the recipient** is not to be bypassed through another transport; a refusal **by the tool** is —
  and the rung used goes in the report, because a broken channel nobody records is the same scare
  twice.
- **`hangar-send` takes the message as an argument, not on stdin.** Long text goes via a
  single-quoted heredoc **inside** a substitution:

  ```bash
  hangar-send <session> "$(cat <<'EOF'
  ...free text, with backticks and $ intact...
  EOF
  )"
  ```

  Raw double quotes make the shell eat backticks and `$`, and a mutilated recipe is worse than no
  recipe. A bare heredoc (`hangar-send <session> <<'EOF'`) returns a usage error — the message
  doesn't go out.
- **Choosing the team is an OFFER, not an obligation — the question is asked ONCE and any answer
  unblocks the work** (no answer → the default, on the account already in use). The whole recipe,
  with the per-role default, is in `references/planejamento.md`, "The team is an output of planning".
  Leaving the account in use, or entering an account that **charges per token**, still requires
  the user's word.
- **THE MODEL IS THE USER'S DECISION. Nobody picks a model outside the default above.** The
  machine's account policy lives in **`~/.hangar/orquestracao-contas.md`** — which accounts exist,
  which are subscriptions (free model switching inside the account), which are pinned to one
  model, and which are forbidden because they charge per token. The arbiter **reads that file
  before assembling a team** and copies into the contract only what this work will use. File
  missing or stale: **take the inventory and ask the user** (the survey recipe is inside the file
  itself), write the answer there with the date, and proceed. The contract carries the
  account↔model table per role (`## Quem é quem` in `regras-<gid>.md`, fixed format in
  `references/planejamento.md` — machine-read: a cell with prose is not read); it is closed. A
  model outside it is not used **even to test**, not because "it's cheaper", not because it showed
  up in the catalog. Each account has its own quota and price, and the wrong provider **charges
  the user's money** — an `openrouter/*` picked on one's own is an invoice, not an experiment.
  - **A new session is born on the harness default, which is not the table's model.** Whoever
    creates it: switch, **read the model back** and check; only then send work. A session working
    on an unchecked model is spend on the wrong account that only shows up on the invoice.
  - **Subagents are allowed — but ALWAYS on the session's own ACCOUNT, and model freedom is PER
    ACCOUNT.** Leaving the account is never allowed. Switching models **inside** it only where the
    contract explicitly allows: there are accounts where the user accepts two models (a stronger
    one for judgment, a cheaper one for mechanical work) and accounts **pinned to a single model**
    — and there is a forbidden account, because it charges per token on his card. Don't deduce
    from the model list the account offers: what's written in the contract is what holds, and an
    unlisted account is **stop and ask**. And check the frontmatter of whatever you dispatch: a
    `model:` written inside overrides yours (the `ecc:*` agents carry `model: sonnet`, which in a
    Claude session spends the Anthropic account; the Pi bridge strips that field).
  - Need a model that is not in the table? **Stop and ask.** It is not an arbiter's, executor's or
    reviewer's decision.
- **Delivery is not a reply.** `entregue -> <session>` and `SendMessage`'s `success` say the
  message **entered the destination's queue**, not that anyone read it, nor that an answer will
  come. There is no per-message deadline: a whole Task takes what it takes, and nudging a working
  executor is noise. **The signal is elsewhere — see "Idleness" below.**
- **Never `command | tail && echo OK`** — the `&&` reads `tail`'s exit code, and the "OK" prints
  with the command failing. Use `set -o pipefail` or check `${PIPESTATUS[0]}`.
- **Verification runs the command the plan defined for that Task**, in a form that does not depend
  on the cwd (explicit prefix/directory). Never invent the command nor run "what it usually is".
