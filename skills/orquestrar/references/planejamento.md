# Role: planner (phases 0, 1 and 2)

You drive the research, write the spec and the plan **with the user**, and launch the team. When
the plan is approved you become the arbiter — and from then on you write no more code. Read
`arbitro.md` at that moment.

## Before phase 0: which METHOD you are using

This skill orchestrates; the *method* is what plans and executes, and there is more than one. It is
**the user's decision** — ask at the start, don't deduce, and the answer goes into the contract's
`Method:` line, which you write in phase 2 and which **every kick-off repeats**.

- `superpowers` → you use `superpowers:brainstorming` and then `superpowers:writing-plans`; the
  executor uses `superpowers:executing-plans`. **It is the default and the recommendation** (the
  user's decision).
- `mattpocock` → you use `/grill-me` (or `/grill-with-docs`) → `/to-spec` → `/to-tickets`; the
  executor uses `/implement`. **Only on the user's explicit request, and only after checking that
  `/implement` is installed in the account that will execute** — a method run without its
  executing half leaves the arbiter improvising, and an install is a fact of a machine, not of
  the skill: check it in the executing account, every time. The executor's kick-off **starts**
  with the `/implement` line (both skills carry `disable-model-invocation: true`, so the session
  won't auto-invoke them — but the kick-off arrives as typing in the pane). Whatever the method,
  the **phase 1 exit gate** (section at the end of this page) holds the same: an artifact the
  method doesn't produce, you produce by hand — and items 2, 3 and 4 are your audit, with a
  command, not something you wait to arrive written. `to-tickets` delivers blocking edges
  (order), not estimates nor disjointness; that doesn't disqualify it, it only says which part of
  the gate is left to you.

- `none` → **the plan already exists and belongs to no method**: the user wrote it by hand, it came
  from another ticket, or there is no written plan. A legitimate and frequent case — see the next
  section.

The plan and the execution, when they come from a method, come from the **same one**: Task/ticket
formats differ, and whoever reads later — executor, arbiter — reads the wrong format with no error
at all. Switching methods midway is `references/replanejar.md`, never a patch.

## The plan is the user's. You write the ORCHESTRATION PLAN next to it

This skill doesn't need the plan to have a format. What it needs is **thirteen properties of the
work** decided before Task 1 — that is the exit gate at the end of this page, and it declares
itself method-agnostic. Nothing there asks who wrote the plan; everything asks whether the Tasks
collide, whether each has proof, who owns each wait, what is untouchable.

So the rule is this, and it holds for all four cases (`superpowers` plan, plan from another method,
hand-written plan by the user, **no plan**):

**You do NOT rewrite, convert or copy the user's plan.** It stays theirs, in their format, in their
file — and it stays the source. A copy diverges from the original on the first tweak, and then two
people read different things believing they read the same one.

**You write a second, short file that POINTS at their plan** and adds only what is missing for the
gate to work:

```markdown
# Orchestration plan — <work>
User's plan: <absolute path>   (it is in charge; this file only orchestrates)

## Tasks
| # | What it is | Where in their plan | Files | Verification | Proof |
|---|---|---|---|---|---|
| 1 | create the schema | section "Database", 2nd paragraph | `<migration paths>` | `<repo test command>` | green suite + the table exists |
| 2 | listing screen | step 3 of the porting skill | `<screen paths>` | `<repo typecheck command>` | screenshot + comparison against the bar |

## What their plan does NOT decide, and I decided here
- Order: 1 before 2 (the screen reads the table).
- Untouchables: <paths>.
- Bar for Task 2: <screen, width>.
```

The **"where in their plan"** column is the heart of this: it may point at a section, a paragraph,
a line number, a domain-skill step — whatever exists. When their plan says nothing about something,
the cell stays empty and the item shows up in the bottom list, which is what you had to decide.
That list is what you show them before launching.

**With no plan at all, the orchestration plan is the only document** — and then it is the plan,
written in whichever method the user picks, or by hand with you. The exit gate holds the same.

**The app's progress bar is the only thing that depends on format.** It reads `### Task N:` and
`- [ ] **Step N: …**`, and nothing else — the word `Step` there is literal, matched by regex by the
app's counter, and it is the only place in this skill where it still holds. A user plan without
that format shows no bar on the phone — a **limitation, not a defect**: the work runs the same.
Whoever wants the bar writes the steps in that format **in the orchestration plan**, which is
yours; the user's stays untouched.

## Phase 0 — Research (only if the plan can't be written without it)

A **read-only** session or subagent, with a closed question ("how does flow X work today", "what
breaks if Y changes"). The output is a file on disk that the plan references — research that only
exists in a session's context dies on `/clear`. Can the plan be written without it? Skip it.

**"It doesn't exist" is the answer to ONE question — write down which question the search asked.**
Absence is not a fact of the repository; it is the result of one query, and two queries over the
same base return opposite answers. Before recording "the field doesn't exist" / "there's nothing
about this", write the phrase you searched for, and **redo the search by a second path** whenever
the absence supports a decision. A conclusion of absence can stand for days and fall at the first
search made with another word — searching for a *date* when the product uses a *status* is the
typical shape. And something else hides in the same place: **the missing rule is often already
written in the user's own material** — before declaring that something depends on their decision,
re-read what they already wrote.

The same holds for database and service queries, where missing permission and missing object
return exactly the same output — zero rows.

## Phase 1 — Spec and plan

### The TEAM is decided BEFORE writing the plan, not at its end

An ordering that looks like detail and isn't. Who will execute changes **what the plan needs to
say** — and the plan is the only document the executor reads whole.

Ask about the team right after the spec closes, and **before** the first Task. Then read each
model's card in `~/.hangar/orq/modelos/` (one per model, measured facts only; format rules in
`references/modelos/README.md`) and write the plan with that in mind. No card for the chosen
model? Write the plan conservatively and **create the card at the end**, in the retrospective —
that is how cards are born.

What the card changes: a **MEASURED** capability becomes protocol in the Tasks (vision, visual
criteria as numbers, a short window planning rotation, how much detail each step needs); a
capability in **HYPOTHESIS** state becomes a **debut test** in the first kick-off (one turn), never
a mandatory protocol — and the card is corrected with the result. Without a card, the plan is
written for a generic executor that doesn't exist, and every real trait of the model becomes a
correction round.

**A step or recipe that creates screen state fed by a request declares the THREE outcomes —
success, failure, pending — and the WHEN of each call (mount × interaction).** This omission is
made independently by planners, arbiters and reviewers, and each instance costs a round: failure
declared and pending silent (fail-open holding for an in-flight request); "fetch the list live"
without a when (a probe that types running on every mount); success and "loading" closed and
failure silent (a failed request becoming an assertion on screen). With a literal-recipe executor,
the undeclared outcome is the unimplemented outcome. And if the plan declares that a Task's recipe
closes AFTER another one (planned replanning), it names WHO closes it — and that who is a planner,
never the arbiter by gravity (`replanejar.md`, "the miniature").

**A team model with no card yet:** the sweep (vendor + community, via `last30days`) and the initial
card format are in `references/modelos/README.md`, "A new model on the team". What the sweep does
**not** do is become a kick-off rule: nothing untested becomes a rule until a run confirms it.

Beyond what `writing-plans` already asks, the plan carries:

- **Task order** and which don't parallelize, with the reason. The default is **serial**: one Task
  at a time, the gate closing each one. Large work with genuinely independent Tasks becomes a
  parallel batch with one worktree each — the trigger and the cost are in `paralelo-worktree.md`,
  and the decision is **here**, with the user, never the arbiter's later. **You audit, you don't
  wait for the plan to declare**: the trigger's four conditions are answered by exit-gate items 3
  and 4, which are your work whatever the method.
- **A-priori estimate, written BEFORE the kick-off**: one line per Task — expected **clock and
  rounds**. It is not guessing: it is the yardstick that lets the arbiter see "blown" while it
  happens. With the line written, a Task blowing its estimate is caught and documented on the
  spot; without it, a Task can run for hours with zero commits and no number screaming. A plan
  without this fails the exit gate.

  **It lives in the plan, and the ACTUALS live only in `eventos.jsonl` — there is no second
  table.** The estimate is written once, here; the actuals the arbiter already records event by
  event in `eventos.jsonl`, and phase 5 crosses the two. A manual "estimated × actual" table that
  duplicates data the machine already has is the twice-written obligation that gets
  half-fulfilled: it stops being filled mid-work with nobody noticing, and the stretch covered by
  neither source is usually the most expensive one.
  **A team with more than one authorized executor: the estimate carries consumption PER MODEL, not
  only per Task** — in **quota and context** (the accounts are subscriptions; there is no
  per-token invoice to analyze): expected context per Task, sessions per Task and which
  account/window each model spends — with the rule of when the heavy model enters declared
  alongside. Two authorized models can burn the same account's window at rates an order of
  magnitude apart, and a single estimate line describes neither. The cards in
  `~/.hangar/orq/modelos/` are the source of the per-model number.
- **External preconditions with an OWNER**: every step whose proof depends on something the
  executor doesn't control within the turn (a server up, a tmux session, a test account, an element
  on screen) declares who creates it — and the owner is **the executor itself**, as an explicit
  prior step ("bring the backend up on port X, confirm with curl, THEN capture"). A wait with no
  declared owner becomes infinite polling — an executor checking, hundreds of times, for a thing
  only it could create.
- **Untouchables**: paths with parallel changes in the tree, listed one by one.
- **Verification per Task**: the exact command and what counts as passing. An **orchestration**
  Task (tmux, CLI, process, account, network) carries a **smoke-test step against the real
  source**, with the literal command — a green suite of fakes proves no flow: a module can pass
  thousands of green tests with the whole flow dead, because the new tests reproduce the code's
  own wrong assumption and no step touches the real thing.
- **Steps written as `- [ ] **Step N: …**` (the word is literal here) — only if you want the
  progress bar on the phone**, and then in the file **you** write (the orchestration plan), never
  by reformatting the user's. It is the format the app's progress counter recognizes (`### Task N:`
  for headers). Numbering any other way (`Passo A`, `Etapa 1`) makes the whole Task count **zero**
  and the bar the user follows on the phone sit still while the work moves. A recipe shared by
  several Tasks: write it as explanatory text and **repeat the steps inside each Task** — the
  executor reads one Task at a time and cannot depend on having read the previous one. Before
  approving, check the bar counts what you expect: open the plan in the app and compare the total
  with the number of steps you wrote.
- **A bar** for Tasks that touch pixels: what the result will be compared against — see below.
- **What the review must cover** — see below. This enters **before Task 1**.
- **Open decisions**: what is not yet decided and who decides. An empty list is the goal.
- **Quota and fallback** — and **not** a money cap; see the rule below. What enters is what **runs
  out**: each team account's remaining quota (pasted, with the reading time) and the **fallback
  authorized in writing** ("X's quota runs out → executors migrate to Y, known effect: ...").
  A provider quota blows overnight with the user asleep, every session on it dies in the same
  minute, and an unwritten fallback becomes middle-of-the-night interventions.
- **The team**, with each role's engine and account.

**There is no money cap in this skill.** Whoever uses this controls spend some other way — through
the subscription they signed, the provider's dashboard, the account they choose to open — and the
machine's account policy already **forbids** accounts that charge per token. A budget in currency
here would be a number nobody can measure from inside a session, and one that would stop good
work.

What exists, and is different, are **walls**:

- **Quota** — it runs out, and when it does the session dies. That is why it is read before
  launching, and why the fallback is written.
- **Context** — when a session passes half of its own window, it rotates ("Autonomy — triggers",
  in `arbitro.md`). That is session rotation, not a budget.
- **Clock and rounds** — a Task past 2× its estimate without closing is a spiral, and the arbiter
  asks.

None of the three is a value the user "agrees to spend": all three are facts the arbiter reads.

### Before closing the decomposition, look for SHARED STATE

Two Tasks that mount hosts of the same store, singleton or registry **are not independent**, no
matter how disjoint the files — and the collision doesn't show at merge time: it shows up as review
rounds, one host writing into state the other clears, reads or deletes, round after round.

Found one? The plan writes the **ownership contract** before the first of the two Tasks: who
writes, who clears, what happens when a host unmounts, and what happens on resize. Two lines in the
plan; without them, rounds.

**An ownership contract ("file X is closed in this batch; each new module creates its own") avoids
merge conflicts and creates duplicates — declare what happens to them.** Written without that
clause, the rule produces near-identical copies of the same client across modules — and when two
Tasks discover the same defect, each fixes its own copy, because **there is no single place to
fix**; the defect outlives the branch.

When writing the ownership rule, write alongside it:
- **how many copies it will create** (it's the number of Tasks touching the pattern), and
- **the unification Task** at the end of the batch, or the explicit sentence "the N copies stay,
  and the set review checks all N" — which is what the final reviewer will enforce.

And the small version of the same thing, which holds inside a single commit: **two computations
that MUST give the same result become one, derived in one place** — not two copies. The same value
computed at several spots of one component, with operators that differ on the edge case (`??` ×
`||` over an empty string), is a regression introduced by the very commit that fixes the screen —
parent green, child red.

**It is the same cause that makes screen-Task estimates miss, which is why a screen Task is
estimated by the STATE it touches, not by the pixel.** A screen that mounts a host of an existing
store costs several rounds; a screen that draws a new component over its own state costs one or
two. Don't estimate by the blind comparison either: mock divergences become notes, while **code
blockers** are what reject rounds. Decomposing by screen is great for splitting work and terrible
for predicting risk.

### The review's rigor enters the contract before Task 1

Write in the plan what the review has to break: the full flow in the UI or the real command,
sibling callers of the changed symbol, concurrency (delayed response, double click, target switch
mid-flight, unmount), final state on disk/storage/URL, and which review skills to use per Task
type.

A visual Task enters with **the list of states** that need screenshots (both widths, overlay,
fullscreen, whatever else the Task affects). That list is what the reviewer enforces later — a
state nobody listed is a state nobody looks at.

#### Capture belongs to the EXECUTOR: how many screenshots, and whether a dedicated session pays off

The list above is about coverage — which states the Task must prove. **How many screenshots to
take, and who takes them, is the executor's decision at execution time** (the user's decision).
The plan imposes no number: it doesn't know how many screens the Task will end up having, and a
cap written here limits legitimate work.

What the plan does is **tell the executor what capture costs**, so they decide with data, not
hunches. It cuts both ways: a large sweep run in its own disposable session closes in hours,
while the same sweep embedded inside the executor ("screenshot of every state × hosts ×
languages") can hold the most expensive Tasks hostage for half a day with no merge. When the
sweep is large, the cheap way out is a disposable capture session, with the state list in its
kick-off — the executor delivers code, verifications and the sanity screenshot, and the capturer
sweeps the rest. **The executor chooses this, and the choice goes in the report.**

And the mother rule, which holds for every gate in this skill: **a demand for new proof (outcome,
more states, more variants) only enters with the OWNER in the same sentence** — who produces that
proof, and where. Two right rules added without owners ("proof ends at the outcome" with no stage
owner, "the cap only counts bar rounds" leaving capture unbounded) are how capture becomes
unbounded work.

### A visual Task also enters with a BAR

One line per pixel-touching Task: **what the result will be compared against**. Not an adjective
("pretty", "polished", "on brand") — a thing that exists and can be opened. Three tests, all
mandatory:

- **Named**: a specific screen, not a category. `EnginesSheet` yes; "the app's other sheets" no.
- **Findable**: whoever judges can **see** it — an absolute path to a screenshot, or a screen of
  the app itself that can be opened and captured. A reference that exists only in someone's head
  doesn't serve.
- **Comparable**: the two images fit side by side, same state, same width.

In practice the bar is almost always **a sibling screen of this app** — the hardest comparison
there is, because it is exactly where misalignment shows. An outside reference (a screenshot of
another product the user saved) counts the same, as long as they hand over the file here in
phase 1.

Without a written bar, the visual gate remains the author asking their own pixels if they look
good — and "does it look good?" returns "it looks good". A Task that draws nothing needs no bar.

#### You PROPOSE the bar; the user chooses

Don't ask *"what's the bar?"*. Whoever uses the app knows what they want to see, not necessarily
which reference works as a bar — and a malformed bar (a category instead of a screen, something the
judge can't open, a state that doesn't match) is worse than no bar, because the gate appears to
exist.

Arrive with **two or three ready candidates**, each already passed through the three tests, and one
sentence saying why that one is hard:

```
Task 3 touches the Settings sheet. Bar — pick one:

a) `EnginesSheet.svelte` on desktop, centered modal, 1440px — the most polished sheet in
   the app and it uses the same `wide`/`centered` pair; if the new one doesn't tie with it, it shows.
b) `Git.svelte`, same width — same glass material, but with tabs; good if you want to
   enforce tab navigation too.
c) A screenshot you save from another product — send me the path and I'll use that.
d) No bar for this Task.
```

**"No bar" is a legitimate option and stays on the list.** Chosen, it enters the plan as
`Bar: none — user's decision, <date>`, and that Task's visual gate goes back to the normal
`executor-visual.md` protocol (open, click, capture, look), without the blind comparison. A
recorded choice is not a hole; the hole is the blank field nobody decided.

If the three candidates look weak to you, say so and propose others — a bar you wouldn't defend
yourself shouldn't go on the list just to make three items.

Without this, the first Tasks pass through a gate that doesn't exist yet, and the price is a
retroactive audit that reopens approved Tasks — more expensive than writing three lines.

### The team is an output of planning — but **you PROPOSE, the user chooses**

Who writes and who reviews is decided **here**, because the research and the brainstorming have
just shown what this work is made of. Deciding at launch is deciding without that data.

**"Decided here" means the QUESTION is asked here — not that you answer it.** Model, engine,
harness and account are the user's, always, and reading the machine's account policy does **not**
authorize you to fill things in: that file says what **may** be used, never what **will** be used
in this work. Filling the table on your own is indistinguishable, in the journal, from a decision
the user made.

**But the question is asked ONCE, and it has a way out.** Someone with a single account has nothing
to choose, and stalling the work on an unanswerable question drives users away. Start with this
one, and proceed on any answer:

> "Do you want to pick the team (account and model per role), or do we go with the default?"

- **Wants to pick** → this section's full recipe: inventory taken, two or three combinations
  proposed, they decide.
- **Doesn't want to, or didn't answer** → **the default, on the account already in use**: executor
  on Opus effort `medium`; reviewer, arbiter, final review and retrospective on Opus effort
  `high`. The table is born filled that way, with the word `default` and the date on the line, and
  the work starts. They can change it whenever, via the modal or by asking.

**This is not a session choosing an account.** The account stays theirs, the one already open —
the default fills model and effort inside it. Switching accounts, or entering an account that
charges per token, still requires their word: it is their invoice, and no automatic default
reaches it.

Same format as the bar (section above): arrive with the work characterized and the combinations
the machine can actually open, and ask **one question**. Don't ask "which model?" — they may not
know what the machine offers; and don't list "the allowed accounts" as if they were options.

Take a real inventory before asking, because **`engines.json` is not the universe**:

```bash
claude-engine                     # engines for a Claude session with --engine
pi --list-models | awk 'NR>1{print $1}' | sort -u   # Pi providers (run from the USER's shell)
ls -d ~/.claude ~/.claude-*       # Claude accounts
```

Two traps, both capable of making you propose something that doesn't exist:

- **Harness ≠ engine.** An account may be unreachable via `--engine` and perfectly reachable via
  `--provider pi` or its own CLI. Listing only the engines hides half the real options.
- **Run the listing from the user's shell.** A provider whose credential is an environment variable
  (fish universal, `set -Ux`) does **not** show up in a non-interactive bash, and you conclude it
  doesn't exist. Use `fish -l -c '...'` (or their shell) before asserting an account is not
  configured.

And check **id collisions** before proposing: two different accounts can offer the same model
names, and only the full `provider/id` distinguishes them — proposing the wrong one is somebody
else's invoice.

There is no default cast. A model named in any example is an example — never a default. Look at
the Tasks and answer:

| Question about the work | What it decides |
|---|---|
| Is each Task mechanical volume, subtle reasoning or visual judgment? | who writes — possibly **one writer per Task** |
| Where does its typical error show: tests, screen, load, state on disk? | what the reviewer must **be able to do** (see screenshots, run a harness, read concurrency) |
| Any visual Task? Does the chosen executor see images? | if it doesn't, the vision protocol of `executor-visual.md` (`see`) is mandatory and enters the contract — not a reason to discard the engine |
| Does every role have its own session, nobody holding two? | **non-negotiable** — see the fixed rules below |
| On which account, and can its quota take it? | the engines, and the fallback |

Fixed rules:

- **One role, one session.** Each row of the roles table is its own session, and no session holds
  two roles at once. It holds between all of them: the arbiter doesn't execute, the executor
  doesn't review, the reviewer doesn't do the final review. The reason is the same in every pair —
  whoever did a thing already defends the choices made doing it, and stacking the next role turns
  judgment into a rubber stamp.
  **It is about sessions, not models.** Two sessions with the same model, account and provider
  satisfy the rule; one session wearing two badges does not. In account rotation this happens all
  the time, and it is fine.
  The only legitimate switch is of **phase**: whoever planned becomes the arbiter when the plan
  closes, and is then read-only in code for the rest of the work. Role succession (passing the
  baton) is also a switch, not stacking — the leaving session stops acting in that role.
- **Final review** in a fresh session that took part in nothing.
- One writer per tree holds even with several writers in the cast: the gate serializes the Tasks,
  so they never write at the same time.

The team goes into `regras-<gid>.md` as a **fixed-header table**, in the `## Quem é quem` section
— six columns, one row per role, **raw value in every cell** (no bold, no parentheses, no prose;
`-` = empty). There is an optional seventh column, `vez`, for when the team rotates between
accounts inside the same role (one row per account, the turn decided by the Task number) — it only
enters the table when some role actually rotates; format and rule in `arbitro-lancamento.md`,
"Opening a session". **Starting point:** if `<pair_dir>/regras-padrao.md` exists (the "default
team", which the user configures in the hangar's Orchestration modal before any group), copy the
table from there and only adjust the session names — it is their choice, not yours:

```markdown
## Quem é quem

| papel | sessão | provider | conta | modelo | esforço |
|---|---|---|---|---|---|
| árbitro | <work>-arbitro | claude | padrao | opus[1m] | high |
| executor | <work>-t* | claude | 200-01 | opus[1m] | medium |
| revisor | <work>-review | pi | clinepass | cline-pass/glm-5.2 | high |
| revisão final | <work>-final | claude | claude-200-3 | opus[1m] | high |
| retrospectiva | <work>-retro | claude | claude-200-3 | opus[1m] | high |
```

**The table is born with ALL the pipeline's roles, including phases 4 and 5.** Branch review and
retrospective arrive days later, when whoever launched is no longer in the session — and without
the row, that moment's arbiter picks account, model and effort alone for a role the user never
saw, and records it as his own decision. The row costs ten seconds here and removes a decision
from his hands there.

- `provider`: `claude` | `codex` | `pi` | `kimi`.
- `conta`: on Claude, the config-dir name (`padrao` for `~/.claude`, `200-01` for
  `~/.claude-200-01`); on Kimi, the provider in `~/.kimi-code/config.toml` (`apikey`); on Pi, the
  provider from its catalog (`clinepass`); on Codex, `openai-codex`.
- `sessão` ending in `*` = a role with one session per Task (`<work>-t*`).
- A role may occupy **more than one row**, rotating between accounts: add the `vez` column
  (`| papel | vez | sessão | …`) and number 1, 2, 3. Task N belongs to row `(N-1) % total`. Full
  rule in `arbitro-lancamento.md`, "A rotating role". Without rotation, the column doesn't
  exist.

**Work in more than one repository**: add to the contract, BEFORE opening the sessions, a section
with what crosses the boundary. It is what keeps two repos from delivering ends that don't fit,
with the defect only showing at integration, after both Tasks have passed the gate.

```markdown
## Interfaces combinadas
- <route, payload, event or type agreed between the repos>
```

The contract's header already names the repos (`Repo: <one> (+ <other> from T13 on)`), and each
row of the roles table is born in its Task's repo.

**The header is exact and the table is machine-read**: the hangar's Orchestration modal shows this
table to the user, with what each live session measures next to it, and writes back here whatever
they change. A cell with prose ("Opus 5, effort `medium`, accounts X and Y (decision of <date>)")
is not read — the role vanishes from the screen. Everything that is explanation — why that
account, what to do when quota runs out, the final review's trigger — goes as prose **outside**
the table.

**How to open goes as prose, right below the table**, one literal command per role. It exists
because *"the final review runs in an X-agent session"* is a sentence that ages badly: months
later, at opening time, it becomes an improvised decision between default account, engine, gateway
and subagent — and the four give different results. Write the command the day the user defines the
role: `hangar-send --new <work>-final <cwd> --conta claude-200-3 --model 'opus[1m]' --effort high`.

**The final review enters the table as its own item, with its trigger alongside:** *"fires when
every code Task is approved"*. Never "after Task N" — a manual Task (uploading an asset,
registering a domain, touching a third-party account) is not a code Task, and tying the final gate
to one makes the trigger never fire. The opening recipe is in `arbitro.md`.

### Before approving

Run the plan through an adversarial pass (architecture subagent + explorer): does every cited
file/symbol exist? Does the order hold? What breaks? A plan citing a nonexistent symbol becomes a
lost round in execution.

**This subagent runs with the model in its own definition — force nothing.** The machine's account
policy governs the **team's sessions**, which haven't even been decided at this point; it does not
govern a subagent you dispatch during planning. Passing `model:` here "to respect the table" is
applying a rule outside its scope, and it also overrides the model the agent's author chose — the
table cannot be respected in a phase where the user has defined no team at all.

Offer the pass — don't run it on your own, and don't skip it because you think you've checked
everything. It catches what you cannot see: cited files that don't exist, an order that doesn't
hold, and even the plan's own guard regex that would start matching the wrong thing after the Task
that fixes it — the class of problem that makes Tasks close red.

## Code that enters the plan is code YOU ran

Even a well-written, audited plan ships defects that all share the **same** cause: the plan
described code its author never executed — an attribute that doesn't exist, a `TypeError`, a test
filter that selects nothing, wrong counts, a "disjoint" batch that collides, a bar demanding what
the reused component doesn't do. None is a reasoning error: they are things **a command would
have answered in seconds**.

Before closing the plan:

- **Run whatever can be run.** Every verification command you wrote runs **now**, in the repo, and
  you paste the real output — including a "0 selected", which is the answer that exposes a test
  filter written in the dark.
- **Every function, attribute and fixture the plan cites, check that it exists** — `grep` the
  repo. The same class of hole passes at file level and at attribute level alike.
- **Test counts: count, don't estimate.** A wrong "Expected: N PASS" makes the executor think they
  broke something and hunt for a defect that isn't there.
- **Batch disjointness is checked in the STEPS' text, not in the "Files" block.** That is exactly
  where collisions hide: a Task's header doesn't cite the file; one of its steps orders editing
  it.
- **The bar must be possible with the code the plan orders reused.** A mock drawing what the
  existing component doesn't do is guaranteed divergence — decide it in the plan, not in the Task.
- **A MEASUREMENT Task whose result depends on initial state sweeps more than one starting state —
  and declares which it swept.** A cycle measured from a single starting state reports fewer modes
  than it has; the wrong conclusion shapes the entire next Task, and the one who catches it is the
  user, not the process.

Whatever you cannot run enters marked: `<!-- NOT VERIFIED: … -->`. The executor treats that as
description, not recipe — and it is infinitely better than them finding out alone mid-Task.

Four things the plan gets wrong **silently**, each worth a round or a blocker:

- **Every claim about an external lib's BEHAVIOR carries the mark, or the installed source snippet
  pasted alongside** — not just the API name. "Option X is a watchdog" and "after `error` the lib
  stops reconnecting" are exactly the sentences that fail without warning. The type checker
  enforces API names; behavior, nobody does.
- **A Task that MOVES a file lists the old path's consumers** — and they are not just imports:
  infra (CI, deploy, installers) and **tests that sweep the tree** point by string, and a
  raw-text gate sweeping the old root goes blind to everything that moved out. Add mock helpers
  that point at paths by string and show up neither in `import` searches nor in the compiler.
- **State shared between Tasks is a design decision written in the plan's HEADER**, not inside a
  Task — it crosses Tasks approved one by one and only shows in the set review, as the number-one
  blocker.
- **The phase 1 exit gate does not close with `___` in the estimates file.** A blank
  "providers' quota" line surfaces mid-batch as provider errors and sessions hurriedly switched
  accounts. And **estimate ≥2 sessions per Task on a flaky provider** — the model's card, in
  `~/.hangar/orq/modelos/`, says how many drops per hour to expect.

## Phase 1 exit gate — a closed, method-agnostic checklist

Phase 1 only closes with the thirteen below checked, **one by one, in writing in the plan or the
contract**. Each item already exists as a rule in some section; the list exists because a rule
scattered in prose is a rule a new method doesn't know, and the missing items are exactly the
batch torn down repeatedly and the Task running for hours with no overrun yardstick. An item the
chosen method doesn't produce, **you produce by hand**.

**Each item says what it is**, because that changes your work when the user arrives with the work
already decided (a spec and ready tickets, for example):

- **AUDIT** — their material already has the answer, or the repo does; you check and paste the
  proof. Don't write a new document for this.
- **PRODUCE** — no method delivers this for free; you write it, in the orchestration plan, which
  is yours. The user's stays untouched.

Item 1 is mixed: **audit** when the material already carries files and verification per Task,
**produce** when it doesn't — which is `to-tickets`' declared case, whose template says to avoid
file paths.

1. **AUDIT/PRODUCE — Every Task has a name, a set of files and a verification** — in the user's
   plan, or in the orchestration plan you wrote next to it. If you want the phone progress bar:
   run `parse_plan` on the file that has the format and paste the output; the bar is optional, a
   Task with an owner and proof is not.
2. **PRODUCE — A-priori estimate** written, one line per Task: **expected clock and rounds**.
   Money cost does not enter — see "There is no money cap in this skill", above.
3. **AUDIT — Non-collision proven**: files per Task × `git merge-tree`, output pasted.
   **This item is what DECIDES whether a parallel batch exists, so it comes before the decision,
   not after.** Waived only when you already declared serial upfront in the plan — enforcing it
   only after the batch is declared is declaring without auditing, the circularity `SKILL.md`
   names when it orders reading the parallel page **while decomposing**.
   **Where the files come from:** the steps' text, when the material declares them; **the repo**,
   via subagent, when it doesn't. A method that carries no file paths isn't disqualified by that —
   the survey is yours, and the command is the same.
4. **AUDIT** — Shared state searched for; ownership contract written where found — **with the
   number of copies it creates and who checks the N** (or the unification Task at the end of the
   batch).
5. **PRODUCE** — A bar (or `none — user's decision`) recorded per visual Task.
6. **PRODUCE** — Long screen Task: context-rotation point planned in the steps ("step N is a safe
   switching milestone"). **How many screenshots, and whether capture becomes a separate session,
   is the executor's call at the time** — the user's decision (see "Capture belongs to the
   EXECUTOR", above).
7. **PRODUCE** — Orchestration Task: smoke step against the real source, literal command.
8. **PRODUCE** — External precondition with a declared owner in every step that waits for
   something. **A stage on a separate device or process is the case that escapes here the most**,
   because it looks like an execution detail and isn't: from which directory the server rises,
   which port belongs to each Task, who holds the device and when it is released. The executor
   decides none of that — they only prove what they carried; the missing lines cost rounds that
   stay green on the serving side.
9. **PRODUCE** — Parallel batch with visual proof: an exclusive browser per executor or proof as a
   critical section (`paralelo-worktree.md`).
10. **AUDIT** — Remaining quota of each team account, with the reading time, and the fallback
    authorized in writing.
11. **AUDIT** — Method with its executing half installed and tested — or `none`, with the
    orchestration plan written.
12. **AUDIT** — Adversarial pass offered, baseline green, every cited piece of code ran.
13. **AUDIT** — **Domain skill declared** (name or `none`), and its two checks done: no Task
    duplicates a step the skill already does internally, and no skill step was left without an
    owner (`SKILL.md`, "The DOMAIN SKILL").

And a prudence rule that is not an item but a posture: **one debut at a time.** A new planning
method, a freshly edited skill and a new provider don't enter the same run together — a run where
everything is a debut has no baseline left to tell which debut is failing.

**The same rule holds for factual claims** — in the plan, in the Task excerpt and in the
kick-off. The executor and the reviewer read the excerpt as data, not as its author's opinion; a
wrong sentence there becomes a wrong comment in the code, and a comment asserting something false
is the seed of a future bug. If you didn't measure it, write "I assume", or don't write it — an
unmeasured claim of "this would error" about a thing that actually works becomes a wrong code
comment whose correction has to be carried into the next Task.

## Phase 2 — Launch (the user's single "go ahead")

### Pre-flight, before creating any session

```bash
git status --short          # dirty tree → the paths become untouchables, listed one by one
git branch --show-current   # right branch
hangar-send --list              # WHO else is alive in this cwd
tmux display -p '#{session_name}'   # ... and WHICH OF THOSE IS YOU
```

Another session writing in this checkout blocks the launch — resolve it with that session, not
with the user. Didn't resolve → then it is their decision.

**The fourth line is not decoration: you show up in your own list.** Without it, the `working`
session in your cwd looks like a second writer, and you spend a round sending a message — to
yourself, which comes back as `[de: <you>]`.

**Branch: the question is MANDATORY at every launch, and the recommendation is a new branch off
`main`.** The user's decision, born from whole runs landing straight on `main`. Before creating
the first session, ask where the work will run, recommendation first:

> "Where does this work run? (a) **new branch off `main`** — recommended: N team commits are not
> born on the main line, and the push becomes a single decision at the end; (b) directly on
> `<current branch>`. Proposed name: `<work>`."

Creating or switching branches on your own initiative remains forbidden — the question is yours,
the choice is theirs, and the answer **goes into the contract** (`Branch: ...`, with the date). If
they ask for a `pull` first, **recheck the plan's numbers afterwards**: a pull bringing hundreds
of lines moves the lines the plan cites and can change the counts phase 1 measured.

**Green baseline before Task 1.** Run every verification command the plan defines, **once, on the
base** — still before creating any session. Only that catches two failure modes that cost a whole
round each:

- a suite already red at the starting HEAD → the first REPROVA blames the executor for inherited
  breakage, and nobody can separate theirs from what was already there;
- a command that doesn't run on this machine → DEVOLVIDO at the first review ("the verifications
  don't run"), discovered by the one who cannot fix it.

Record the result in the contract: `baseline: <command> → <green, N tests>, <date>`. Red → the
user's decision **before** launching: fix first, or record it as a known failure the review
ignores. Never launch silently on a broken base.

### Create, in order

```bash
hangar-send --new <work>-writer /path/to/repo --engine <plan's engine>
hangar-send --new <work>-review /path/to/repo --engine <other engine>
hangar-send --pair <session> "<work>: <where the contract is>"   # one call per session
```

**NEVER put the role in the `--pair` string.** It is a **GROUP** field, not the session's: every
member's sidecar holds the SAME `task`, and each new `--pair` **overwrites everyone's** and fires
a group-wide notice with that text. Pairing the executor with `"role: executor"` and then the
reviewer with `"role: independent reviewer"`, the executor receives a notice saying it is the
reviewer — and assumes it, because the message came through the infrastructure, looking like
authority: a session announcing *"the second message corrected my role"* and reading the contract
as another role is the direct product, and fixing it costs role corrections and sidecar
rewrites.

The `--pair` string is **neutral and points at the contract**, never asserts a role:

```bash
hangar-send --pair <session> "<work> — each session's role is in the grupo-<gid>.md contract"
```

Roles are declared **in the kick-off and in the contract's table**, and the contract says
explicitly that if a group notice contradicts the table, the table wins. If you already made this
mistake, fix the state, not just the text: the sidecars live in
`<config>/.hangar-pair/<session>.json`, field `task`, and can be rewritten directly (tmp+rename)
without firing a new broadcast.

Mandatory order: `--new` → `--pair` → read the `gid` in your own sidecar → **write the contract**
→ only then the kick-offs. An address pointing at a file that doesn't exist yet is a stalled
session asking questions.

A nonexistent engine returns `400` and the session is not born. List engines: `claude-engine`.

### FOUR files are born, each with one reader

- **`regras-<gid>.md`** — **this work's agreement**, which executor and reviewer read whole. Who
  is who, untouchables, gates, method, domain skill, branch, bars, accounts. Written now and
  nearly immutable after. Two pages.
- **`grupo-<gid>.md`** — the journal, which only the arbiter reads. Progress, history, dated
  decisions.
- **`licoes.md`** (in the durable directory) — the guidelines the run keeps fixing, with date and
  proof. **It grows freely and nothing leaves it.** Nobody reads it whole: the arbiter pastes into
  each kick-off the three or four that serve that Task. Born empty, header only.
- **`eventos.jsonl`** — one JSON line per event, written by the arbiter in the durable directory
  below. Nobody on the team reads it; machines do — the app's orchestration screens and the
  retrospective. Types and fields contract: `references/arbitro.md`.

And a **durable directory for the work's artifacts**, decided now and written into the rules and
into each session's first kick-off:

```
~/.hangar/orq/<date>-<gid>/{pareceres,tasks,kickoffs,visual}/
```

Review reports, Task excerpts, kick-offs and screenshots live there, **never in `/tmp`**, which
vanishes on reboot. Phase 5 reads exactly the review reports — each round's waste line is its raw
material. Deciding this mid-work costs moving files by hand.

The boundary is the content's type: **it happened → journal; it is the agreement → rules; it is a
new guideline → lessons.** Without that separation the file everyone reads grows with every
approved Task until it charges every new session the whole history. Detail in `SKILL.md`, "Three
files, each with one reader".

The **journal** skeleton is below. The **rules** one is the same thing minus the history: the
`## Quem é quem` table (fixed format from phase 2, above — it lives in the rules, not in the
journal), the literal untouchables, the gates (exact command, cwd-independent), the bar per Task,
what the review must cover, quota and accounts. **New guidelines do NOT enter here** — they go to
`licoes.md`, and from `licoes.md` into kick-offs.

### The contract is born from a skeleton, not from memory

The contract's content is described in prose across three files; rebuilding it from memory is how
a forgotten field shows up — mid-execution, as a gap nobody decided, like a reviewer reviewing
with none of the installed subagents because the tooling section was left blank. Copy and fill; a
field that doesn't apply gets `n/a`, **never disappears** — a deleted field is invisible to
whoever reads later:

````markdown
> Arbiter's journal. Group rules (what the team reads): <path to regras-<gid>.md>.
> Lessons: <path to licoes.md>. User's plan: <path>.
> Orchestration plan: <path | this very file>.
> Method: <superpowers | mattpocock | none>. Domain skill: <name | none>.
> Branch: <branch>. Starting HEAD: <hash>.

## Quem é quem
In the rules (`regras-<gid>.md`, fixed table `| papel | sessão | provider | conta | modelo | esforço |`).
Here only what is history: who took over from whom, when, why.
A group notice contradicting that table: the table wins.

## What the plan owns (point, don't copy)
Task order, steps, verification per Task, untouchables, phase-1 bars: <plan, section>.
Baseline: <command> → <result>, <date>.

## Review tooling (per Task type)
| Task type | Subagents/skills to dispatch | Don't use (reason in one line) |
|---|---|---|

## What the review must cover
<from the phase-1 plan: full flow, sibling callers, concurrency, final state, visual>

## Quota and fallback
<each team account's remaining quota, with reading time; where to migrate when it runs out>

## Bars decided AFTER plan approval
Task N — Bar: <screen, state, width> | none — user's decision, <date>

## Progress
| Task | Hash | Verdict | Who fixed (if a correction round) |
|---|---|---|---|

## Supervening decisions
<date> — <decision, whose, reason in one line>
````

### The new session proves model and effort live

`hangar-send --new --engine` does **not** configure effort, and asking for "max" in the first
prompt doesn't work. Before releasing the first Task, demand live proof from the new session (what
its statusline shows, or the switch command's return) — repeating what the kick-off asked is not
proof. Without it, the session works for hours on the wrong effort while asserting it's on the
right one.

### Messaging: native or hangar-send

Session in `ListAgents` and you have `SendMessage` → `SendMessage`. Otherwise
`hangar-send <session>`. `--new`, `--pair` and `--group` are always `hangar-send`. Long messages go
via single-quoted heredoc.
