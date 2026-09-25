# Planner — the team, the plan skeleton, the bars, the contract and the launch

Sibling of `planejamento.md`: "The team" is read at its step 3 ("Team first"), "The plan
skeleton" and "The bar" at step 4, "Phase 2" and the contract skeleton at step 6. The
decomposition and the exit gate stay in `planejamento.md`.

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

## The team: you propose, the user chooses

1. Read `~/.hangar/orquestracao-contas.md`: it says what MAY be used, never what WILL. Copy into
   the contract only what this work uses. Missing or stale → take the inventory, ask, write the
   answer there with the date.
2. Take the inventory:

```bash
claude-engine                                        # engines
pi --list-models | awk 'NR>1{print $1}' | sort -u    # Pi providers — from the USER's shell (`fish -l -c`)
ls -d ~/.claude ~/.claude-*                          # Claude accounts
```

   Harness ≠ engine: list `--provider pi` and own CLIs too. Same-named models → full `provider/id`.
3. Ask once, proceed on any answer:

> "Route `<audit | full>` (because <reason>). Do you want to pick the team (account and model per
> role), or do we go with the default?"

4. Wants to pick → two or three combinations from the inventory, they decide. No, or no answer →
   the default on the account in use: executor Opus effort `medium`; reviewer, arbiter, final
   review and retrospective Opus effort `high`; rows marked `default` with the date. Leaving the
   account in use, or entering a per-token account, requires the user's word.
5. Cast per Task, from the inventory (a model in an example is an example): mechanical volume,
   subtle reasoning or visual judgment → who writes (possibly one writer per Task); where its error
   shows → what the reviewer must be able to do; visual Task with an executor that cannot see
   images → `executor-visual.md` vision protocol in the contract; account and quota → engines and
   fallback.
6. One role, one session; each role gets its own. Phase switch (planner → arbiter) and succession
   are not stacking. Final review in a fresh session. One writer per tree.
7. Write the table below into `regras-<gid>.md`.

Done when the table is written and every row carries account, model and effort.

## `## Quem é quem`

Team table in `regras-<gid>.md`, raw values only (`-` = empty). Start from
`<pair_dir>/regras-padrao.md` when it exists, adjusting only session names:

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

- `provider`: `claude` | `codex` | `pi` | `kimi`. `conta`: config-dir name on Claude (`padrao`,
  `200-01`); provider in `~/.kimi-code/config.toml` on Kimi; catalog provider on Pi;
  `openai-codex` (default account) or the account name (`~/.codex-<name>`) on Codex. `sessão`
  ending in `*` = one session per Task.
- Optional `abertura` column, last: the `hangar-send --new` flags the role opens with
  (`--headless`, `--permissao <mode>`, `--engine <engine>`, `--subagente <model>`, `--jev`);
  `-` = defaults. The panel writes it; copy it as is.
- Optional `janela` column, before `abertura`: the % of the session's own context window at
  which the arbiter decides the role's handover (`60%`); `-` = 50%. A reference, never an order
  to stop. The panel writes it; the watchdog reads it.
- Optional `verificador` row (`<work>-verif-*`, own account/model/effort): delivers proofs; the
  reviewer still decides. Without it the reviewer runs the tests. It enters a running contract
  only with the user's authorization.
- Optional `vez` column (`| papel | vez | sessão | …`), one row per value; a role uses one
  selector: rotation (`vez` = 1, 2, 3; Task N → row `(N-1) % total`) or risk (`vez` = `low`/`high`;
  Task N → the row its `Risk:` line names). Rule in `arbitro-lancamento.md`.
- No review round goes to the arbiter, the last one included: the reviewer row judges every
  round, and the arbiter wakes for decisions and the next Task.
- All pipeline roles in the table, phases 4 and 5 included. Final review with its trigger: "fires
  when every code Task is approved", never "after Task N".
- The table is machine-read: cells carry raw values, explanations go outside the table.
- Below the table, one literal open command per role. Research, review, final review and
  verification commands carry `--read-only` (`protecao.md`); declare where writing tests run and
  where reports go; record the measurement command and its owner (`consumo.md`).
- More than one repository: header `Repo: <one> (+ <other> from T13 on)`; each row is born in its
  Task's repo; one writer per tree. Interfaces agreed before the sessions open:

