#!/usr/bin/env bash
# The pipeline's watchdog. It watches ALL the work's live sessions — every executor, every
# reviewer and the arbiter itself — and wakes the arbiter by message when NOBODY has the ball.
#
# Why all three and not just the pair: an arbiter knocked out by a provider error leaves the
# executor's report stuck in the queue, the reviewer with nothing to review, and the whole team
# stopped for hours. A watchdog that only watches the pair and sends `echo` — which only becomes
# a notification if the arbiter's turn is ALIVE — shouts into the void.
#
# Two fixes, and both matter:
#   1. It watches the ARBITER too. A stalled judge is the failure mode nobody was watching.
#   2. It WAKES via `hangar-send --tmux`, which enters as a prompt and revives a dead turn. The
#      `--tmux` is MANDATORY: plain `hangar-send` REFUSES to talk to a Claude session on the same
#      machine (rc=3, "use SendMessage") — and a shell script has no SendMessage.
#
# The firing condition is conservative on purpose: only when ALL of them are stopped at the same
# time. An idle arbiter with someone working is the NORMAL state (he waits), and waking him there
# is noise that spends the most expensive token at the table. In a parallel batch this matters
# even more: with ONE watchdog per pair, each saw only its own slice and woke the arbiter while
# another executor worked.
#
# Run it as a SERVICE, never as a background process of the turn (`setsid nohup … &` dies with
# the turn that launched it — gone from ps, empty log, no error):
#   systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
#     "${CLAUDE_SKILL_DIR}/scripts/vigia.sh" <session> [session...] <arbiter> -m 5 -d <registro.md>
# `Restart=always` is the other half: without it, a unit that falls leaves the work without a net.
#
# Usage: vigia.sh <session> [session...] <arbiter> [-m <minutes>] [-d <journal.md>]
#      The LAST name is always the arbiter. Minutes go by FLAG (`-m 5`), never as a loose number
#      at the end: with more than three sessions the positional number becomes a session NAME, and
#      the alarms go to a session called "5" while the group stalls. E.g.:
#      vigia.sh t1 t2 t3 review review2 arbitro -m 10 -d ~/.hangar/orq/<date>-<gid>/registro.md
#      The old form `vigia.sh exec rev arb 5` still works.
#
# Confirming it LIVES (is-active right after the systemd-run answers `active` because it was just
# born, not because it reads the API — a watchdog once sat `active` for hours with no log line):
#   journalctl --user -u vigia-<gid> --since "-3min"   # no error repeating every cycle
#   systemctl --user show vigia-<gid> -p ActiveState -p MainPID
# And wait one full cycle (60s). The real proof is the synthetic alarm arriving (below).
#
# Three alarms beyond "everybody stopped", each guarding a real failure mode:
#   - REPETITION: a `working` session whose last command is the SAME for N straight readings is
#     looping, not working — polling produces an event every few seconds and fools the idleness
#     sensor, and thousands of repeats can become most of a run's bill. Repeated success is as
#     stalled as repeated error.
#   - JOURNAL (-d): the arbiter's journal 60min without a write with the group active — the hours
#     that go unrecorded are exactly the most expensive ones.
#   - PROVEN ARMING: on start, the watchdog sends a synthetic alarm to the arbiter THROUGH THE
#     SAME path as the real ones. "Working" is that prompt arriving — not `is-active`, not a
#     hand-typed test (both "proved" twice a channel that was broken).

set -u
# Accepts AS MANY sessions as given: `vigia.sh <s1> <s2> ... <arbiter> [minutes]`. The last name
# is always the ARBITER — the notices go to him, and he is whom the watchdog revives.
#
# Why N and not three: a parallel batch has more than one writer. One watchdog per pair worked,
# but each saw only its own slice, and the firing condition ("nobody has the ball") is only true
# looking at EVERYONE — with separate pairs, one watchdog woke the arbiter while another executor
# worked. The Python reader always accepted N names (`sys.argv[1:]`); what limited it to three
# was this shell.
#
# Minutes go by FLAG (`-m N` or `--minutos N`), in any position. The old form — a loose number at
# the end — is still accepted, but ONLY in the three-name signature the documentation used to
# teach (`vigia.sh exec rev arb 5`). Reason: with N sessions, "last numeric argument" is
# ambiguous — a session named `123` would be eaten as the minutes limit, and the watchdog would
# silently watch one session less. A numeric session name is no hypothesis:
# `sanitize_session_name` accepts them.
LIMITE=5
DIARIO=
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -m|--minutos) LIMITE=${2:?"-m needs the number of minutes"}; shift 2 ;;
    -d|--diario)  DIARIO=${2:?"-d needs the journal's path"}; shift 2 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done
