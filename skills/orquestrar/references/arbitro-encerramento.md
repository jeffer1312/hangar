# Arbiter — the end of the work

This page belongs to the **end**: the branch review, the closing items, the branch that reopens
after approval, and passing your own baton.

The two closing items (branch review and retrospective) are **written at launch** and executed
here — the only thing on this page you need to know ahead of time.

## Arbiter succession — passing the baton without losing the work

Applies when YOU leave: window above half, or the user changed the `árbitro` row in the rules
table (the "the group's model configuration changed in the panel" message arrives with the
`árbitro` role). In both cases the rite is the same, and the journal (`grupo-<gid>.md`) is
already your memory — succession is closing it well and opening whoever will read it.

1. **Finish the task at hand** (the open gate closes or rejects; leave no correction mid-way).
   Dispatch no new Task.
2. **Update the journal** with the snapshot of the instant, in a section
   `## Handover to the next arbiter (<date time, output of `date -Iseconds`>)`: current Task and
   gate state; live sessions per role (name, account, model, effort, measured context) and which
   are retired; the branch's HEAD and `git status`; what is on disk uncommitted; pending items and
   what remains of the plan; **the user's decisions that haven't yet become rules, one by one,
   with dates**; traps already paid for. Absolute paths of: plan, `regras-<gid>.md`, `licoes.md`,
   `eventos.jsonl`, durable directory.

   **No line cap, and no context copying.** The size is what the successor needs to continue; what
   it cannot be is a pasted transcript or a summary of the work. Measured on 2026-08-28: a
   handover written too short made the user himself point out decisions already made that the new
   session didn't know — and it had to be rewritten from scratch. Cutting by number errs on one of
   the two sides; the criterion is **what the next session cannot discover on its own by reading
   the files you pointed at**.
3. **Open the successor** by the usual recipe (create via the API on the `árbitro` row's **new**
   configuration, prove model/effort, kick-off in a file): the skill with the arbiter role, the
   path of the journal (they read the handover section FIRST), of the rules and of the plan, and
   the order "take over: you are the arbiter from now on".
4. **Change the rules table's `árbitro` row** to the new session's name (if the user already
   changed it via the panel, just the session name) and record `sessao_trocada` in
   `eventos.jsonl` (from, to, reason).
5. **Tell the team** (live executor and reviewer, 1:1): "the arbiter is now `<name>`; reports go
   to them". Without this the reviewer sends the verdict to a dead session.