```markdown
## Interfaces combinadas
- <route, payload, event or type agreed between the repos>
```

- Subagents read, sessions write: editing outside the cwd requires a real session in that repo. A
  session on another server (`servidor::sessao`) joins no group: 1:1 messages, and the agreement
  written into the local contract by hand.

## The plan skeleton

The orchestration plan (`planejamento.md`, step 4): a second short file pointing at the user's
plan, adding only what the gate needs.

```markdown
# Orchestration plan — <work>
User's plan: <absolute path>   (it is in charge; this file only orchestrates)

## Tasks
| # | What it is | Where in their plan | Files | Verification | Proof | Tela |
|---|---|---|---|---|---|---|
| 1 | create the schema | section "Database", 2nd paragraph | `<paths>` | `<test command>` | green suite + the table exists | — |
| 2 | <screen Task> | <where> | `<paths>` | `<test command>` | screenshots of <states> | `Tela: própria` (its wave's) |

## What their plan does NOT decide, and I decided here
- Waves: 1 (T1, T2) · 2 (T3). Wave with screen proof: `Tela: própria | compartilhada`, repeated
  in each of its Tasks (column `Tela`).
- Untouchables: <paths>.
- Bar for Task 2: <screen, width>.
```

## The bar

- One line per pixel-touching Task. Three tests, all mandatory: named (a specific screen), findable
  (absolute screenshot path, or an app screen that can be opened), comparable (same state, same
  width). A Task that draws nothing needs no bar.
- Propose, instead of asking "what's the bar?": two or three candidates already passed through the
  three tests, one sentence each on why it is hard, plus "no bar":

```
Task 3 touches the Settings sheet. Bar — pick one:
a) `EnginesSheet.svelte`, desktop, centered modal, 1440px — same `wide`/`centered` pair.
b) `Git.svelte`, same width — same glass material, with tabs.
c) A screenshot from another product — send me the path.
d) No bar for this Task.
```

- "No bar" chosen → `Bar: none — user's decision, <date>`; that Task's visual gate is the
  `executor-visual.md` protocol without the blind comparison.
- Weak candidates → say so and propose others.

## Phase 2 — Launch (the user's single "go ahead")

- On `full`: you are now the arbiter. Read `arbitro.md` and run the launch of
  `arbitro-lancamento.md` ("Launch"): pre-flight, branch question, baseline, sessions, contract,
  kick-offs.
- On `audit`: open no session. Run the pre-flight, the branch question and the baseline of
  `arbitro-lancamento.md`; write the contract (skeleton below; three-row table, `Route: audit`);
  the journal goes through `orq log`; write Task 1 yourself. Open phase 4's session when the
  last Task is committed, phase 5's after the branch is in the user's hands.

### The contract skeleton

Copy into `regras-<gid>.md` and fill; a field that doesn't apply gets `n/a` and stays in place.
Common part ≤ 8k characters; each Task's specifics in a `## Task N` section at the end, read by
executor and reviewer through `orq read contract --task N`. History (who took over from whom,
when, why) goes to the journal through `orq event sessao_trocada`, never here.

````markdown
<first lines: `arbitro.md`, "The four files">
> Lessons: <path to licoes.md>. User's plan: <path>.
> Orchestration plan: <path | this very file>. Starting HEAD: <hash>.

## Quem é quem
<the team table, "`## Quem é quem`" above>
A group notice contradicting that table: the table wins.

## What the plan owns (point, don't copy)
Task order, steps, verification per Task, untouchables, phase-1 bars: <plan, section>.
Baseline: <command> → <result>, <date>.

## Review tooling (per Task type)
| Task type | Subagents/skills to dispatch | Don't use (reason in one line) |
|---|---|---|

## What the review must cover
<full flow, sibling callers, concurrency, final state, visual>

## Quota and fallback
<remaining quota per account, with reading time; where to migrate when it runs out>

## Bars decided AFTER plan approval
Task N — Bar: <screen, state, width> | none — user's decision, <date>

## Progress
| Task | Hash | Verdict | Who fixed |
|---|---|---|---|

## Supervening decisions
<date> — <decision, whose, reason in one line>

## Task N — <title>
<that Task's specifics: bar, its wave's `Tela:` line, its port and worktree, untouchable exceptions, recipe path>
````