# Strict backwards compatibility: exactly 4 positionals with a numeric last one = the old call.
n=${#ARGS[@]}
if [ "$n" -eq 4 ] && printf '%s' "${ARGS[3]}" | grep -qE '^[0-9]+$'; then
  LIMITE=${ARGS[3]}
  ARGS=("${ARGS[0]}" "${ARGS[1]}" "${ARGS[2]}")
fi
SESSOES=("${ARGS[@]}")
[ "${#SESSOES[@]}" -ge 2 ] || { echo "usage: vigia.sh <session> [session...] <arbiter> [minutes]" >&2; exit 2; }
ARB=${SESSOES[$((${#SESSOES[@]}-1))]}      # the last one is the arbiter
export ARB

BASE=${CP_BASE:-http://127.0.0.1:8765}
ENVFILE=${CP_ENV:-$(dirname "$(realpath "$(command -v hangar-send)")")/../backend/.env}
T=$(grep '^CP_AUTH_TOKEN=' "$ENVFILE" | cut -d= -f2-)
# An empty token cannot turn into "can't read the API" five minutes later: the watchdog would
# stand, log clean, watching nothing — the same failure mode the file-based reader came to fix.
[ -n "$T" ] || { echo "[vigia] CP_AUTH_TOKEN missing in $ENVFILE — cannot read /api/sessions" >&2; exit 1; }

# The token goes through a curl CONFIG FILE, never `-H` on the command line: a process argument
# is readable by any user of the machine via `ps aux` / /proc/<pid>/cmdline, and this call runs
# once a minute through the whole night. mktemp creates it 0600.
CURLRC=$(mktemp /tmp/vigia-curlrc-XXXXXX)
printf 'header = "Authorization: Bearer %s"\n' "$T" > "$CURLRC"

# Where the reader's errors go. The default CANNOT be /dev/stderr: running without a terminal
# (systemd, cron, redirected nohup) it doesn't open for writing, the `2>>` redirection FAILS, and
# bash doesn't run the command — `st` comes back empty and the watchdog concludes "API not
# answering", sitting `active` while shouting "I am watching nothing" with the backend answering
# 200 in milliseconds. Test once and fall back to a file.
if [ -z "${CP_VIGIA_LOG:-}" ]; then
  if : 2>>/dev/stderr; then CP_VIGIA_LOG=/dev/stderr
  else CP_VIGIA_LOG=${TMPDIR:-/tmp}/vigia-$$.err; fi
fi
export CP_VIGIA_LOG

parados=0
avisos=0
PSEQ=()          # straight stalled readings, per session (the per-session notice uses this)
NUDGE=()         # already nudged this session in this stall? (1 push per stall, not per notice)
RHASH=()         # hash of the last seen command, per session (loop detector)
RSEQ=()          # straight readings with the SAME command, per session
RAVISO=()        # already warned about this loop? (1 warning per streak)
mudos=0
avisou_cota=
avisou_travado=
diario_avisado=0

# PROVEN ARMING: the synthetic alarm goes out through the SAME path as the real ones. If it does
# not deliver, the watchdog does NOT stand pretending to be a net — it exits loudly, which is the
# opposite of shouting into the void.
hangar-send --tmux "$ARB" "[vigia] ARMED over: ${SESSOES[*]} (window ${LIMITE}min${DIARIO:+, journal $DIARIO}). This message IS the channel's proof — if you read it, the alarms arrive. Do not reply."
rc_arm=$?
if [ "$rc_arm" -ne 0 ]; then
  echo "[vigia] FAILED to prove the channel with '$ARB' (hangar-send --tmux rc=$rc_arm). I am NOT armed." >&2
  exit 1
fi

# The state reader lives in a file, not in a `python3 -c '...'` line inside the loop. The
# reason: with `-c` between the shell's SINGLE quotes, a `\"` there
# reaches Python as backslash-plus-quote inside an f-string — `SyntaxError`. Since the call ended
# in `2>/dev/null`, the error vanished, `st` came back empty and the `continue` below skipped the
# reading. The watchdog ran the whole time, process alive and log clean, having NEVER looked at a
# session. Names go by argument, not environment variable, to avoid quoting inside the embedded
# script.
LEITOR=$(mktemp /tmp/vigia-leitor-XXXXXX.py)
trap 'rm -f "$LEITOR" "$CURLRC"' EXIT
cat > "$LEITOR" <<'PY'
import json, sys, time

# `working` is NOT proof that someone has the ball: an executor that fires an AskUserQuestion
# BLOCKS its own turn and sits stalled waiting for an answer nobody will give — with the app
# reporting `working` the whole time, because the Pi hook (`scripts/pi/hangar-state.ts`) only publishes `working` or
# `idle`: there is no `awaiting_input` for a Pi session. The watchdog stayed silent and was right
# by the rule it had.
#
# The signal that tells the two apart is `last_activity`, which advances on every transcript
# event. A truly working session moves that number; one blocked in a picker freezes. `working`
# frozen for more than PARADO_S seconds counts as STOPPED.
PARADO_S = 600

agora = time.time()
dados = json.load(sys.stdin)
mapa = {s.get("name"): s for s in dados}
saida = []
for nome in sys.argv[1:]:
    s = mapa.get(nome)
    if s is None:
        saida.append("gone")
    elif s.get("limited"):
        # A blown quota is a stall that doesn't undo itself: the model won't answer again until
        # someone switches the account. It outranks the pane's state, which in that case shows
        # the last frame and can be read as work in progress.
        saida.append("noquota")
    else:
        estado = s.get("state") or "?"
        ts = s.get("last_activity")
        if estado == "working" and isinstance(ts, (int, float)) and agora - ts > PARADO_S:
            saida.append("stuck")
        else:
            saida.append(estado)
print("|".join(saida))
PY

# LOOP detector: extracts from /history the hash of the last tool command. A `working` session
# returning the SAME hash for REP_LIMITE readings is not working — it is pressing the same key.
# The event id changes on every call; what is compared is the CONTENT (tool_input).
LOOPDET=$(mktemp /tmp/vigia-loopdet-XXXXXX.py)
trap 'rm -f "$LEITOR" "$CURLRC" "$LOOPDET"' EXIT
cat > "$LOOPDET" <<'PY'
import hashlib, json, sys
try:
    evs = json.load(sys.stdin)
    tool = [e for e in evs if isinstance(e, dict) and e.get("kind") == "tool_use"]
    if not tool:
        print("")
    else:
        payload = json.dumps(tool[-1].get("tool_input"), sort_keys=True, ensure_ascii=False)
        print(hashlib.md5(payload.encode()).hexdigest())
except Exception:
    print("")
PY
REP_LIMITE=${CP_VIGIA_REP:-10}

# Interval between readings. It exists as a variable only so the smoke test can run the whole
# loop in seconds; in normal use nobody passes it.
INTERVALO=${CP_VIGIA_INTERVALO:-60}

for i in $(seq 1 1440); do
  sleep "$INTERVALO"
  st=$(curl -s --config "$CURLRC" "$BASE/api/sessions" \
       | python3 "$LEITOR" "${SESSOES[@]}" 2>>"${CP_VIGIA_LOG:-/dev/stderr}")
  if [ -z "$st" ]; then
    # The API's silence cannot be the watchdog's silence: that is how the hole above hid.
    mudos=$((mudos+1))
    if [ "$mudos" -eq 5 ]; then
      echo "[vigia] 5 straight readings with no answer from $BASE/api/sessions — I am watching nothing"
      hangar-send --tmux "$ARB" "[vigia] I cannot read /api/sessions for 5 minutes. Meanwhile I am watching NOBODY — check the backend and re-arm me." >/dev/null 2>&1
    fi
    continue
  fi
  mudos=0

  # One state per session, in the SAME order as SESSOES. `resumo` is what goes in the messages.
  IFS='|' read -r -a ESTADOS <<< "$st"
  resumo=""
  for k in "${!SESSOES[@]}"; do
    resumo="$resumo${resumo:+ · }${SESSOES[$k]}=${ESTADOS[$k]:-?}"
  done
  # Only the PAIR (everyone but the arbiter) counts for stuck/noquota: an idle arbiter is normal.
  par_estados="${st%|*}"

  # "Stopped" is everything that is not work in progress:
  #   idle          — finished the turn and is waiting
  #   awaiting_input— blocked on an input request; stopped waiting for people, the most common
  #                   stuck-session case, which the previous version treated as "busy"
  #   gone          — died
  #   noquota       — account limit blown; doesn't come back on its own
  #   stuck         — says `working` but has produced no event for 10min (a picker blocking the turn)
  quieto=1
  for e in "${ESTADOS[@]}"; do
    case "$e" in idle|awaiting_input|gone|noquota|stuck) ;; *) quieto=0 ;; esac
  done

  # PER SESSION: any one of the pair stopped for LIMITE straight readings warns ON ITS OWN,
  # without waiting for the whole team to stop. The collective firing below ("nobody has the
  # ball") exists for a deadlocked pipeline, and it NEVER closes while the arbiter works — which
  # is how a whole set of executors can die together on a provider timeout with nobody told: the
  # arbiter has the ball, so `quieto` never becomes 1. The real requirement: "if some session
  # stops and stays stopped for a long time, it must warn".
  # Re-warns every LIMITE minutes while it stays stopped (the counter resets on warning).
  ULT=$(( ${#SESSOES[@]} - 1 ))
  for k in "${!SESSOES[@]}"; do
    [ "$k" -eq "$ULT" ] && continue          # the arbiter is last; stopped is his normal
    case "${ESTADOS[$k]:-?}" in
      idle|awaiting_input|gone|noquota|stuck) PSEQ[$k]=$(( ${PSEQ[$k]:-0} + 1 )) ;;
      *) PSEQ[$k]=0; NUDGE[$k]=0 ;;
    esac
    if [ "${PSEQ[$k]:-0}" -ge "$LIMITE" ]; then
      # FIRST nudge the session itself, THEN warn the arbiter. The order matters: the most
      # common case is a turn dead on a provider timeout with the 3
      # retries blown — Pi does not retry on its own and the session stays alive, stopped,
      # until someone types into it. One push solves that with nobody waking. Only warning the
      # arbiter didn't solve it: he may be down too, and then it was the user who came looking.
      # Nudge ONCE per stall (nudge=1) and keep warning every LIMITE min while it lasts.
      if [ "${NUDGE[$k]:-0}" -eq 0 ] && [ "${ESTADOS[$k]:-?}" != "gone" ] && [ "${ESTADOS[$k]:-?}" != "noquota" ]; then
        hangar-send --tmux "${SESSOES[$k]}" "[vigia] You have been stopped for ${LIMITE} min without reporting. If your last turn died (provider timeout, retries blown, connection cut), CONTINUE from where you stopped, without restarting and without redoing what was done. If you already delivered and are waiting for a verdict, ignore this message. If you are blocked waiting for something from the arbiter, say in one line what it is." >/dev/null 2>&1
        NUDGE[$k]=1
        cutucada=" — I NUDGED it just now (1st time); if it doesn't come back, the turn didn't die, it is truly stuck"
      else
        cutucada=" — already nudged in this stall; it did NOT come back on its own"
      fi
      msg="[vigia] ${SESSOES[$k]} is stopped (${ESTADOS[$k]:-?}) for ${LIMITE} min${cutucada}. Team: $resumo. Look at its PANE: a provider timeout with retries blown, a dead turn and a report stuck in the queue do not undo themselves."
      echo "$msg"
      hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
      PSEQ[$k]=0
    fi
  done

  # LOOP: a pair session `working` with the SAME command for REP_LIMITE readings. The idleness
  # sensor never catches this (polling produces an event every few seconds and looks like work);
  # the same command can repeat thousands of times over hours, `working` the whole time, and the
  # one who notices ends up being the user. One tail curl per working session, per cycle — cheap.
  for k in "${!SESSOES[@]}"; do
    [ "$k" -eq "$ULT" ] && continue
    if [ "${ESTADOS[$k]:-?}" = "working" ]; then
      h=$(curl -s --config "$CURLRC" "$BASE/api/sessions/${SESSOES[$k]}/history?limit=3" \
          | python3 "$LOOPDET" 2>>"${CP_VIGIA_LOG:-/dev/stderr}")
      if [ -n "$h" ] && [ "$h" = "${RHASH[$k]:-}" ]; then
        RSEQ[$k]=$(( ${RSEQ[$k]:-0} + 1 ))
      else
        RSEQ[$k]=0; RAVISO[$k]=0
      fi
      RHASH[$k]=$h
      if [ "${RSEQ[$k]:-0}" -ge "$REP_LIMITE" ] && [ "${RAVISO[$k]:-0}" -eq 0 ]; then
        msg="[vigia] ${SESSOES[$k]} MAY be looping: it says working but the last command is the SAME for ${RSEQ[$k]} readings (~${RSEQ[$k]} min). Look at the pane before deciding — wait-polling is not work, but long work also repeats commands. You give the stop order, after looking. Team: $resumo"
        echo "$msg"
        # A question, never an order: the watchdog reads two numbers and does not know whether
        # the session is stuck or working — an imperative false alarm has ordered a STOP in the
        # middle of legitimate work. Stop orders come from the arbiter, after looking.
        hangar-send --tmux "${SESSOES[$k]}" "[vigia] You repeat the SAME command for ~${RSEQ[$k]} min. Is this a wait on an external condition? If so, the cap has blown: report to the arbiter what you wait for and the last return (executor.md rule). If you are working, ignore this notice." >/dev/null 2>&1
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        RAVISO[$k]=1
      fi
    else
      RSEQ[$k]=0; RHASH[$k]=""; RAVISO[$k]=0
    fi
  done

  # Stalled JOURNAL: the arbiter's journal is the retrospective's net; >60min without a write
  # with the group active is the arbiter working without a trail. Re-warns hourly.
  if [ -n "$DIARIO" ] && [ -f "$DIARIO" ]; then
    # eventos.jsonl is the journal's sibling in the same directory, and stalled during work is
    # the SAME failure — so the check looks at the OLDER of the two, and names the stalled one.
    parado=$DIARIO
    mtime=$(stat -c %Y "$DIARIO" 2>/dev/null || echo 0)
    eventos="$(dirname "$DIARIO")/eventos.jsonl"
    if [ -f "$eventos" ]; then
      mtime_ev=$(stat -c %Y "$eventos" 2>/dev/null || echo 0)
      if [ "$mtime_ev" -lt "$mtime" ]; then parado=$eventos; mtime=$mtime_ev; fi
    fi
    idade=$(( $(date +%s) - mtime ))
    if [ "$idade" -ge 3600 ] && [ "$diario_avisado" -lt "$(( idade / 3600 ))" ]; then
      diario_avisado=$(( idade / 3600 ))
      hangar-send --tmux "$ARB" "[vigia] The trail ($parado) has gone $(( idade / 60 ))min without a write, with the group active. The journal and eventos.jsonl are written AT the event — if reports/merges happened in this window, they are outside the trail." >/dev/null 2>&1
      echo "[vigia] trail stalled for $(( idade / 60 ))min ($parado)"
    fi
    [ "$idade" -lt 3600 ] && diario_avisado=0
  fi

  # A STUCK session in the pair warns immediately, without waiting for all three to stop: the
  # arbiter holds the ball precisely because he thinks the other one is working. It was the
  # real case — 1h17 of stalled queue with everyone thinking the Task moved.
  case "$par_estados" in
    *stuck*)
      if [ "$avisou_travado" != "$par_estados" ]; then
        msg="[vigia] STUCK session: $resumo. It says 'working' but has produced no event for over 10 minutes — the classic case is a picker/AskUserQuestion blocking the firing turn. Look at the pane and unblock it (POST /api/sessions/<name>/select with {\"option\": N})."
        echo "$msg"
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        avisou_travado="$par_estados"
      fi
      ;;
  esac

  # A blown quota in the pair waits neither for all three to stop nor for the minutes limit: the
  # arbiter must switch the session's account, and every waiting minute is a stalled queue
  # minute.
  case "$par_estados" in
    *noquota*)
      if [ "$avisou_cota" != "$par_estados" ]; then
        msg="[vigia] Account out of quota: $resumo. The session will not come back on its own — open the substitute on an account ALLOWED by the contract and send the same kick-off, with the open Task."
        echo "$msg"
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        avisou_cota="$par_estados"
      fi
      ;;
  esac

  if [ "$quieto" -eq 1 ]; then parados=$((parados+1)); else parados=0; fi

  if [ "$parados" -ge "$LIMITE" ]; then
    msg="[vigia] Nobody has had the ball for ${LIMITE} min: $resumo (minute $i). If you fell (an API error), this is what brings you back. Check whether someone delivered while you were out — a report stuck in the queue and a stalled verdict are the two ways the pipeline locks up with nobody noticing."
    echo "$msg"
    hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
    avisos=$((avisos+1))
    parados=0
    if [ "$avisos" -ge 20 ]; then
      echo "20 warnings without unblocking; shutting the watchdog down"
      exit 0
    fi
  fi
done
echo "1440min over; last state: $resumo"
