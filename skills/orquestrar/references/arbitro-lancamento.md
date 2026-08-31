# Arbiter — the launch (phase 2)

This page belongs to the **moment the team is born**: picking accounts, checking tooling and
opening the sessions. You read it **once**, before Task 1, and don't come back to it — except to
open a new session midway (rotation, replacement).

Return to `arbitro.md` as soon as the team is standing.

## Before the team: read the machine's account policy

**The policy lives in `~/.hangar/orquestracao-contas.md`, not here.** Read it before opening the
first session and **copy into the contract only what this work will use**, in the rules'
`## Quem é quem` table. Don't relay the whole file: sessions choose by what is in the contract.

Three reading rules, all three protecting the same thing — the invoice of whoever trusted you:

- **The table rules, not the prose.** The file has a table of enabled accounts, written by the
  panel when the user turns an account on or off from the screen. **An account outside the table
  is forbidden**, even if a paragraph below seems to allow it: prose ages, the table is what they
  touched last.
- **An account that charges per token is forbidden.** You discover that an account exists; only
  the user knows whether it debits. Discovery lists provider, model and address — none of that
  says whose account it is or whether they want to spend there. A new provider that appeared since
  the last review **does not enter on its own**. On a real machine, 341 of the catalog's 390
  models belonged to a per-token provider.
- **File missing or stale → build the inventory and ask ONE question** (which are allowed, which
  are subscriptions, which charge), write the answer there with the date, and **open no session**
  until it arrives — not even "just to test". The survey recipe is inside the file itself.

**The message "the group's model configuration changed in the panel"** comes from the screen, not
from a session: the row is already in `regras-<gid>.md`. Re-read the file and apply — switching
account or model **is** closing and reopening; Claude doesn't switch with the session open:

| That role's session is… | What to do |
|---|---|
| idle | close it and open another already on the new configuration |
| working | let it finish; the **next** one is born on the new one. Its context is worth more than the model |
| you (the arbiter) | finish the task at hand and pass the baton via the "Arbiter succession" rite |

Answer the message only if it asks for an answer.

## Survey the tooling BEFORE opening the team

A new session doesn't know what the machine has. If you don't say, each one reviews and builds by
whatever method it invents — a reviewer with that part of the contract left blank finds real
blockers **using none** of the installed review subagents, and the lost coverage is invisible.

One sweep, once, at the start. Then **write into the contract a table per work type** — which
subagents and skills the reviewer dispatches, and which help the executor deliver. Every new
session receives it ready, instead of discovering alone (or not discovering).

Look at the three shelves: **subagents** (per-language and per-dimension reviewers — silent
failure, security, accessibility, test coverage), **skills** (click-path audit, security review,
production readiness, browser QA, house patterns) and marketplace **commands**.

