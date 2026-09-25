# Executor — verification that does not lie

Read at step 4 of `executor.md`.

## Run

- The command the plan defined for this Task, cwd-independent (explicit prefix or directory).
- Once per round, after the last edit; never between edits. Red → rerun only what failed.
- The round's verification commands go out together: chained in one Bash, or parallel calls in
  one message; never one command per response.
- `set -o pipefail` or `${PIPESTATUS[0]}`: `command | tail && echo OK` prints OK on failure.
- Before sending (the round is uncommitted, so diff against HEAD, never `<base>..HEAD`):
  `git diff HEAD -- <file>` shows only what the Task asked; check removed lines with
  `git diff HEAD | grep -E '^-.*(role=|aria-|try|catch|await)'`.
- Output goes to a file in the durable directory (`<command> > <durable>/out-task-<N>-r<R>.txt
  2>&1`); read `tail -n 40` and `grep -nE 'FAIL|Error|error\['` of it, never the file whole,
  and paste those lines in the report.

## Proof

Before pasting a proof, say what would make it fail. Then:

- Visual proof is of the component mounted in the served app: build → open what is served →
  check the loaded artifact matches the build → capture. Static HTML proves nothing.
- "X shows up when it should not" needs a negative assertion on the same real fixture.
- Real world before mock; mock only after the real one failed, saying why.
- A long-lived service serves the code from when it started: check its start time against the
  commit, or bring up your own instance on another port. The user's service keeps running.
- Image reading and DOM disagree about something visible → the screenshot rules; "there is no
  X in the image" is a result.
- A blocker fix ships with its trap in the same round: the test that fails without the fix
  exists; undo the fix and watch it go red. Same for a finding an automatic reviewer raised.
- Mutation runs in a detached worktree: `git worktree add --detach <tmp>/mut-<x> <object>` →
  apply → run → `git worktree remove --force`. The tree you commit stays intact.

## The proof stage

- Own `HOME`: `HOME=<proof dir> <command> --directory <worktree>/...`.
- The project's installers (`install*.sh`) stay unrun.
- Own port, torn down at the end; the user's services and ports keep running.
- Kill by exact PID. Used `pkill -f` anyway → say so unprompted.

## Waiting on an external condition (any step)

- Cap: 10 attempts or 10 minutes. Blew it → stop and report "waiting on <condition>; tried N
  times over T", last return pasted.
- An identical response 3 times in a row → change the check, or stop and report.
- The stage of your proof (server, test account, proof session) is created by you, as an
  explicit step, before checking. Repeated exit 0 is as stalled as repeated error.

## Report line

```
Removed lines: <output of the removed-lines `grep` of "Run", or "none">
Served: <start time of the long-lived service against the round, or your own instance's port — or "nothing served">
```
