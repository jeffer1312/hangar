# Arbiter — the watchdog and the silence

This page belongs to the **moment someone should be working and you received nothing**: how to arm
the watchdog, what to do when it fires, and how to tell an idle session from a dead one.

You read it when arming the watchdog (once, at launch) and when an alarm arrives. The rest of the
time it isn't yours — the normal cycle is in `arbitro.md`.

## Idleness — the signal that something didn't arrive

You always know **who owes work**: the executor of the released Task, or the reviewer of the round
you know is open. While the current owner is `working`, there is nothing to do — a whole Task
takes time, and nudging someone working is noise.

The signal is the inverse: **the current owner is `idle` and you received nothing.** There are
only three causes, and all three resolve without asking anyone:

1. **The message didn't arrive** (queue, restarted session) → resend once, saying it is a resend.
2. **The reply was produced and not sent** — the session finished the report and died before the
   message, or the send failed → **read its transcript**
   (`~/.claude*/projects/<sanitized-cwd>/<uuid>.jsonl`, the most recent, messages
   `type: "assistant"`; the last one is usually exactly what was missing).
3. **The session vanished** → section below: open another and move on.

**You don't resend before looking at the disk.** Their file may already be there, and the
transcript almost always holds the full text. Reading costs a `cat`; resending costs a paid
session's turn and may arrive duplicated.

**And before blaming the channel, look at the recipient's pane.** A first-run assistant open in
their session refuses all typing, and the backend reports that as "session unavailable" — which
looks like a broken queue and isn't.

**The whole team idle at the same time is the strongest alarm there is**, because in normal
operation someone always has the ball. Reached that state without a Task having closed: something
didn't arrive.

Don't sit watching, and **don't ask "how's it going?"**: both spend your turn, the most expensive
token at the table. Leave a **watchdog in the background** — a shell loop, not a model turn — that
polls the sessions' state and wakes you when the current owner goes idle. Use the script that
ships with the skill:

```bash
systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
  "${CLAUDE_SKILL_DIR}/scripts/vigia.sh" <session> [session...] <arbiter> -m 5 \
  -d ~/.hangar/orq/<date>-<gid>/registro.md
```

The command's manual — flags, why a service and not a background process, how to confirm it is
alive — lives in the **header of `vigia.sh` itself**. Two things that are yours, not the
script's: the last name is always the arbiter, and `-d` points at the journal (the watchdog dings
you if it goes 60 min without a write).

**The watchdog covers whoever has the BALL now, plus you — and nobody else.** The command's
session list is the current turn's state, not the contract's table: a session not yet opened, a
retired session and a session **stopped by your order** stay out, and you **rewrite the command at
every handoff** — when releasing a Task, when sending a commit to review. That includes the
executor: after a `REPROVA` the ball passes from reviewer to executor **without you seeing**, and
that is the design. In a parallel batch "who has the ball" is every writer, because there all of
them do — a single watchdog, with all of them inside:

```bash
systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
  "${CLAUDE_SKILL_DIR}/scripts/vigia.sh" t1 t2 t3 review review2 arbitro -m 10 \
  -d ~/.hangar/orq/<date>-<gid>/registro.md
```

