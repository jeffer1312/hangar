# Executor — the flow Task

This page belongs to the **Task that creates or changes orchestration** — tmux, CLI, process,
account, network. It holds even if the plan has no smoke step: an incomplete plan is not
permission to skip. If your Task isn't one of these, it isn't yours; the cycle is in
`executor.md`.

## You must RUN the flow

**A primitive's double returns what the PRIMITIVE returns.** A fake reproducing your assumption
about tmux proves the assumption, not tmux. A Task can deliver with thousands of green tests and
the whole flow dead — hundreds of lines of new tests passing with the module inoperative, because
the fakes assume exactly what the code assumes and no test demands the real interaction. The
blockers are found by whoever runs against the real source.

- Before the commit, **run the flow end to end against the real source** — the real tmux, the
  real CLI, the real test account — and paste into the report what happened, not what the tests
  say would happen.
- **A suite count that DROPS becomes a mandatory note in the report.** One unit below the base
  is half a report: a silent drop is a deleted test of an approved Task.

The rule has two halves:

1. **The double replaces the I/O, never the function under correction.** An account label used
   as a directory path survives rounds of green suite when the double reproduces the code's
   assumption instead of checking it.
2. **A test that swaps the whole library for a double proves the button calls the function —
   never where the function goes.** Test files swapping the network libraries for doubles let
   gate after gate approve a screen that promises one server and acts on another — deleting
   account and conversations on the wrong machine, and sending the login credential to the wrong
   host. The missing test has a handful of cases and is born in minutes. And the type gate
   **doesn't catch it either**: mutating a function's body back to the wrong client leaves the
   whole repository at zero errors.

Hence the form rule: **a Task that changes destination, credential or target delivers a test
with the real libraries**, and the best format has an **internal control** — the neighboring
screen that already gets it right, measured in the same test. That is how the set review proved
the blocker instead of arguing it.

And two outcome rules of the same family:

- **Proof of a two-ended flow is the content of both ends** (the two files, the two
  identifiers), **never the badge the screen itself paints**. A hard-coded badge shows whatever
  it was painted to show while the real defect is the return leg asking the wrong server about
  itself. Two twenty-second `cat`s spare the round.
- **The evidence must carry what distinguishes the two paths.** Proof of "it went to the right
  server" carries **which one was active at that instant** — otherwise it doesn't separate "it
  went to the owner" from "the active one already was the owner". "After" logs showing the call
  that only goes to the active one, dozens of times on one side and none on the other, proved
  nothing; what separated them was a component test with the two sides inverted.
