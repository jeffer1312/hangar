# Executor — the flow Task

This page belongs to the Task that creates or changes orchestration: tmux, CLI, process,
account, network. It holds even when the plan has no smoke step. Not your kind of Task: the
cycle is in `executor.md`.

## Run the flow

- Before sending the round, run the flow end to end against the real source (the real tmux, the
  real CLI, the real test account) and paste into the report what happened.
- A suite count that drops below the base is a mandatory note in the report.
- A double replaces the I/O, never the function under correction.
- A test that swaps the whole library for a double proves the button calls the function, not
  where the function goes: a Task that changes destination, credential or target delivers a
  test with the real libraries, with an internal control (the neighboring screen that already
  gets it right, measured in the same test).
- Proof of a two-ended flow is the content of both ends (the two files, the two identifiers),
  never a badge the screen itself paints.
- The evidence carries what distinguishes the two paths: proof of "it went to the right server"
  says which one was active at that instant.

## Report line

```
Flow: <command run end to end against the real source> → <what happened, pasted> · suite count <base → now>
```
