# Executor — subagents inside your session

Read at step 3 of `executor.md` before the first subagent, and at step 4 on the first round.

| The steps… | Run |
|---|---|
| touch disjoint file sets | one subagent per set, in parallel |
| one needs the other's output, or touch the same file | you, in series |
| are reads (callers, flow tracing, precedents) | subagents, in parallel |

- Each arm receives the literal list of files it may touch.
- Arms edit and report to you. Git, the Task's verification, plan boxes, the contract and
  every message to another session stay with you.
- After all return: read what each did, run the verification once (step 4), then freeze. The
  round report says what each arm touched.
- First round, before sending: dispatch the machine's reviewer subagents from the contract's
  tooling table, in parallel, with the Task's explicit paths. Correction round: re-run only
  when the fix grew beyond the recipe (new file, new symbol, a step the recipe did not name).
- An arm returning something you do not understand, or outside its file list → undo its part
  and redo it yourself.

## Report line

```
Subagents: <arm → files it touched, or "none"> · reviewers: <names dispatched from the contract's tooling table | "the contract has no table" | "waived by <who>">
```
