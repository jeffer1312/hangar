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
                    "de", "para", "plano", "branch", "gid")
JEV_URL = "https://api.typesafe.ai/v1/systemone"
JEV_MODEL = "jev-1.13.0"
JEV_TIMEOUT_S = 5
DISCARD_P = 0.9
# (instruction, options, the option that means "no need to wake the arbiter")
JEV_QUESTIONS = {
    False: ("What does the coordinator of a software team need to do with this message from a "
            "worker session?",
            {"no_action": "nothing: it is a status note, a confirmation or an acknowledgement",
             "decision": "decide, authorize, unblock or answer something the sender asks",
             "round_report": "read a report of finished work",
             "none": "none of these"},
            "no_action"),
    True: ("A watchdog alarm about a worker session in a software team. What is the session doing?",
           {"idle": "it stopped and owes work",
            "stuck": "it claims to work but nothing moves",
            "waiting_as_told": "it is waiting exactly as it was told to",
            "none": "none of these"},
           "waiting_as_told"),
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


def _closed(d: Path) -> set:
    p = d / "closed.jsonl"
    if not p.exists():
        return set()
    out = set()
    for line in p.read_text(encoding="utf-8").splitlines():
        try:
            out.add(json.loads(line).get("task"))
        except ValueError:
            continue
    return out


def state(d: Path) -> dict:
    """One pass over the events: who executes and reviews each Task (after swaps), who has the
    ball, and who the arbiter is now. The ball table is arbitro-vigia.md's."""
    arbiter = config(d)["arbiter"]
    roles: dict[int, dict] = {}
    last: dict[int, dict] = {}
    ended = False
    for ev in events(d):
        t = ev.get("tipo")
        if t == "task_inicio":
            roles[ev.get("task")] = {"executor": ev.get("executor"), "par": ev.get("par")}
            last[ev.get("task")] = ev
        elif t in ("entrega", "veredito"):
            last[ev.get("task")] = ev
        elif t == "sessao_trocada":
            if ev.get("de") == arbiter:
                arbiter = ev.get("para")
            for r in roles.values():
                for k in ("executor", "par"):
                    if r.get(k) == ev.get("de"):
                        r[k] = ev.get("para")
        elif t == "execucao_fim":
            ended = True
    ball: list[str] = []
    if not ended:
        closed = _closed(d)
        for task, ev in last.items():
            if task in closed:
                continue
            r = roles.get(task, {})
            if ev["tipo"] == "entrega":
                owner = r.get("par")
            elif ev["tipo"] == "veredito" and ev.get("resultado") == "devolvido":
                owner = None  # the arbiter's, and he is always watched
            else:
                owner = r.get("executor")
            if owner and owner not in ball:
                ball.append(owner)
    return {"roles": roles, "ball": ball, "arbiter": arbiter}


def _event_line(ev: dict) -> str:
    parts = [ev["tipo"]]
    if "task" in ev:
        parts.append(f"T{ev['task']}")
    if "rodada" in ev:
        parts.append(f"r{ev['rodada']}")
    for k in ("resultado", "commit", "sessao", "executor", "par", "de", "para", "motivo"):
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
    r = subprocess.run(cmd + [target, text], capture_output=True, text=True)
    if r.returncode != 0:
        raise OrqError(f"hangar-send {target} failed (rc={r.returncode}): {r.stderr.strip()[:200]}")


def _after_event(d: Path, ev: dict) -> None:
    """APROVA goes to the executor only; the arbiter wakes for DEVOLVIDO and a repeated cause.
    REPROVA wakes nobody: the reviewer already sent the recipe to the executor."""
    if ev["tipo"] != "veredito":
        return
    st = state(d)
    task, rnd, res = ev["task"], ev["rodada"], ev["resultado"]
    if res == "aprova":
        ex = st["roles"].get(task, {}).get("executor")
        if not ex:
            raise OrqError(f"Task {task} has no task_inicio: executor unknown")
        send(ex, f"APROVA Task {task} round {rnd}: commit only the Task's paths, by explicit path, "
                 f"then run `orq commit --task {task} --hash <hash>`.")
    elif res == "devolvido" or ev.get("reincide"):
        extra = " (reincide)" if ev.get("reincide") else ""
        send(st["arbiter"], f"[decisao] Task {task} round {rnd}: {res}{extra}. "
                            f"Report: {ev.get('motivo', 'see the journal')}")


def cmd_init(a) -> int:
    d = base_dir(a.dir)
    cfg = {"arbiter": a.arbiter, "repo": str(Path(a.repo).expanduser().resolve()),
           "contract": str(Path(a.contract).expanduser().resolve()), "untouchables": a.untouchable}
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
        c = "".join(common)
        if len(c) > COMMON_CAP:
            print(f"orq: the contract's common part has {len(c)} characters (cap {COMMON_CAP}); "
                  "tell the arbiter", file=sys.stderr)
        own = "".join(sections.get(a.task, []))
        print(c.rstrip())
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
    """The stash object of the round the last APROVA of this Task judged."""
    evs = events(d)
    rnd = next((ev.get("rodada") for ev in reversed(evs) if ev.get("tipo") == "veredito"
                and ev.get("task") == task and ev.get("resultado") == "aprova"), None)
    if rnd is None:
        return None
    return next((ev.get("commit") for ev in reversed(evs) if ev.get("tipo") == "entrega"
                 and ev.get("task") == task and ev.get("rodada") == rnd), None)


def cmd_commit(a) -> int:
    """The arbiter's step-5.1 metadata check, done here so the arbiter wakes once per Task."""
    d = base_dir(a.dir)
    cfg = config(d)
    repo = cfg["repo"]
    problems = []
    full = git(repo, "rev-parse", "--verify", f"{a.hash}^{{commit}}").strip()
    head = git(repo, "rev-parse", "HEAD").strip()
    if full != head:
        problems.append(f"{a.hash} is not the tip (HEAD={head[:12]})")
    files = set(git(repo, "show", "--name-only", "--format=", full).splitlines()) - {""}
    obj = _approved_object(d, a.task)
    if obj is None:
        problems.append(f"no APROVA for Task {a.task} with a delivered round object")
    else:
        # ^2 is the index the executor staged: the Task's paths, not the arbiter's dirty plan.
        rnd = set(git(repo, "diff", "--name-only", f"{obj}^1", f"{obj}^2").splitlines()) - {""}
        if files != rnd:
            problems.append(f"files differ from the approved round {obj[:12]}: "
                            f"only in commit {sorted(files - rnd)}, only in round {sorted(rnd - files)}")
    bad = sorted({f for f in files for pat in cfg.get("untouchables", []) if fnmatch.fnmatch(f, pat)})
    if bad:
        problems.append(f"untouchable in the commit: {bad}")
    if problems:
        print("REFUSED:\n- " + "\n- ".join(problems))
        return 1
    with (d / "closed.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"ts": now(), "task": a.task, "hash": full}) + "\n")
    journal_append(d, f"commit T{a.task} {full[:12]} checked ({len(files)} file(s))")
    send(state(d)["arbiter"], f"[decisao] Task {a.task} closed and checked: {full[:12]}, "
                              f"{len(files)} file(s), tip = hash, matches the approved round. "
                              "Release the next Task.")
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


def jev_ask(text: str, alarm: bool) -> dict:
    """{'choice', 'p'} or {'error'}; never raises — any failure wakes the arbiter."""
    key = jev_key()
    if not key:
        return {"error": "no key"}
    instructions, criteria, _ = JEV_QUESTIONS[alarm]
    body = json.dumps({"model": JEV_MODEL, "state": text[-20_000:],
                       "questions": {"kind": {"type": "choice", "instructions": instructions,
                                              "criteria": criteria}}}).encode()
    req = urllib.request.Request(os.environ.get("ORQ_JEV_URL", JEV_URL), data=body,
                                 headers={"authorization": f"Bearer {key}",
                                          "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=JEV_TIMEOUT_S) as r:
            ans = json.load(r)["answers"]["kind"]
        choice = ans.get("choice")
        p = (ans.get("probabilities") or {}).get(choice)
    except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError, KeyError,
            TypeError, AttributeError) as e:
        return {"error": f"{type(e).__name__}: {str(e)[:120]}"}
    # The winner's probability, not `confidence`: the Jev calibrates them differently.
    return {"choice": choice, "p": float(p) if isinstance(p, (int, float)) else 0.0}


def triage(d: Path, text: str, alarm: bool) -> str:
    """Unmarked message: 'drop' only in mode `on` and when the Jev is sure it needs no action.
    Shadow (default) asks and records, and the arbiter wakes the same."""
    mode = os.environ.get("ORQ_JEV", "shadow")
    if mode == "off":
        return "wake"
    r = jev_ask(text, alarm)
    would_drop = r.get("choice") == JEV_QUESTIONS[alarm][2] and r.get("p", 0.0) >= DISCARD_P
    with (d / "jev-shadow.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"ts": now(), "mode": mode, "alarm": alarm, "text": text[:500], **r,
                            "would_drop": would_drop}, ensure_ascii=False) + "\n")
    return "drop" if mode == "on" and would_drop else "wake"


def cmd_notify(a) -> int:
    d = base_dir(a.dir)
    m = MARK.match(a.text)
    if m and m.group(1).lower() == "aviso":
        journal_append(d, a.text)
        print("journal")
        return 0
    if not m and triage(d, a.text, a.alarm) == "drop":
        journal_append(d, f"(jev: no action) {a.text}")
        print("journal (jev)")
        return 0
    send(state(d)["arbiter"], a.text, tmux=a.alarm)
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
    s = sub.add_parser("screen", help="the shared-screen lock")
    s.add_argument("action", choices=["take", "release"])
    s.add_argument("--owner", required=True)
    s.add_argument("--wait-min", type=float, default=20)
    s = sub.add_parser("commit", help="check the Task's commit and close it")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--hash", required=True)
    s = sub.add_parser("notify", help="the only path of a message to the arbiter")
    s.add_argument("--alarm", action="store_true")
    s.add_argument("text")
    s = sub.add_parser("log", help="a decision entry in the journal")
    s.add_argument("--task", type=int)
    s.add_argument("text")
    return p


CMDS = {"init": cmd_init, "event": cmd_event, "read": cmd_read, "ball": cmd_ball,
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