**Nobody with the ball = watchdog disarmed — and the ball with the USER is also nobody with the
ball.** A team with no work (everything approved, waiting for the user's decision) with a live
watchdog only produces false alarms and nudges into paid sessions. Disarm **before** asking the
user, and re-arm when the answer arrives. An arbiter in `awaiting_input` waiting for a human
answer is not a fallen arbiter — it is the legitimate state of someone who already delivered the
decision; the watchdog can't tell the two apart, and the one who can is you, exactly whom it
wakes.

**False alarms have a single family, and it is large:** a session stopped by your order read as a
broken session — the one not yet opened, the one that already delivered, the one waiting for a
verdict. The watchdog can't tell "stopped because done" from "stopped because broken". **A nudge
into a deliberately stopped session is not just noise: it is a paid turn**, and the nudged session
shares a tree with whoever is measuring the gates.

And the watchdog command does **not** go into the rules file with the name list: the form goes. A
session list written in a file ages between writing and reading, the same reason the turn's state
lives in the kick-off.

It polls every 60s and wakes you after N consecutive stalled readings. Three things in it are not
implementation detail — they are what makes it work, and each one cost a real failure:

**1. It watches EVERYONE, including YOU.** Watching only the pair leaves out the failure mode
nobody was watching: the judge falling. An arbiter knocked out by a provider error stops the whole
team with the report stuck in the queue — and from the inside that is invisible, because the next
turn feels like it continues from the previous one.

**2. It wakes via `hangar-send --tmux`, not via `echo`** — the message enters as a **prompt** and
revives a dead turn; an `echo` only becomes a notification with the turn alive. The why of
`--tmux` is in the script's header.

**3. It fires when the CURRENT OWNER stops — not when everyone stops.** An idle arbiter with
someone working is the **normal** state. `vanished` counts as stopped: a dead session doesn't
work either. Two exceptions warn immediately, without waiting for silence: a **stuck** session
(says `working` and has produced no event for 10 min) and a session **out of quota**. With the
arbiter on the list, him talking to the user reads as "working" and masks a dead executor: remove
him from the list while an executor has the ball, and put him back when nobody does.

**The watchdog ASKS; it doesn't order a stop.** It is a shell loop reading two numbers — how long
without an event, and whether the last command repeated — and from that it does **not know**
whether the session is stuck or working well. An alarm written in the imperative makes the
recipient obey without checking, because the message arrives through the same door as real
orders. An imperative false alarm has ordered executors to STOP in the middle of legitimate
work; a question never has.

Every message the watchdog sends to a team session is a **question with the evidence attached**
(the text lives in the script); the one that goes to **you** may be affirmative — you are the
only one who can tell a deliberately stopped session from a broken one. Stop orders still exist —
they just come from you, after looking, and not from a minute counter.

**The proof that it works is the synthetic alarm ARRIVING.** On arming, the watchdog fires a
`[vigia] ARMADA ...` to you by itself, **through the same path as real alarms** — if that prompt
reached your session, the channel is proven; if the unit came up and it didn't arrive within 2
minutes, the channel is broken and "active" is worth nothing. A hand-typed test doesn't count: it
proves a path that isn't the broken one, while the real alarms go into the void.

**Confirming it came up is NOT confirming it lives.** `is-active` right after the `systemd-run`
answers `active` because the unit was just born — a unit can sit `active` for hours without one
log line. The confirmations that count (journal over one full cycle, `show -p ActiveState -p
MainPID`) are in the script's header.

**Re-arm the watchdog every time the ball passes** — when releasing a Task, when sending a commit
to review. An expired, un-rearmed watchdog is silence nobody notices. **And kill the old watchdog
when retiring a session**, or it reads "vanished" as stopped and wakes you for a false alarm. One
live watchdog at a time, pointed at the current pair.

A session's message arrives as a prompt and already wakes you by itself: the watchdog is the
**net** for the case where the message doesn't come, not the normal path.

## Night mode — three preconditions, or you don't put the group to sleep

Letting the team run through the night without the user is legitimate — with three things proven
BEFORE, because overnight there is nobody to discover what you didn't foresee. A provider quota
that blows overnight kills every executor on it in the same minute — and with the watchdog
`inactive`, the one who finds out is the user, hours later.

1. **Watchdog proven** — not `active`: the synthetic alarm it fires on arming arrived as a prompt
   in your session (see the watchdog section).
2. **Quota checked** — each team provider's remaining quota against this work's measured average
   consumption per Task. Doesn't cover the night → don't launch.
3. **Fallback valid** — the provider plan B the contract authorized in writing still exists.

Any of them failing: **stop at the current Task's end and wake the user BEFORE sleeping** — a
question at 11pm costs one answer; its absence cost 3 overnight interventions.

## A dying session is not a case for investigation

A team session gone (missing from `hangar-send --list` and from tmux) without you having ordered
it closed: **open another and move on**. That is what autonomy is — the work cannot stop because a
session fell.

The user closes sessions whenever they want, the machine reboots, the process dies. None of that
is an incident; all of it has the same fix. Chasing the cause costs turns, interrupts the user
with a false alarm and doesn't bring the session back — the usual answer is that the user simply
closed the window.

What to do, in order, without asking anyone:

1. **Read the dead session's transcript** (`~/.claude*/projects/<sanitized-cwd>/<uuid>.jsonl`, the
   most recent, messages `type: "assistant"`). It may have **produced** the report or the review
   and died before sending — in that case the work is not lost and you don't even redo it.
   **And look at the pane before requesting anything again**:
   `tmux capture-pane -p -t "=<name>:" -S -200`. With the output channel dying (it happens on
   flaky providers), the whole report sits **on screen**, complete, having never left — and the
   one who notices that the session "couldn't send" shouldn't have to be the user.
2. **Open the substitute** by the usual recipe (create → prove → request in a file → check
   delivery), with the full kick-off: role, expected HEAD, literal untouchables, contract, plan,
   and the current commit or recipe.
3. **Record in the contract** in one line: which session vanished, what was recovered from the
   transcript and who took over.

It only becomes a case for investigation if the **repo** is also strange — a dirty tree nobody
explains, a commit nobody reported, an untouchable touched. Then the subject is the repo, not the
session.
