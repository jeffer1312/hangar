# Arbiter — launch, sessions and kick-offs

Read whole at step 1 of `arbitro.md`; "Opening a session" and "Kick-off" again whenever a
session is opened or a Task released. Back to `arbitro.md` once the team stands.

## Launch (phase 2)

1. Pre-flight:

   ```bash
   git status --short                  # dirty tree → the paths become untouchables, one by one
   git branch --show-current
   hangar-send --list                  # who else is alive in this cwd
   echo "$CP_SESSION_NAME"             # which of those is you; renamed → MCP who_am_i
   ```

   Another session writing in this checkout → resolve it with that session; unresolved → the
   user decides; the launch waits.
2. Ask where the work runs, at every launch, recommendation first: "Where does this work run?
   (a) **new branch off `main`** — recommended; (b) directly on `<current branch>`. Proposed
   name: `<work>`." The user creates or switches the branch, never you. The answer goes in the
   contract (`Branch:`, with the date). Pull requested → recheck the plan's numbers afterwards.
3. Green baseline: run every verification command once on the base, before any session.
   Record `baseline: <command> → <result>, <date>`. Red → the user decides before launch: fix
   first, or record a known failure the review ignores.
4. Read the account policy and write the contract lines ("Account policy", "Contract lines you
   write", below) and the closing items (`arbitro-encerramento.md`); write the a-priori
   estimate: time and rounds per Task.
5. Survey the tooling (below), once.
6. Create, in order: `--new` ("Opening a session", below) → `--pair` → read the `gid` in your
   own sidecar → write the contract (skeleton in `planejamento-equipe.md`) → arm the watchdog
   and prove it (`arbitro-vigia.md`, "Arming") → kick-offs ("Kick-off", below).

   ```bash
   hangar-send --pair <session> "<work> — each session's role is in the regras-<gid>.md contract"   # one call per session
   ```

   The `--pair` string names the work and the contract, nothing more; roles live in the
   kick-off and in the table, and the contract says the table wins over any group notice. Wrong
   string already sent → rewrite `task` in `<config>/.hangar-pair/<session>.json` (tmp+rename).

Done when the five items of `arbitro.md`, step 1, stand.

## Contract lines you write

- `Method:`, `Executes with:`, `Domain skill:`, `Route:` — mandatory in `regras-<gid>.md`, written before the first session, repeated in every kick-off.
- No `Method:` → ask the user; write the line with its `Executes with:` before proceeding.
- `Route:` comes from phase 1 and only escalates, `audit` → `full`, via `replanejar.md`. `audit` has no arbiter: this page is not read.
- The method holds to the end for everyone; a switch the user asks for is `replanejar.md`.
- A user restriction: copy the exact command, the reason with its number, and what remains allowed, at the extent given. Never widen it; in doubt about extent, ask.

```markdown
Forbidden: `<heavy checker>` in this repo — <reason, measured>.
Allowed: `<cheap variant>` — <what it catches>.
```

- Permission to touch an untouchable enters the contract before the dispatch, on the untouchable's line, with scope and date; the kick-off carries the list with the exception inside.

```markdown
Untouchables: <path/to/file>, <other/path>
  - EXCEPTION: `<file>` released for Task 7, only function `<name>` — user, <date>.
```

- Off-plan Task (finding promoted to work, fresh user request, finishing touch) has no row in `## Quem é quem`: ask one question with a proposal and the measured why, from the cards in `~/.hangar/orq/modelos/` and this journal: "New Task: <what, nature>. I propose <role: session/model/account>, because <history, number>. Keep the plan's team, or switch?" Record the answer as its own row, dated.

## Account policy

- Read `~/.hangar/orquestracao-contas.md` before the first session. Copy into `## Quem é quem` only what this work uses; the file itself stays with you.
- The table rules, not the prose. An account outside the table is forbidden.
- An account that charges per token is forbidden. A newly discovered provider enters only by the user's hand.
- File missing or stale → build the inventory (recipe inside the file), ask ONE question (allowed / subscription / charges), write the answer there with the date, open no session until it arrives.
- Message "the group's model configuration changed in the panel" → re-read the file and apply. Switching account or model = close and reopen. Answer only if the message asks.

| That role's session is… | Do |
|---|---|
| idle | close; open another on the new configuration |
| working | let it finish; the next one is born on the new configuration |
| you | finish the task at hand; "Arbiter succession" in `arbitro-encerramento.md` |

## Survey the tooling

Once at launch: subagents (per-language and per-dimension reviewers), skills (click-path audit, security review, production readiness, browser QA, house patterns), marketplace commands. Write into the contract a table per work type: what the reviewer dispatches, what helps the executor.

Outside tool (skill, subagent, command), three questions: exists under that name in this account; reads where this round's code is (uncommitted); reads this Task's files (pass paths explicitly). Failed → record why, one line. A tool's silence counts only if you know what it read.

## Locks on model and tools

- The model is the user's decision. The contract carries the account↔model table per role; a model outside it is not used even to test; the model comes from the ROLE, including bug worktrees and one-off tasks. Need one outside the table → stop and ask.
- Before creating any session: re-read its row and state in the message which engine/model you use and where it came from.
- A new session is born on the harness default → switch, read the model back, only then send work.
- Subagents: same account always; a model switch inside it only where the contract allows; an agent frontmatter `model:` overrides yours.

## Who you open

- The cast is the contract's table, never `hangar-send --list`.
- The verifier session is the reviewer's: they open it, send the script, record `consumo.md` snapshots and close it, from the optional `verificador` line. You open neither; you receive only the review report.

## A rotating role

- The `vez` column exists only when a role rotates between accounts.
- Numeric `vez`: Task N uses that role's row of index `(N-1) % total`, in table order.
- Risk `vez` (`low` | `high`): Task N uses the row named by its `Risk:` line in the orchestration plan. A line raised mid-work lands on `high` from the next session on.
- One selector per role, never both. Rotation is not parallelism: one session of the role per Task.

## Opening a session — five steps, one unit

1. Create on the agent's default account: `hangar-send --new <name> <cwd>`. "An <agent> session" = that agent's default account; the same model through a gateway or router is another provider. `--engine <engine>` only when the plan named one. Model, effort and permission go on the command (`--model <id> --effort <level> --permissao <mode>`; Pi: `--effort` → `--thinking`; Kimi: `--model` only; `--permissao` Claude-only). The row's `abertura` cell goes on the command as written (it may carry `--engine` and `--permissao`); a Codex `conta` other than `openai-codex` → `--conta <name>`. A 400 = session not born: recreate with the flags right, never create-then-switch. Old `hangar-send` without the flags → POST to the API with `model`/`effort`/`permission_mode`.

   ```bash
   hangar-send --new <name> <repo> --provider pi --model <provider>/<id> --effort <level>
   ```

   Research, review, final review, verification: add `--read-only` and prove the protection (`protecao.md`). Record the initial `consumo.md` snapshot before the first request and the final one at close or replacement — also for the executor and for your own period.

2. Prove what was born (below). Diverged → delete and recreate.
3. Write the request in a file; deliver with `hangar-send <name> "$(cat <file>)"`.
4. Check the return: `entregue -> <name>` is delivery; anything else → resend. Then check engagement: ctx left zero within a minute. On a resend, point only at the kick-off's path.
5. Only then the turn closes.

Done when the proof is taken, the kick-off engaged, and the row's model read back from the live session.

### Prove what was born

The real engine, model and harness, never the request:

- With a terminal, `tmux display -p -t "=<name>:" '#{pane_start_command}'`; without one, its sidecar in `~/.hangar/claude-headless/` or `~/.hangar/codex-sessions/`: the request became a command, and the harness (`claude` × `pi`) plus the API's `provider` match the row.
- Live proof from the session (statusline or `/cp-think` return) on its first turn, before its first `Edit`. Echoing the kick-off is not proof.
- The proof comes from the pane and the live session; `/proc/<pid>/cmdline` stays unread. A sidecar proof must match the live session's `session-id`.
- A proof belongs to the session it was taken from: another row = another session = another proof.
- A model's capability (images) is proven in the session, one `Read` on a PNG; never copied from another work's contract.

## Kick-off — the message points, it doesn't copy

```
Read ~/.claude/skills/orquestrar/references/<executor|revisor|revisao-final|retrospectiva>.md — your role's page, plus the sibling pages it names; nothing else of that skill.
Role: <executor | reviewer | branch review | retrospective>.
Method: <name | none — the plan is the user's>.   Executes with: <command | none>.
Domain skill: <name | none>.   Route: <audit | full>.
Repo/branch: <path> / <branch>.   Expected HEAD: <hash>.
Baseline (<hash>): backend N · check N · front N + <named known red>.
Group rules: <path to regras-<gid>.md>.
Durable dir: <~/.hangar/orq/<date>-<gid>/ with {pareceres,tasks,kickoffs,visual}/ — reports, diffs and screenshots go here, never /tmp>.
The current Task: <path to its file>.
Untouchables: <paths, one by one, exceptions inside — not "the ones in the contract">.
Lessons that apply to this Task: <pasted here, 3 or 4, not the file path>.
Reviewer for this Task: <session>.        ← executor kick-offs only
Frozen round: <hash> · the dirty tree is YOURS.   ← only when you replace an executor midway
Your turn now: <Task N | wait for the first round>.
When done, send the round to <reviewer-session> and STOP.

Read ONLY these files. The whole plan, the journal and the lessons file are NOT yours.
```

- Lessons go pasted, never as a path. Every visual Task kick-off also pastes the visual-proof invalidators (`executor-visual.md`), even when in the contract.
- The same text re-sent puts a `/clear`-ed session back. No line carries turn state; "Task 2 already passed" belongs to the contract.
- Sending someone to check a set → the command that discovers the list, never the list (`arbitro.md`, "Release one Task", handoff checklist).

### Tightened criterion (reviewer kick-off, spiral with the user unavailable)

Write it in the kick-off; it loosens nothing:

- a blocker is what a real user reaches, and the report writes how to get there;
- a case that exists only by fabricating a race in a test is a NOTE;
- still full blockers: screen that doesn't mount, focus trapped/lost outside the modal, dead contract, wrong text on screen, gate regression, untouchable in the commit;
- declare the family limit: "another variation of this same defect is a note".

## One Task per executor, never the whole plan

- Monolithic plan → excerpt that Task's section plus the short header (goal/architecture) into `~/.hangar/orq/<date>-<gid>/tasks/task-<N>.md`; send that path.
- One file per unit (tickets) → point at the user's file, no copy; paste the context the ticket lacks into the kick-off, 3–4 lines.

## Visual Task without a bar

- Ask before releasing, with 2–3 verified candidates plus `no bar` (`planejamento-equipe.md`, "The bar"). No conservative default here.
- Record on the Task's line either way. A recorded `none` counts as a bar for a NEW surface.

```markdown
Task 3 — Bar: `EnginesSheet.svelte`, desktop 1440px, centered modal
Task 5 — Bar: none — user's decision
```

- A Task that REPLACES a surface: the bar is the thing replaced; `none` is not an answer; nothing to ask. The Task enters with an inventory of the old surface: one line per thing a person can do or read (field, toggle, hint, warning, button, empty text), per mode (create × edit, empty × filled), with where each is now. Visible counts; behind an unannounced click doesn't. Method: interface keys of `git show <base>:<file>` minus the new files' keys. The inventory goes into the ready criterion; the reviewer checks item by item.
