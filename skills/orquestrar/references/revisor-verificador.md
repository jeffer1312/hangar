# Reviewer — the verifier session

Read at step 3 of `revisor.md`, when the contract has the optional `verificador` line (account,
model and effort approved at planning). Without the line, verify in your own session on your
own model. Rotation, when the line has it, uses the current Task; in the final review the turn
must be defined in the contract.

Grunt work it takes: a disposable environment, click scripts, captures, suite runs. Judgment
stays yours: you read the screenshots, and a finding you did not reproduce is no blocker.

```bash
hangar-send --new <work>-verif-<task> <worktree> --provider <provider> --model <id> --effort <level> --read-only   # + the account flag of the line
# 1. read model and effort back
# 2. capture the consumption start (consumo.md)
hangar-send <work>-verif-<task> "<closed script>"
# 3. capture the end, then: hangar-send --close <work>-verif-<task>
```

- The script is closed: exact steps, states to capture, absolute save paths, what to report
  (command run, raw output, each file's path), the object and base under test, the protection
  of `protecao.md`, the artifact destination.
- The verifier runs and reports failures to you only; fixes, architecture and approval stay
  with the roles that own them. An unexpected result comes back to you for the next test.
- You open, drive and close it; one per Task, closed when the round is done. Subagents stay on
  the account of the session that opened them.