And run each through the **three questions** of `SKILL.md` ("An outside tool — skill, subagent,
command"): does it exist under that name, does it serve the flow, does it serve the files. All
three have already failed here in the same sweep — a tool announced as a skill that was a command,
the same one building a diff of uncommitted changes for a gate that reviewed made commits, and a
per-language reviewer whose extension filter couldn't see the file type where the work's two
screen blockers lived.

A tool that fails the three: record in the contract **why it doesn't serve**, one line. That is
worth as much as the list of what to use — it saves the next session a turn of trying.

## Opening a session — a recipe, not a decision

**Exception:** the reviewer's **verification session** is not yours. The reviewer opens, drives
and closes it alone, without asking you — it is their arm for running the app, clicking screens
and capturing shots, and what reaches you is still only the review report. Don't create, manage or
demand reports from it. **Its model is not the reviewer's choice**: it comes from the contract,
like everyone's — but the reviewer creates and checks it, not you.

### A rotating role: which table row holds for this Task

The `## Quem é quem` table gains a seventh column, `vez`, **only** when some role rotates between
accounts. Without rotation, it doesn't exist and nothing here changes.

A role with a numeric `vez` has more than one row, one per account, and **Task N belongs to that
role's row of index `(N-1) % total`**, in table order. Nobody decides whose turn it is — it is
arithmetic over the Task number, and that is why two sessions doing the math separately reach the
same result without coordinating. The two easy mistakes: it is `(N-1)`, not `N` (Task 1 uses the
**first** row), and the cycle restarts — with 3 accounts, Task 4 belongs to the first again, not
to a continuation of 3.

Rotation is **not** parallelism: within a Task there is **one** session of that role, on the
turn's account. Running Tasks at the same time is another mechanism — a worktree per Task,
declared in the PLAN, in `paralelo-worktree.md` — and the "one round, ONE reviewer" rule holds
whole in both cases.

This holds for every session you create. The five steps are **one unit**: the turn doesn't close
in their middle.

1. **Create on the agent's default account:** `hangar-send --new <name> <cwd>`, **without**
   `--engine`. A provider engine enters **only** when the plan named one: `--engine <engine>`.
   *"An <agent> session"* means that agent's default account. That vendor's model reachable
   through a gateway, router or API is **not** a session of that agent — it is another provider
   serving a similar model, with another account and other behavior.

   **Model, effort and permission go ON the `hangar-send --new` itself**:
   `--model <id>`, `--effort <level>` and `--permissao <mode>`. A contract that names model and
   thinking (the normal case when the team runs on Pi) fits in the command — the session is
   already born on it:

   ```bash
   hangar-send --new <name> <repo> --provider pi --model <provider>/<id> --effort <level>
   ```

   On Pi the `--effort` becomes `--thinking` (also accepts `off|minimal`); on Kimi only
   `--model`; `--permissao` is Claude-only. The backend validates **before** any effect on disk: a
   model outside the regex, a level outside the closed list or an unknown provider return 400 and
   the session is **not born** — never a session that looks like it's on the right model and
   isn't. The alternative path (create without the flags and switch later via `/cp-model` +
   `/cp-think`) works, but leaves the session alive for an interval on the wrong model, and
   contradicts step 2 below. (An install with an old `hangar-send`, without the flags: the direct
   POST to the API with `model`/`effort`/`permission_mode` in the body remains plan B.)

2. **Prove what was born**, reading the session's **real** engine/model, never what you asked
   for. Diverged from the plan → delete and recreate. The wrong session receiving the request is a
   whole work in the wrong place, and the datum that exposes it appears before any error.

   Two proofs, and you want both — they fail for different reasons:

   ```bash
   tmux display -p -t "=<name>:" '#{pane_start_command}'   # the real argv the pane started with
   ```

   That shows `exec pi --session-id … --model <provider>/<id> --thinking <level>` and proves the
   **request** became a command. It doesn't prove what the agent **accepted**: Pi truncates the
   level to what the model supports, so also demand the **live** proof from the session itself
   (statusline, or `/cp-think`'s return) on its first turn, before its first `Edit`. Repeating
   what the kick-off asked is not proof.

   Don't read `/proc/<pid>/cmdline` expecting the flags: Pi rewrites its own argv and the cmdline
   shows only `pi` — which looks, for a minute, like a session created with no model at all.

   **And a model proof proves the model, not the HARNESS.** A Claude Code session with an engine
   pointed at provider X and a Pi session running model X show **the same status line**. What
   distinguishes them is the `pane_start_command` (`claude` × `pi`) and the `provider` the API
   returns — check both. An executor born in the wrong shape can still prove model and effort
   correctly — the harness check is what catches it, and it is cheap only while the tree is still
   clean.

   Also: **proof via status sidecar must match the `session-id` of the live session** — the
   directory keeps one file per id and doesn't delete them when a session dies. Two of those
   three read the sidecar of the dead session that previously occupied the pane, and the value
   came out right by coincidence.
3. **Write the request into a file** and deliver with `hangar-send <name> "$(cat <file>)"`. A
   long request typed straight on the line breaks: `|`, `$`, backticks and the `|` of "YES | NO"
   become commands, and the message goes out mutilated or not at all.
4. **Check the return.** `entregue -> <name>` is delivery. Anything else — `404`, a usage error,
   silence — is **not delivered**: resend, don't move on.
   **And `entregue` proves delivery, not EXECUTION.** Before recording (or reporting) that the
   session is working, check engagement: its ctx left zero on the statusline, or the pane is
   processing. A session that received the kick-off and died on a provider timeout sits `idle`
   with the same face as an idle session — ctx frozen near zero with a retry error in the pane,
   while the Task gets reported as "running". On the resend, point only at the kick-off's PATH;
   the ctx leaving zero within a minute is the cheap proof.
5. Only then does the turn close. **A session opened with an undelivered request is a session
   nobody will use** and that you will believe is working.
