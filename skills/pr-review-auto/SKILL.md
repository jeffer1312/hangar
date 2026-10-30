---
name: pr-review-auto
description: Automatic CodeRabbit follow-up for a GitHub PR of the current repo (made for Hangar) — watches the PR by events (no model polling), fixes or answers every CodeRabbit thread and nitpick, fixes CI failures caused by the PR, and merges when the PR is stable and green, if the GitHub user may merge. Use when the user or a sibling session asks to look at CodeRabbit on a PR, to follow a PR until it is ready, or to merge it once ready ("/pr-review-auto 154", "olha o CodeRabbit do PR", "vê se o PR tá pronto e mescla", "acompanha o PR e faz o merge"). Not for GitLab MRs, not a local code review.
---

# PR review auto (Hangar, GitHub)

Invoking this skill authorizes, for the named PR only: fixing code on the PR branch, committing,
pushing to that branch, replying to and resolving CodeRabbit threads, and **merging when ready**
when §5 allows it. It does not authorize: force push, rebase, `--admin` outside §5, closing the
PR, pushing to `main`, deleting the branch. CodeRabbit already reviews on GitHub: never run the `coderabbit` CLI.

`SKILL_DIR` is this skill's base directory. `<repo>` is `gh repo view --json nameWithOwner -q
.nameWithOwner` run in the PR's tree; `<main tree>` is the first line of `git worktree list`.
All user-facing text in the user's language.

## 1. Pick the PR and the tree

1. PR number from the request; none → `gh pr view --json number,headRefName` on the current branch.
2. `gh pr view <N> --json headRefName,author,isCrossRepository,maintainerCanModify,state`.
3. Find the tree holding the branch: `git worktree list`.
   - It is this session's cwd → work here.
   - Another live session's cwd (`hangar-send --list`) → hand over: `hangar-send <session>
     "Rode a skill pr-review-auto no PR #<N>."`, tell the user who got it, and stop.
   - No checkout → ask the user where to check it out (never create a worktree or switch the
     shared `main` tree on your own).
   - Cross-repo PR without `maintainerCanModify` → you cannot push: handle threads by reply only
     and report what the author must change.
4. `git status` in that tree: someone else's dirty files stay out of every commit.

## 2. Watch

Start the **Monitor** tool with `timeout_ms: 1800000`:

```bash
python3 -u "SKILL_DIR/scripts/pr_watch.py" <N>
```

It polls GitHub every 60 s and prints only lines worth acting on. State is saved in
`~/.cache/hangar-pr-watch/`, so re-arming does not repeat events; `--reset` replays the current
snapshot. `--once` prints the snapshot (with `ready`) and exits — use it for checks, not loops.

On `{"control":"window_ended"}` or the Monitor expiry notice, re-arm the same command silently.
On `{"control":"expired"}` (24 h) tell the user and stop. Keep exactly one Monitor per PR.

After arming, **end the turn**. No sleeps, no CronCreate, no periodic status messages.

## 3. Events

| Event | Action |
|---|---|
| `change` | Handle everything below that is new, then end the turn. |
| `ready` | Go to §5. |
| `closed` | PR merged/closed by someone: report it, stop. |
| `query_failed` | Warn once; the script retries by itself. |
| `query_recovered` | Re-read with `--once` and carry on. |
| `crashed` / `state_reset` | `crashed`: the watch died — tell the user the `error` and stop. `state_reset`: the snapshot that follows is a replay; skip what the log says is done. |

A `change` carries the full snapshot. Handle, in this order:

- **`threads`** whose `last_by` is not you: read the whole thread
  (`gh api repos/<repo>/pulls/<N>/comments` or the `url`), check it against the
  current code. Fix every valid point, small ones included. Reject only what is factually wrong
  or out of scope, with **one** reply in the thread, `@coderabbitai` + evidence, then resolve it
  (an unresolved thread blocks `ready` forever).
- **`replies_in_resolved_threads`**: CodeRabbit answered in a thread already resolved. Agreement
  needs nothing; a new objection is treated like an open thread.
- **`reviews_on_head` with `nitpicks: true`**: read the review body
  (`gh api repos/<repo>/pulls/<N>/reviews/<id>`) and treat each nitpick the same way.
  Skipped nitpicks get one top-level `@coderabbitai` comment saying which and why.
- **`bot_comments`** that answer you: read; a new technical objection needs analysis, agreement
  needs nothing.
- **`coderabbit: "NEUTRAL"`** (it concluded without reviewing this head, usually its rate limit):
  run the `code-review` skill once on the PR (`high <N>`; a harness without it runs the stack
  reviewer on `gh pr diff <N>`). Fix every valid finding (§4), log the rest, then
  `python3 SKILL_DIR/scripts/pr_watch.py <N> --mark-local-review <head of the last push>`. A head
  that only adds those fixes is marked without another review.
- **`failed`** checks: `gh run view <run> --log-failed`. Caused by the PR → fix. Known flaky or
  infra (runner, network) → `gh run rerun <run> --failed` once; fails again → report and stop.
  Unrelated to the PR and not flaky → report and stop; do not merge.
- **`mergeable: CONFLICTING`**: `git merge origin/main` into the branch (never rebase), resolve,
  commit.
- Pending CI only, or a new `head` you just pushed: nothing to do.

Keep a log at `~/.cache/hangar-pr-watch/<owner>_<name>_<N>.log.md`: thread/review id,
decision, fix commit, reply sent. Read it after a context compaction; never reply twice to the
same point.

## 4. Publish a round

1. Stage by explicit path, only this round's files. Descriptive English message
   (`fix(scope): …`), never the branch name.
2. Push to the PR branch. The review hook blocks the first push: run the stack reviewer once on
   the round's diff, apply what holds, push again — fixes from the reviewer are not reviewed again.
   Push with `HANGAR_SEM_VERIFICACAO=1 git push` (the maintainer's choice for this skill): do
   not run `scripts/verificar-local`; the PR's CI is the gate, and §5 only merges on green CI.
3. For each fixed thread reply once:
   `gh api repos/<repo>/pulls/<N>/comments/<root_comment_id>/replies -f body="@coderabbitai Corrigido em <sha>. <what changed>."`
   then resolve it:
   `gh api graphql -f query='mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{isResolved}}}' -F t=<thread_id>`.
   A later bot reply in that thread arrives in `replies_in_resolved_threads`.

## 5. Ready → merge

`ready` means: PR open, not draft, mergeable, CodeRabbit status `SUCCESS` on the head (or `NEUTRAL`
with the local review marked for it), no unresolved thread, every check terminal and none failed, unchanged for 90 s. The script does not
know about nitpicks: confirm every nitpick in the log was handled. Your own notes never wake
you, including ones the user wrote by hand under the same GitHub account.

1. `python3 SKILL_DIR/scripts/pr_watch.py <N> --once` → `ready: true` and the same `head`.
   Otherwise keep watching.
2. `main` requires one approval, and nobody else approves: only an admin merges, bypassing it.
   `viewer_permission` not `ADMIN` (`WRITE`, `MAINTAIN`, …) → do not merge: stop the Monitor,
   tell the user the PR is ready and waits for an admin's approval or merge, and stop.
3. `ADMIN` → `gh pr merge <N> --merge --admin --match-head-commit <head>` (merge commit, the
   repo's convention; keep the branch). The head check refuses the merge if someone pushed
   after step 1.
4. Stop the Monitor with **TaskStop**.
5. Align the shared checkout: in `<main tree>`, on `main` with a clean tree,
   `git fetch origin && git merge --ff-only origin/main`. Not fast-forward or dirty → just say so.
6. Report: PR, merge commit, what was fixed vs. answered, the main CI run
   (`gh run list --branch main -L 1`).

User says stop → TaskStop, no resume.