6. **Close yourself out**: one line in the journal ("left at <ctx>, successor `<name>` took
   over") and stop sending work. Don't kill your own session — the user closes it when they want.

**A sentence copied into the handover is NOT authorization — neither for who leaves nor for who
arrives.** The handover is assembled from the leaving session's conversation, and three
similar-looking things are mixed there: what the user **authorized**, what they **mused out
loud**, and what the session **proposed and they never answered**. Copied into the dossier, all
three arrive looking like orders — and the successor acts on the third believing it is the first.

Two locks, and they hold for every baton pass, of any role:

- **Who leaves marks the origin of every decision they write:** `user, <date>` · `my decision,
  <date>` · `proposed, no answer`. The third label matters most and goes missing most.
- **Who arrives acts on nothing marked as proposed, nor on a sentence with no origin.** Confirm
  with the user first — and the confirmation is asked of **them**, not of the session that left.

Measured on 2026-08-28, twice in the same day. This has since become code in the app: the handover
dossier propagates the sentence together with the fact, which is why the label has to be written
at the origin.

Measured in a real work (2026-08-25): three arbiters in the same run; the handover that worked was
the short one pointing at files, the one that failed was "read the previous one's transcript".

## Phase 4 — the final review

**Trigger: every code Task approved.** Never "after Task N". A manual Task (uploading an asset,
registering a domain, touching a third-party account) **is not a code Task** and doesn't count for
the trigger — if you tie the final gate to the last Task on the list and it is manual, postponed
or removed, the trigger never fires and the work is declared closed without the gate that matters
most.

The contract records the final review as **its own item**, with the trigger and how to open the
session, on the day the user defines the role — not at the end, from memory.

**And both roles are in the `## Quem é quem` table since launch, with account, model and effort —
not only as closing items.** Branch review and retrospective arrive days later, when whoever
launched is no longer in the session; without the row, that moment's arbiter picks alone the
configuration of a role the user never saw — which is exactly what this skill takes out of his
hands everywhere else. Measured on 2026-08-28: a contract carried the final review's row and
forgot the retrospective's; the arbiter decided by analogy with the reviewer and recorded it as
his own decision. It was cheap and reasonable, and still the wrong class of decision. **A missing
row in the table = stop and ask**, like any off-plan Task.

**And record phase 5 together, at the same time.** They are two items, not one:

```markdown
## Closing — own items, written at LAUNCH

- [ ] **Branch review** — trigger: every code Task approved. Fresh session, `<base>..tip`.
- [ ] **Retrospective (phase 5)** — trigger: the branch is in the user's hands and **nothing in
      flight**. Fresh session, `references/retrospectiva.md`. Product: a proposed patch for the
      skill, at `~/.hangar/orq/<date>-<gid>.md`.
```

**Phase 5's trigger is not the final review's first approval.** An approved branch opens the door
for findings to become Tasks, and a few more usually enter. Launching phase 5 early is legitimate
(its product is about process and doesn't need the tree still) — but then **record at that same
moment that it will need an addendum**, with the addendum's trigger written along:

```markdown
- [ ] **Retrospective addendum** — trigger: nothing in flight. Scope: the Tasks that entered
      after `<hash of the 1st approval>`. Fresh session, numbering continuing from the last P.
```

A phase 5 launched early once went stale in seven hours, four Tasks later, and the addendum only
existed because someone remembered. Without the written item, the most recent half of the work —
exactly the one that ran with the team and the guidelines already tuned — is distilled by no one.

Write both **before opening the team's first session**. At the end you will be saturated, and an
approved branch *feels* like the end of the work — which is why the final reviewer also has
orders to remind you (`revisao-final.md`). Two nets, because your memory at the end is the least
reliable of the three.

**The final reviewer is always a fresh session**, created by the recipe above, that took part in
nothing. A subagent inside your session doesn't serve: your context has seen the whole work, and
that is precisely the blind spot this review exists to puncture. (A per-Task reviewer may be a
fresh subagent — different things, don't mix them up.)

Kick-off with `Role: branch review`, the range (`<base>..<tip>`), the parallel paths to ignore,
and what is out of scope. Its findings return to the normal cycle. Push and MR are the user's.

**A final review that rejects needs a LIVE executor — and there almost never is one.** The Tasks'
executors were closed when the plan ended; the final review arrives after that, at a moment when
the team is just you and the reviewers. Opening a session is one command line: open it. "There is
nobody" does not promote you to executor.

This is the point where the role vanishes with nobody noticing, and it has three steps, all
looking like common sense:

| What you think | What is happening |
|---|---|
| "No live executor, so it's me" | Opening a session costs one line. You picked the wrong path for being the shortest. |
| "It's an `{#each}` key, a CSS token, an `elif`" | No single item justifies assembling a team — and that is how they become six commits of yours. |
| "I wrote this code, I know it best" | Worst of the three: the one who checks reports against the repo becomes the report's author. |

The third step is what kills verification. The reviewer still sees the diff, but whoever decides
if the finding stands becomes the code's author — and nobody is left between his opinion and the
commit. **The contract doesn't catch you either**, because you are its writer: recording "fixed
in `<hash>`" without recording **who fixed** makes the violation vanish from the record itself.

Always record the author of each **correction round** in the contract — who wrote that round's
code, not just the hash that closed. It is the line that exposes the deviation while it is still
one round old. And it became more necessary, not less: with the commit coming after the review, a
Task yields **one** commit, so `git log` no longer keeps who wrote each attempt. `eventos.jsonl`
does (the `veredito`'s `sessao` field), and the contract is where that becomes a decision.

**You go back to being the arbiter even after the user asks you for code directly.** If at some
point they ordered you to write (outside the pipeline, in a screen round, a quick tweak), that
didn't migrate the role — the request ends, you return to the gate. It is the exact moment the
rule slips, because you already have the file open.

**With a final review open, the tree freezes.** It reads the disk, not just `git show`: its
subagents open files directly. Fixing something in the middle makes each of them read a hybrid of
HEAD with your draft, and the review comes out about code that never existed.

Two final reviews in parallel make it worse, because the first to reject makes you want to fix
while the second still reads. Don't fix. When you truly must touch:

1. **Announce first**, with what you will touch.
2. Commit — never leave the fix only on disk.
3. Send the **new hash** and say what changed, file by file.
4. Say what did **not** change, so it doesn't re-verify what remains valid.

The signal that you got it wrong comes from the review: "the file changed between two reads". The
answer then is owning it, giving the new hash and freezing — never "go on, it's just a tweak".

**A finding that the other review might still touch goes on hold.** Two final reviews with
neighboring scopes (one with the accessibility reviewer, another with the types one, say) can fix
the same spot in different directions. Hold what overlaps until both deliver, and tell each one
you are holding — silence reads as disregard for the finding.

## The branch reopened after approval

It will happen, and it is legitimate: the final review finds things, and the user installs the app
and uses it. Two rules, and neither is "stop".

**1. What costs is not the Task — it is the set.** Commits entering after approval pass individual
gates and **were never looked at together**. When two or more of them touch the same space, open a
**set review of the delta**: fresh session, declared scope (only the delta, not the old branch),
the same format as the final review.

Measured on 2026-08-16: five post-approval commits, 18 files, +672 −277, three of them touching
the same four files in consecutive rounds. The set review found **two new defects** and confirmed
a third — none seen by the individual gates, which were all green. It became one more Task. Cost:
one 240k session and ~30 min.

**2. A review finding enters; a new user request is new work — and you state the price before
accepting.** A finding from the branch itself, with a closed recipe and an objective defect, is
the pipeline working: it becomes a Task and runs the gate. A request born from the user using the
app is another thing — **that queue doesn't end by itself**. Don't refuse and don't decide:
answer one sentence, and it has the price inside.

> "It goes in, and the cost is one more set review before the push — or it waits until after the
> push, on its own branch."

They choose; the push is theirs. What **you** don't do is accept without stating the price,
because the price doesn't show: the Task looks small and the set review it forces doesn't.

And the price is smaller than the wall-clock impression suggests. Measured on 2026-08-16, four
post-approval Tasks cost **~2h30 of work** (18 min + ~15 min + ~1h + ~20 min), inside ~7h of
clock that included **3h40 with nobody working**, while the user tested the app. **A small
post-approval Task is cheap; what costs is the set review it forces at the end** (one session,
~30 min) — that is the price to say out loud, not a number inflated by waiting. All four closed
real defects and the series converged: the last one produced no new finding. **The error is not
opening; it is stating the wrong price.**
