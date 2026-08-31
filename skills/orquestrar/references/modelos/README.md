# Model cards — what each one does well, badly, and what that changes in the plan

One card per model, short. They exist for the **planner** to read before writing the plan
(`planejamento.md`, "The TEAM is decided BEFORE writing the plan") and for the
**retrospective** to update at the end (`retrospectiva.md`).

**The cards live in the vault — `~/.hangar/orq/modelos/` — not in this skill.** They are dated
records of the user's machine (the orchestration works whole without them), and they age by
construction. What lives here is only this page: the rule of how a card is written.

## Rules for these files

1. **Measured facts only, dated.** "Seems better at X" doesn't enter. "On 2026-08-15, 3 rounds
   against the siblings' 2, 2.3× the day's cost" enters.
2. **What changes in the PLAN, not praise.** Every card line answers: *what do I write
   differently because of this?* If it changes nothing, it isn't card material, it is trivia.
3. **Short — 40 lines.** A card nobody reads whole protects nobody. What aged leaves; the date
   gives it away.
4. **Not an accounts table.** Price, quota and permission live in
   `~/.claude/orquestracao-contas.md`, which belongs to the machine and whose decider is the
   user. Here is behavior.
5. **One run doesn't make a card.** A pattern observed once enters as "seen once, on <date>".
   Two agreeing runs become an assertion.

## File name

`~/.hangar/orq/modelos/<provider>-<id>.md` — the same pair `--model` receives, so there is no
doubt which one it is.

## A new model on the team: research first, but keep it apart

A model that never worked here has no card. Before writing the plan, do **one** short sweep and
record it in its own section. Two sources, and the second is usually worth more:

- **The vendor** — the prompting guide, release notes, published limits. It says what the model
  should do.
- **The community** — what real users discovered, including what the vendor doesn't tell: where
  it breaks, which workaround became standard, which tool people pair it with. Use the
  **`last30days`** skill for the wide sweep (Reddit, HN, X, YouTube, last 30 days) and then go
  deep on the two or three sources that repeat.

```markdown
## What they say  <!-- HYPOTHESIS — not tested here -->
- <recommendation> — vendor, read on <date>
- <finding> — community (<where>), read on <date>
```

Why the community pays more: it reports the **limitation and the workaround together**. Two
examples that arrived that way on 2026-08-15, which no official guide would carry: that the
cheap executor model is blind and a pair of CLIs gives it sight; and a self-improvement loop
where one model executes and another notes each lap's waste — the second became a rule of this
skill the same day (`revisor.md`, the WASTE line).

Two rules, and the second is the point:

1. **A separate section, always.** The vendor's guide and our measurement never mix in the same
   paragraph — whoever reads the card three months on must know what was tested here and what
   was read somewhere.
2. **When they diverge, the measured wins, and the divergence stays written.** It is the card's
   most valuable information: it is where the model behaves differently from the advertised in
   *this* kind of work. An example from 2026-08-15: the Opus 5 guide advises against extra
   verification instructions ("it verifies on its own, and asking more makes it over-verify") —
   and what solved things here was demanding an explicit read command, because the failure
   wasn't lack of care, it was truncating its own check with `head`.

A vendor recommendation that was **never tested here** stays marked as a hypothesis until a run
confirms it. Don't turn it into a kick-off rule without measurement: the kick-off is where the
cost shows.

## What the card answers, in order

- **Window and practical ceiling** — and what a typical Task of this work costs on it.
- **Does it see images?** — decides whether the visual bar needs code or a screenshot is enough.
- **How it fails** — the pattern, not the isolated case. The section that pays most.
- **What the kick-off must say because of it.**
- **Where it is good** — so it isn't wasted in the wrong role.
