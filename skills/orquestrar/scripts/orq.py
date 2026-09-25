#!/usr/bin/env python3
"""orq — the orchestration's conductor: transport and bookkeeping without waking the arbiter.

Events, the journal, the commit check, the shared-screen lock, who has the ball and the triage
of messages to the arbiter live here, so the arbiter's session wakes only for decisions.

Directory: `--dir D` (before the command) or ORQ_DIR — the durable dir ~/.hangar/orq/<date>-<gid>/.
Config: <dir>/orq.json, written once by `orq init` at launch. Stdlib only.
"""
from __future__ import annotations

import argparse
import fnmatch
import http.client
import importlib.util
import io
import json
import os
import re
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.request
from contextlib import redirect_stdout
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
JOURNAL_CAP = 40_000
COMMON_CAP = 8_000
# A session that died holding the screen cannot freeze the team; a legit proof re-takes it.
SCREEN_STALE_S = 60 * 60
TASK_HEAD = re.compile(r"^## Task (\d+)\b")
EVENT_FIELDS_INT = ("task", "rodada")
EVENT_FIELDS_STR = ("commit", "resultado", "sessao", "motivo", "titulo", "executor", "par",
                    "de", "para", "plano", "branch", "gid", "fase")
JEV_URL = "https://api.typesafe.ai/v1/systemone"
JEV_MODEL = "jev-1.13.0"
JEV_TIMEOUT_S = 5
# A hung backend cannot hang the session that called orq.
SEND_TIMEOUT_S = 30
# Calibrated on real arbiter messages: changing a word or a threshold means measuring again.
DISCARD_P = 0.85  # p of "nothing" (the choice's winner) needed to drop
VETO_P = 0.40     # any alert above this keeps the arbiter awake
JEV_VETOES = ("context", "user", "problem", "deviation")
JEV_QUESTIONS = {
    "kind": {
        "type": "choice",
        "instructions": ("A session of a software team sent this message to the team's coordinator. "
                         "Routine bookkeeping is automatic; the coordinator is needed only to decide "
                         "or act. What does the coordinator have to do with this message?"),
        "criteria": {
            "act": ("decide or act: answer a question; grant a permission or a go-ahead (screen time, "
                    "more actions, 'may I', 'waiting for your OK'); choose between options; handle a "
                    "failure, blocker or environment problem; open, name or replace a session (the "
                    "sender is at its context limit or hands over, a session is gone, a reviewer must "
                    "be named); settle a disagreement or a change of plan; record a decision from the "
                    "user; or release the next Task after a commit"),
            "nothing": ("nothing: the message only informs - progress, a status note, an "
                        "acknowledgement, a wake-up or environment confirmation, a proof window "
                        "opened or closed, a round delivered to the reviewer, a verdict already sent "
                        "to the executor - and asks nothing of the coordinator"),
            "none": "none of these",
        },
    },
    "context": {"type": "noul", "instructions": (
        "Does the message say the sender's context is at or above about 45% of its "
        "window, or that the sender retires, stops for good or will be replaced "
        "('sucessora', 'passagem', 'aposento', 'última entrega', 'sessão nova')?")},
    "user": {"type": "noul", "instructions": (
        "Does the message relay words or a decision of the user ('palavra do usuário', "
        "'resposta do usuário', 'decisão de produto')?")},
    "problem": {"type": "noul", "instructions": (
        "Does the message report something broken that blocks the work: a crash, a "
        "failing command or tool (for example HTTP 401), a full disk, a dirty git tree "
        "nobody explained, a session that no longer exists, or a repeated defect "
        "('reincide')?")},
    "deviation": {"type": "noul", "instructions": (
        "Does the message say a required step was skipped, deferred or done "
        "differently from the instructions or the recipe?")},
}


class OrqError(Exception):
    pass


def now() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def base_dir(arg: str | None) -> Path:
    raw = arg or os.environ.get("ORQ_DIR")
    if not raw:
        raise OrqError("no directory: pass --dir or set ORQ_DIR")
    d = Path(raw).expanduser()
    if not d.is_dir():
        raise OrqError(f"directory does not exist: {d}")
    return d


def config(d: Path) -> dict:
    try:
        return json.loads((d / "orq.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise OrqError(f"{d}/orq.json missing: the arbiter runs `orq init` at launch") from None


def journal_append(d: Path, text: str) -> None:
    """One line per entry, so `read journal` can filter without parsing paragraphs."""
    j = d / "registro.md"
    line = f"- {now()} · " + " ⏎ ".join(text.strip().splitlines()) + "\n"
    if j.exists() and j.stat().st_size + len(line.encode()) > JOURNAL_CAP:
        n = 1
        while (d / f"registro-arquivo-{n}.md").exists():
            n += 1
        j.rename(d / f"registro-arquivo-{n}.md")
        j.write_text(f"- {now()} · journal continues; older entries in registro-arquivo-{n}.md\n",
                     encoding="utf-8")
    with j.open("a", encoding="utf-8") as f:
        f.write(line)


def _validator():
    spec = importlib.util.spec_from_file_location("orq_valida_eventos", HERE / "orq-valida-eventos.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def events(d: Path) -> list[dict]:
    p = d / "eventos.jsonl"
    if not p.exists():
        return []
    out = []
    for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if isinstance(ev, dict):
            out.append(ev)
    return out


def event_append(d: Path, ev: dict) -> dict:
    ev = {"ts": now(), **ev}
    buf = io.StringIO()
    with redirect_stdout(buf):
        errors = _validator()._valida_linhas("event", [json.dumps(ev, ensure_ascii=False)])
    if errors:
        raise OrqError(buf.getvalue().strip())
    with (d / "eventos.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps(ev, ensure_ascii=False) + "\n")
    return ev


def _closed(d: Path) -> dict:
    """Task → ts of its latest close (None: an old line without ts)."""
    p = d / "closed.jsonl"
    if not p.exists():
        return {}
    out = {}
    for line in p.read_text(encoding="utf-8").splitlines():
        try:
            c = json.loads(line)
        except ValueError:
            continue
        if isinstance(c, dict):
            out[c.get("task")] = c.get("ts")
    return out


def _closed_after(closed_ts, ev_ts) -> bool:
    """A Task reopened under the same number after its close owns the ball again. A line without
    ts, or a ts that does not parse, closes everything before it."""
    if closed_ts is None:
        return True
    try:
        return datetime.fromisoformat(closed_ts) >= datetime.fromisoformat(ev_ts)
    except (TypeError, ValueError):
        return True


def state(d: Path) -> dict:
    """One pass over the events: who executes and reviews each Task (after swaps), who has the
    ball, who the arbiter is now, which Tasks are open, whether the run has ended, and which swaps
    replaced an executor or reviewer. The ball table is arbitro-vigia.md's."""
    arbiter = config(d)["arbiter"]
    roles: dict[int, dict] = {}
    last: dict[int, dict] = {}
    ended = False
    replaced: list[tuple[str, str]] = []
    for ev in events(d):
        t = ev.get("tipo")
        if t == "task_inicio":
            roles[ev.get("task")] = {"executor": ev.get("executor"), "par": ev.get("par")}
            last[ev.get("task")] = ev
        elif t in ("entrega", "veredito"):
            last[ev.get("task")] = ev
        elif t == "sessao_trocada":
            was_arbiter = ev.get("de") == arbiter
            if was_arbiter:
                arbiter = ev.get("para")
            held = False
            for r in roles.values():
                for k in ("executor", "par"):
                    if r.get(k) == ev.get("de"):
                        r[k] = ev.get("para")
                        held = True
            # Only who held a Task role is closable, and never an arbiter swapped out: it may be
            # the user's own coordinator session.
            if held and not was_arbiter:
                replaced.append((ev.get("de"), ev.get("para")))
        elif t == "execucao_fim":
            # Work may resume in the same file without a new execucao_inicio; only Tasks
            # touched after the end can own the ball.
            last.clear()
            ended = True
        if t != "execucao_fim" and isinstance(ev.get("task"), int):
            ended = False
    ball: list[str] = []
    open_tasks: list[int] = []
    closed = _closed(d)
    for task, ev in last.items():
        if task in closed and _closed_after(closed[task], ev.get("ts")):
            continue
        open_tasks.append(task)
        r = roles.get(task, {})
        if ev["tipo"] == "entrega":
            owner = r.get("par")
        elif ev["tipo"] == "veredito" and ev.get("resultado") == "devolvido":
            owner = None  # the arbiter's, and he is always watched
        else:
            owner = r.get("executor")
        if owner and owner not in ball:
            ball.append(owner)
    return {"roles": roles, "ball": ball, "arbiter": arbiter, "open": open_tasks, "ended": ended,
            "replaced": replaced}


def done(d: Path) -> list[tuple[str, str]]:
    """Sessions whose part is over, with why: the watchdog closes these. Never the arbiter of the
    moment, never an executor or reviewer of an open Task, never a name the events do not carry."""
    st = state(d)
    busy = {st["roles"].get(t, {}).get(k) for t in st["open"] for k in ("executor", "par")}
    out: dict[str, str] = {}
    if st["ended"]:
        for r in st["roles"].values():
            for k in ("executor", "par"):
                if r.get(k):
                    out.setdefault(r[k], "execution ended")
    for task in sorted(k for k in _closed(d) if isinstance(k, int)):
        ex = st["roles"].get(task, {}).get("executor")
        if ex and task not in st["open"]:
            out.setdefault(ex, f"Task {task} closed")
    for de, para in st["replaced"]:
        if de:
            out.setdefault(de, f"replaced by {para}")
    return [(n, why) for n, why in out.items() if n not in busy and n != st["arbiter"]]


def team(d: Path) -> list[str]:
    """Who must be in the arbiter's group: the open Tasks' executors and reviewers, then him."""
    st = state(d)
    names = [st["roles"].get(t, {}).get(k) for t in st["open"] for k in ("executor", "par")]
    return [n for n in dict.fromkeys(names) if n and n != st["arbiter"]] + [st["arbiter"]]


def _event_line(ev: dict) -> str:
    parts = [ev["tipo"]]
    if "task" in ev:
        parts.append(f"T{ev['task']}")
    if "rodada" in ev:
        parts.append(f"r{ev['rodada']}")
    for k in ("resultado", "fase", "commit", "sessao", "executor", "par", "de", "para", "motivo"):
        if k in ev:
            parts.append(f"{k}={ev[k]}")
    if ev.get("reincide"):
        parts.append("reincide")
    return " ".join(parts)


def send(target: str, text: str, tmux: bool = False) -> None:
    """Wakes a session through hangar-send, keeping the caller's identity. ORQ_SEND: tests."""
    cmd = [os.environ.get("ORQ_SEND", "hangar-send")]
    if tmux:
        cmd.append("--tmux")
    try:
        r = subprocess.run(cmd + [target, text], capture_output=True, text=True,
                           timeout=SEND_TIMEOUT_S)
    except subprocess.TimeoutExpired:
        raise OrqError(f"hangar-send {target} did not answer in {SEND_TIMEOUT_S}s") from None
    if r.returncode != 0:
        raise OrqError(f"hangar-send {target} failed (rc={r.returncode}): {r.stderr.strip()[:200]}")


def _send_after_event(target: str, text: str, arbiter: str) -> None:
    """The event is already in eventos.jsonl here: running `orq event` again would log it twice."""
    try:
        send(target, text)
    except OrqError as e:
        again = (f"orq notify {shlex.quote(text)}" if target == arbiter
                 else f"hangar-send {shlex.quote(target)} {shlex.quote(text)}")
        raise OrqError(f"{e}. The event IS recorded: do not run `orq event` again. "
                       f"Resend with: {again}") from None


def _after_event(d: Path, ev: dict) -> None:
    """APROVA goes to the executor only; the arbiter wakes for DEVOLVIDO and a repeated cause.
    REPROVA wakes nobody: the reviewer already sent the recipe to the executor.
    A code-phase APROVA sends the executor to prove, never to commit."""
    if ev["tipo"] != "veredito":
        return
    st = state(d)
    task, rnd, res = ev["task"], ev["rodada"], ev["resultado"]
    if res == "aprova":
        ex = st["roles"].get(task, {}).get("executor")
        if not ex:
            raise OrqError(f"Task {task} has no task_inicio: executor unknown")
        if ev.get("fase") == "codigo":
            obj = _code_approved_object(d, task) or "<the approved stash hash>"
            _send_after_event(ex, f"CODE OK Task {task} round {rnd}: prove it on this exact code, "
                                  f"then `orq event entrega --task {task} --rodada {rnd + 1} "
                                  f"--fase prova --commit {obj}`. Do not commit yet.",
                              st["arbiter"])
            return
        _send_after_event(ex, f"APROVA Task {task} round {rnd}: commit only the Task's paths, by "
                              f"explicit path, then run `orq commit --task {task} --hash <hash>`.",
                          st["arbiter"])
    elif res == "devolvido" or ev.get("reincide"):
        extra = " (reincide)" if ev.get("reincide") else ""
        _send_after_event(st["arbiter"], f"[decisao] Task {task} round {rnd}: {res}{extra}. "
                                         f"Report: {ev.get('motivo', 'see the journal')}",
                          st["arbiter"])


def _contract_parts(text: str) -> tuple[str, dict[int, str]]:
    """The contract's common part and each `## Task N` section."""
    common: list[str] = []
    sections: dict[int, list[str]] = {}
    cur = None
    for line in text.splitlines(keepends=True):
        m = TASK_HEAD.match(line)
        if m:
            cur = int(m.group(1))
            sections[cur] = [line]
            continue
        if cur is not None and line.startswith("## "):
            cur = None
        (sections[cur] if cur is not None else common).append(line)
    return "".join(common), {k: "".join(v) for k, v in sections.items()}


def _over_cap(common: str) -> str | None:
    """Every executor and reviewer re-reads the common part on each response of the session."""
    if len(common) <= COMMON_CAP:
        return None
    return (f"the contract's common part has {len(common)} characters (cap {COMMON_CAP}): "
            "the arbiter must cut it — Task specifics go in `## Task N` sections")


def cmd_init(a) -> int:
    d = base_dir(a.dir)
    contract = Path(a.contract).expanduser().resolve()
    try:
        too_big = _over_cap(_contract_parts(contract.read_text(encoding="utf-8"))[0])
    except FileNotFoundError:
        too_big = None  # written after init: `orq read contract` enforces the cap then
    except (OSError, UnicodeDecodeError) as e:
        raise OrqError(f"contract unreadable: {contract}: {e}")
    if too_big:
        raise OrqError(too_big)
    cfg = {"arbiter": a.arbiter, "repo": str(Path(a.repo).expanduser().resolve()),
           "contract": str(contract), "untouchables": a.untouchable}
    (d / "orq.json").write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")
    journal_append(d, f"orq init: arbiter={a.arbiter} repo={cfg['repo']}")
    print("ok")
    return 0


def cmd_event(a) -> int:
    d = base_dir(a.dir)
    config(d)
    ev = {"tipo": a.tipo}
    for k in EVENT_FIELDS_INT + EVENT_FIELDS_STR:
        v = getattr(a, k)
        if v is not None:
            ev[k] = v
    if a.reincide:
        ev["reincide"] = True
    if ev.get("tipo") == "entrega" and ev.get("fase") == "prova":
        ok = _code_approved_object(d, ev.get("task"))
        got = ev.get("commit") or ""
        if not ok or not got or not (ok.startswith(got) or got.startswith(ok)):
            raise OrqError("the proof must run on the approved code: deliver a code round first "
                           f"(approved code: {ok or 'none'}, given: {got or 'none'})")
        ev["commit"] = max(ok, got, key=len)  # a short prefix never reaches `orq commit`
    if ev.get("tipo") == "veredito":
        # A verdict without the round's phase would send a code round straight to commit.
        ent = next((x for x in reversed(events(d)) if x.get("tipo") == "entrega"
                    and x.get("task") == ev.get("task") and x.get("rodada") == ev.get("rodada")), None)
        if ent and ent.get("fase") and ent["fase"] != ev.get("fase"):
            raise OrqError("verdict phase must match the delivered round: "
                           f"round {ev.get('rodada')} was delivered with --fase {ent['fase']}")
    ev = event_append(d, ev)
    journal_append(d, _event_line(ev))
    _after_event(d, ev)
    print("ok")
    return 0


def cmd_read(a) -> int:
    d = base_dir(a.dir)
    if a.what == "contract":
        if a.task is None:
            raise OrqError("read contract needs --task N")
        path = config(d)["contract"]
        try:
            text = Path(path).read_text(encoding="utf-8")
        except FileNotFoundError:
            raise OrqError(f"contract not found: {path}") from None
        common, sections = _contract_parts(text)
        too_big = _over_cap(common)
        if too_big:
            raise OrqError(too_big)
        own = sections.get(a.task, "")
        print(common.rstrip())
        print("\n" + own.rstrip() if own else f"\n(no '## Task {a.task}' section in the contract)")
        return 0
    j = d / "registro.md"
    lines = j.read_text(encoding="utf-8").splitlines() if j.exists() else []
    pat = re.compile(rf"\bT{a.task}\b|\bTask {a.task}\b") if a.task is not None else None
    start = max(0, len(lines) - a.last)
    print("\n".join(l for i, l in enumerate(lines) if i >= start or (pat and pat.search(l))))
    return 0


def cmd_ball(a) -> int:
    st = state(base_dir(a.dir))
    names = st["ball"]
    if a.with_arbiter:
        # The watchdog's list: the arbiter of the moment last, so it follows succession.
        names = [n for n in names if n != st["arbiter"]] + [st["arbiter"]]
    print(" ".join(names))
    return 0


def cmd_done(a) -> int:
    for name, why in done(base_dir(a.dir)):
        print(f"{name} {why}")
    return 0


def cmd_team(a) -> int:
    print(" ".join(team(base_dir(a.dir))))
    return 0


def _lock_owner(lock: Path) -> str | None:
    try:
        return lock.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return None


def cmd_screen(a) -> int:
    d = base_dir(a.dir)
    lock = d / "screen.lock"
    if a.action == "release":
        held = _lock_owner(lock)
        if held != a.owner:
            print(f"not yours: held by {held or 'nobody'}")
            return 1
        lock.unlink()
        journal_append(d, f"screen released by {a.owner}")
        print("released")
        return 0
    deadline = time.time() + a.wait_min * 60
    while True:
        try:
            fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
        except FileExistsError:
            held = _lock_owner(lock)
            try:
                age = time.time() - lock.stat().st_mtime
            except FileNotFoundError:
                continue
            if held == a.owner:
                os.utime(lock)
                print("already yours (renewed)")
                return 0
            if age > SCREEN_STALE_S:
                lock.unlink(missing_ok=True)
                journal_append(d, f"screen lock of {held} stale ({int(age // 60)} min), taken by {a.owner}")
                continue
            if time.time() >= deadline:
                print(f"held by {held} for {int(age // 60)} min")
                return 1
            time.sleep(min(5.0, max(0.1, deadline - time.time())))
            continue
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(a.owner)
        journal_append(d, f"screen taken by {a.owner}")
        print("taken")
        return 0


MARK = re.compile(r"^\s*\[(aviso|decis[aã]o)\]", re.IGNORECASE)


def git(repo: str, *args: str) -> str:
    # Unquoted paths: an escaped accented name would slip past the untouchables glob.
    r = subprocess.run(["git", "-c", "core.quotePath=false", "-C", repo, *args],
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise OrqError(f"git {' '.join(args)}: {r.stderr.strip()[:200]}")
    return r.stdout


def _approved_object(d: Path, task: int) -> str | None:
    """The stash object of the round the last APROVA of this Task judged.
    A code-phase APROVA never releases the commit: the proof has to pass first."""
    evs = events(d)
    rnd = next((ev.get("rodada") for ev in reversed(evs) if ev.get("tipo") == "veredito"
                and ev.get("task") == task and ev.get("resultado") == "aprova"
                and ev.get("fase") != "codigo"), None)
    if rnd is None:
        return None
    return next((ev.get("commit") for ev in reversed(evs) if ev.get("tipo") == "entrega"
                 and ev.get("task") == task and ev.get("rodada") == rnd), None)


def _code_approved_object(d: Path, task: int) -> str | None:
    """The stash object of the round the last code-phase APROVA of this Task judged."""
    evs = events(d)
    rnd = next((ev.get("rodada") for ev in reversed(evs) if ev.get("tipo") == "veredito"
                and ev.get("task") == task and ev.get("resultado") == "aprova"
                and ev.get("fase") == "codigo"), None)
    if rnd is None:
        return None
    return next((ev.get("commit") for ev in reversed(evs) if ev.get("tipo") == "entrega"
                 and ev.get("task") == task and ev.get("rodada") == rnd), None)


def cmd_commit(a) -> int:
    """The arbiter's step-5.1 metadata check, done here so the arbiter wakes once per Task."""
    d = base_dir(a.dir)
    cfg = config(d)
    repo = a.repo or cfg["repo"]
    problems = []
    full = git(repo, "rev-parse", "--verify", f"{a.hash}^{{commit}}").strip()
    head = git(repo, "rev-parse", "HEAD").strip()
    if full != head:
        problems.append(f"{a.hash} is not the tip (HEAD={head[:12]})")
    files: set[str] = set()
    obj = _approved_object(d, a.task)
    if obj is None:
        problems.append(f"no APROVA for Task {a.task} with a delivered round object")
    elif subprocess.run(["git", "-C", repo, "rev-parse", "--verify", "--quiet", f"{obj}^2"],
                        capture_output=True).returncode != 0:
        # A stash has two parents (base, index); an old-contract round was a diff file.
        problems.append(f"round object {obj} is not a stash commit; freeze rounds with git stash "
                        "create + git stash store (executor.md step 5)")
    else:
        # --no-renames: a renamed path shows both names, so neither side hides behind the other.
        # Everything since the round's base, so correction commits count as a whole.
        files = set(git(repo, "diff", "--name-only", "--no-renames", f"{obj}^1", full).splitlines()) - {""}
        # ^2 is the index the executor staged: the Task's paths, not the arbiter's dirty plan.
        rnd = set(git(repo, "diff", "--name-only", "--no-renames", f"{obj}^1", f"{obj}^2").splitlines()) - {""}
        if files != rnd:
            problems.append(f"files differ from the approved round {obj[:12]}: "
                            f"only in commit {sorted(files - rnd)}, only in round {sorted(rnd - files)}")
        if rnd:
            # Against the tree the reviewer judged (the stash itself), unstaged edits included.
            changed = sorted(set(git(repo, "--literal-pathspecs", "diff", "--name-only", "--no-renames",
                                     obj, full, "--", *sorted(rnd)).splitlines()) - {""})
            if changed:
                problems.append(f"content differs from the approved round in: {changed}")
        # Per commit, not the net diff: history is never rewritten, so a reverted untouchable
        # still sits in it and only the arbiter or the user may accept that.
        touched = set(git(repo, "log", "--no-renames", "--name-only", "--format=",
                          f"{obj}^1..{full}").splitlines()) - {""}
        bad = sorted({f for f in touched for pat in cfg.get("untouchables", []) if fnmatch.fnmatch(f, pat)})
        if bad:
            problems.append(f"untouchable in commit history: {bad}")
    if problems:
        print("REFUSED:\n- " + "\n- ".join(problems))
        return 1
    with (d / "closed.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"ts": now(), "task": a.task, "hash": full}) + "\n")
    journal_append(d, f"commit T{a.task} {full[:12]} checked ({len(files)} file(s))")
    send(state(d)["arbiter"], f"[decisao] Task {a.task} closed and checked: {full[:12]}, "
                              f"{len(files)} file(s), tip = hash, matches the approved round. "
                              "Release the next ready Task(s).")
    print("ok")
    return 0


def jev_key() -> str:
    k = os.environ.get("TYPESAFE_API_KEY", "")
    if k:
        return k
    try:
        loaded = json.loads((Path.home() / ".claude" / "settings.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ""
    env = loaded.get("env") if isinstance(loaded, dict) else None
    v = env.get("TYPESAFE_API_KEY") if isinstance(env, dict) else None
    return v if isinstance(v, str) else ""


def jev_ask(text: str) -> dict:
    """{'choice', 'p', 'veto'} or {'error'}; never raises — any failure wakes the arbiter."""
    key = jev_key()
    if not key:
        return {"error": "no key"}
    model = os.environ.get("JEV_MODEL", "").strip() or JEV_MODEL
    body = json.dumps({"model": model, "state": text[-20_000:], "questions": JEV_QUESTIONS}).encode()
    url = os.environ.get("ORQ_JEV_URL") or os.environ.get("JEV_ENDPOINT", "").strip() or JEV_URL
    req = urllib.request.Request(url, data=body,
                                 headers={"authorization": f"Bearer {key}",
                                          "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=JEV_TIMEOUT_S) as r:
            answers = json.load(r)["answers"]
        choice = answers["kind"].get("choice")
        # Only a "nothing" winner can drop; its probability, not `confidence`.
        p = (answers["kind"].get("probabilities") or {}).get("nothing") if choice == "nothing" else 0.0
        veto = {k: float(answers[k]["noul"]) for k in JEV_VETOES}
    except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError, KeyError,
            TypeError, AttributeError) as e:
        return {"error": f"{type(e).__name__}: {str(e)[:120]}"}
    return {"choice": choice, "p": float(p) if isinstance(p, (int, float)) else 0.0, "veto": veto}


def triage(d: Path, text: str, alarm: bool) -> str:
    """Unmarked message: 'drop' only in mode `on`, when the Jev is sure it asks nothing and no veto
    fires. Shadow (default) asks and records, and the arbiter wakes the same. Every real alarm
    needs action, so alarms wake without asking."""
    mode = os.environ.get("ORQ_JEV", "shadow")
    if mode == "off" or alarm:
        return "wake"
    r = jev_ask(text)
    would_drop = ("error" not in r and r["choice"] == "nothing" and r["p"] >= DISCARD_P
                  and all(v <= VETO_P for v in r["veto"].values()))  # NaN never drops
    try:
        with (d / "jev-shadow.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps({"ts": now(), "mode": mode, "alarm": alarm, "text": text[:500], **r,
                                "would_drop": would_drop}, ensure_ascii=False) + "\n")
    except OSError as e:
        # Losing the shadow record never costs the arbiter the message.
        print(f"orq: jev-shadow.jsonl not written: {e}", file=sys.stderr)
    return "drop" if mode == "on" and would_drop else "wake"


def cmd_notify(a) -> int:
    d = base_dir(a.dir)
    m = MARK.match(a.text)
    if m and m.group(1).lower() == "aviso":
        journal_append(d, f"aviso: {a.text}")
        print("journal")
        return 0
    if not m and triage(d, a.text, a.alarm) == "drop":
        journal_append(d, f"(jev: no action) {a.text}")
        print("journal (jev)")
        return 0
    # Before sending: a failed send still leaves the message in the journal. The prefixes are what
    # the panel's feed classifies by.
    journal_append(d, f"{'alarm' if a.alarm else 'notify → arbiter'}: {a.text}")
    try:
        send(state(d)["arbiter"], a.text, tmux=a.alarm)
    except OrqError as e:
        journal_append(d, f"notify FAILED: {e}")
        raise
    print("arbiter woken")
    return 0


def cmd_log(a) -> int:
    d = base_dir(a.dir)
    journal_append(d, (f"T{a.task} " if a.task is not None else "") + a.text)
    print("ok")
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="orq", description=__doc__.splitlines()[0])
    p.add_argument("--dir")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("init", help="write orq.json (arbiter, once at launch)")
    s.add_argument("--arbiter", required=True)
    s.add_argument("--repo", required=True)
    s.add_argument("--contract", required=True)
    s.add_argument("--untouchable", action="append", default=[])
    s = sub.add_parser("event", help="validate and append one eventos.jsonl line")
    s.add_argument("tipo")
    for k in EVENT_FIELDS_INT:
        s.add_argument(f"--{k}", type=int)
    for k in EVENT_FIELDS_STR:
        s.add_argument(f"--{k}")
    s.add_argument("--reincide", action="store_true")
    s = sub.add_parser("read", help="the contract's common part + one Task, or the journal's tail")
    s.add_argument("what", choices=["contract", "journal"])
    s.add_argument("--task", type=int)
    s.add_argument("--last", type=int, default=15)
    s = sub.add_parser("ball", help="who owes work now")
    s.add_argument("--with-arbiter", action="store_true", help="append the current arbiter, last")
    sub.add_parser("done", help="sessions whose part is over (the watchdog closes them)")
    sub.add_parser("team", help="who must be in the arbiter's group (the watchdog joins them)")
    s = sub.add_parser("screen", help="the shared-screen lock")
    s.add_argument("action", choices=["take", "release"])
    s.add_argument("--owner", required=True)
    s.add_argument("--wait-min", type=float, default=20)
    s = sub.add_parser("commit", help="check the Task's commit and close it")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--hash", required=True)
    s.add_argument("--repo", help="the checkout holding the commit (a batch worktree); default orq.json's")
    s = sub.add_parser("notify", help="the only path of a message to the arbiter")
    s.add_argument("--alarm", action="store_true")
    s.add_argument("text")
    s = sub.add_parser("log", help="a decision entry in the journal")
    s.add_argument("--task", type=int)
    s.add_argument("text")
    return p


CMDS = {"init": cmd_init, "event": cmd_event, "read": cmd_read, "ball": cmd_ball,
        "done": cmd_done, "team": cmd_team,
        "screen": cmd_screen, "commit": cmd_commit, "notify": cmd_notify, "log": cmd_log}


def main(argv: list[str] | None = None) -> int:
    a = build_parser().parse_args(argv)
    try:
        return CMDS[a.cmd](a)
    except OrqError as e:
        print(f"orq: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
