#!/usr/bin/env python3
"""orq — the orchestration's conductor: transport and bookkeeping without waking the arbiter.

Events, the journal, the commit check, the shared-resource locks, who has the ball and the triage
of messages to the arbiter live here, so the arbiter's session wakes only for decisions.

Directory: `--dir D` (before the command) or ORQ_DIR — the durable dir ~/.hangar/orq/<date>-<gid>/.
Config: <dir>/orq.json, written once by `orq init` at launch. Stdlib only.
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import http.client
import importlib.util
import io
import json
import math
import os
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unicodedata
import urllib.error
import urllib.request
from collections import Counter
from contextlib import redirect_stdout
from datetime import datetime
from functools import cache
from pathlib import Path

HERE = Path(__file__).resolve().parent
JOURNAL_CAP = 40_000
COMMON_CAP = 8_000
# A session that died holding a shared resource cannot freeze the team.
LOCK_STALE_S = 60 * 60
TASK_HEAD = re.compile(r"^## Task (\d+)\b")
SECTION = re.compile(r"^## (.+?)\s*$")
# The whole line, newline included: removing it must give back the text that was hashed.
PREPARADO = re.compile(r"^Preparado: .* · sha ([0-9a-f]{12})[ \t]*(?:\n|$)", re.MULTILINE)
PROJETO_KEYS = {"checagens": "Checagens", "integracao": "Integração", "prova": "Prova",
                "paralelo": "Paralelo", "correcao": "Correção pelo revisor", "revisao": "Revisão"}
SUBAGENT = "subagente"   # `--par` of a Task reviewed by a subagent: a name, never a session
CHECK_TIMEOUT_S = 900
# The merge commit runs the repo's hooks, which may run checks of their own.
MERGE_TIMEOUT_S = CHECK_TIMEOUT_S
TIMELINE_KINDS = ("advance", "woke", "dropped", "would_drop", "failed", "notice")
EVENT_FIELDS_INT = ("task", "rodada")
EVENT_FIELDS_STR = ("commit", "resultado", "sessao", "motivo", "titulo", "executor", "par",
                    "de", "para", "plano", "branch", "gid", "fase", "patch", "ate")
JEV_URL = "https://api.typesafe.ai/v1/systemone"
JEV_MODEL = "jev-1.13.0"
# The same Jev served by OpenRouter, for a `sk-or-` key with no endpoint configured.
OPENROUTER_JEV_URL = "https://openrouter.ai/api/alpha/decisions"
OPENROUTER_JEV_MODEL = "typesafe/jev-1.13-20260917"
JEV_TIMEOUT_S = 5
# A hung backend cannot hang the session that called orq.
SEND_TIMEOUT_S = 30
SEND_TRIES = 3   # failed wakes of the batch or final step before it counts as failed
# Calibrated on real arbiter messages: changing a word or a threshold means measuring again.
DISCARD_P = 0.85  # p of "nothing" (the choice's winner) needed to drop
VETO_P = 0.40     # any alert above this keeps the arbiter awake
JEV_VETOES = ("context", "user", "problem", "deviation")
# `orq init --auto` without `--jev`: "on" only once these questions were measured with no wrong drop.
AUTO_JEV_DEFAULT = "on"
JEV_QUESTIONS = {
    "kind": {
        "type": "choice",
        "instructions": ("A session of a software team sent this message to the team's coordinator. "
                         "Routine bookkeeping is automatic; the coordinator is needed only to decide "
                         "or act. What does the coordinator have to do with this message?"),
        "criteria": {
            "act": ("decide or act: answer a question; grant a permission or a go-ahead (time on a "
                    "shared resource, more actions, 'may I', 'waiting for your OK'); choose between "
                    "options; handle a failure, blocker or environment problem; open, name or replace "
                    "a session (the sender is at its context limit or hands over, a session is gone, a "
                    "reviewer must be named); settle a disagreement or a change of plan; record a "
                    "decision from the user; or release the next Task after a commit"),
            "nothing": ("nothing: the message only informs - progress, a status note, an "
                        "acknowledgement, a wake-up or environment confirmation, a shared resource "
                        "taken or released, a round delivered to the reviewer, a verdict already sent "
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


class SendError(OrqError):
    """hangar-send did not deliver: the step may be tried again."""


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


def is_auto(d: Path) -> bool:
    """An orquestrar-auto run; any unreadable orq.json is a plain run."""
    try:
        cfg = json.loads((d / "orq.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return isinstance(cfg, dict) and cfg.get("auto") is True


def timeline(d: Path, kind: str, text: str, task: int | None = None, *, notify: bool = False,
             sender: str | None = None) -> None:
    """One pt-BR line of what the orchestrator did, shown by Hangar's orq row. Auto runs only;
    the file name carries the run so two runs never share an SSE id."""
    if kind not in TIMELINE_KINDS:
        raise ValueError(f"unknown timeline kind: {kind}")
    if not is_auto(d):
        return
    row = {"ts": now(), "kind": kind, "text": text, "task": task}
    if notify:
        # A chave diz ao Hangar que a linha veio do `notify`, mesmo com remetente desconhecido.
        row["from"] = sender
    line = json.dumps(row, ensure_ascii=False)
    try:
        with (d / f"timeline-{d.resolve().name}.jsonl").open("a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError as e:
        # A view of the run: losing a line never costs the arbiter a message.
        print(f"orq: timeline not written: {e}", file=sys.stderr)


def _sender(alarm: bool) -> str | None:
    """Quem chamou `orq notify`, pela identidade que assina o recado; na dúvida, ninguém."""
    if alarm:
        return "vigia"
    try:
        r = subprocess.run([os.environ.get("ORQ_WHOAMI", "hangar-send"), "--whoami"],
                           capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        return None
    name = r.stdout.strip() if r.returncode == 0 else ""
    # "cli" e o "aviso:" são o fallback do me(): o nome pode ser o de outra sessão.
    return name if name and name != "cli" and "aviso:" not in r.stderr else None


def _session_ids(name: str) -> dict:
    """Como achar o transcript de uma sessão do time depois que ela fecha: ids, nunca o caminho
    (o rollout do Codex só nasce no primeiro turno)."""
    ids = dict.fromkeys(("provider", "session_id", "config_dir", "thread_id", "codex_home"))
    for sub, dflt in (("claude-headless", "claude"), ("codex-sessions", "codex")):
        f = Path.home() / ".hangar" / sub / f"{name}.json"
        if f.exists():
            sc = json.loads(f.read_text(encoding="utf-8"))
            return {**ids, **{k: sc.get(k) for k in ids}, "provider": sc.get("provider") or dflt}
    r = subprocess.run(["tmux", "display", "-p", "-t", f"={name}:", "#{pane_start_command}"],
                       capture_output=True, text=True, timeout=5)
    m = re.search(r"--session-id[ =]([0-9a-fA-F-]{36})", r.stdout) if r.returncode == 0 else None
    if m:
        env = subprocess.run(["tmux", "show-environment", "-t", f"={name}", "CLAUDE_CONFIG_DIR"],
                             capture_output=True, text=True, timeout=5).stdout.strip()
        ids.update(provider="claude", session_id=m.group(1),
                   config_dir=env.split("=", 1)[1] if env.startswith("CLAUDE_CONFIG_DIR=") else None)
    return ids


def _record_session(d: Path, name: str, role: str | None, task: int | None) -> None:
    """Uma linha em sessions.jsonl; é visão do Hangar e nunca custa o passo de quem chama."""
    try:
        row = {"ts": now(), "name": name, "role": role, "task": task, **_session_ids(name)}
        with (d / "sessions.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
    except Exception as e:  # noqa: BLE001 — a view of the run: never breaks the step that called it
        _journal_or_warn(d, f"sessions.jsonl not written for {name}: {e}")


def _validator():
    spec = importlib.util.spec_from_file_location("orq_valida_eventos", HERE / "orq-valida-eventos.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


@cache
def _identity_reader():
    path = HERE.parents[2] / "backend" / "app" / "orq_identity.py"
    spec = importlib.util.spec_from_file_location("orq_session_identity", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def session_identity(name: str) -> str:
    if "::" in name:
        raise OrqError("cannot capture another server's session locally")
    identity = _identity_reader().identity(name)
    if not isinstance(identity, str) or not identity:
        raise OrqError("session identity is empty")
    return identity


def capture_identities(d: Path, names) -> dict[str, str]:
    captured = {}
    for name in dict.fromkeys(names):
        if not name or name == SUBAGENT:
            continue
        try:
            captured[name] = session_identity(name)
        except (OrqError, OSError, ValueError, ImportError) as e:
            # Falta de identidade não autoriza associar outro trabalho pelo nome.
            journal_append(d, f"session identity unavailable: {name}: {type(e).__name__}")
    return captured


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
    if ev.get("tipo") == "task_inicio":
        ev["session_identities"] = capture_identities(d, (ev.get("executor"), ev.get("par")))
    elif ev.get("tipo") == "sessao_trocada":
        captured = capture_identities(d, (ev.get("para"),))
        if captured.get(ev.get("para")):
            ev["session_identity"] = captured[ev["para"]]
    with (d / "eventos.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps(ev, ensure_ascii=False) + "\n")
    if ev.get("tipo") in {"task_inicio", "execucao_fim"}:
        snapshot_plan(d)
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


def _until_future(deadline) -> bool:
    """Prazo inválido não dispensa ninguém: o responsável continua com a bola."""
    try:
        return datetime.fromisoformat(deadline) > datetime.now().astimezone()
    except (TypeError, ValueError):
        return False


def state(d: Path) -> dict:
    """One pass over the events: who executes and reviews each Task (after swaps), who has the
    ball, who the arbiter is now, which Tasks are open, whether the run has ended, and which swaps
    replaced an executor or reviewer. The ball table is arbitro-vigia.md's."""
    arbiter = config(d)["arbiter"]
    roles: dict[int, dict] = {}
    last: dict[int, dict] = {}
    ended = False
    replaced: list[tuple[str, str]] = []
    waits: dict[str, dict] = {}   # sessão → última espera ainda não encerrada por evento
    for ev in events(d):
        t = ev.get("tipo")
        if t == "espera":
            waits[ev.get("sessao")] = ev
            continue
        # Só evento registrado encerra a espera antes do prazo; `orq log` não é evento.
        waits.pop(ev.get("sessao"), None)
        if isinstance(ev.get("task"), int):
            # task_inicio nomeia suas sessões antes de registrá-las em roles.
            r = ev if t == "task_inicio" else roles.get(ev["task"], {})
            for who, w in list(waits.items()):
                if w.get("task") == ev["task"] or (w.get("task") is None and who in (r.get("executor"), r.get("par"))):
                    del waits[who]
        if t == "task_inicio":
            roles[ev.get("task")] = {"executor": ev.get("executor"), "par": ev.get("par")}
            last[ev.get("task")] = ev
        elif t in ("entrega", "veredito"):
            last[ev.get("task")] = ev
        elif t == "sessao_trocada":
            waits.pop(ev.get("de"), None)
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
            waits.clear()
            ended = True
        if t != "execucao_fim" and isinstance(ev.get("task"), int):
            ended = False
    owners: list[str] = []
    open_tasks: list[int] = []
    closed = _closed(d)
    for task, ev in last.items():
        if task in closed and _closed_after(closed[task], ev.get("ts")):
            continue
        open_tasks.append(task)
        r = roles.get(task, {})
        if ev["tipo"] == "entrega":
            # A subagent reviewer runs inside the executor's turn: the executor holds the ball.
            owner = r.get("executor") if r.get("par") == SUBAGENT else r.get("par")
        elif ev["tipo"] == "veredito" and ev.get("resultado") == "devolvido":
            owner = None  # the arbiter's, and he is always watched
        else:
            owner = r.get("executor")
        if owner and owner not in owners:
            owners.append(owner)
    # Fechar a Task encerra sua cobertura, mesmo sem evento de integração.
    waiting = {who for who, w in waits.items()
               if _until_future(w.get("ate")) and (w.get("task") is None or w["task"] in open_tasks)}
    ball = [o for o in owners if o not in waiting]
    return {"roles": roles, "ball": ball, "owners": owners, "waiting": waiting, "arbiter": arbiter,
            "open": open_tasks, "ended": ended, "replaced": replaced}


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
        if task in st["open"]:
            continue
        for role in ("executor", "par"):
            name = st["roles"].get(task, {}).get(role)
            if name:
                out.setdefault(name, f"Task {task} closed")
    for de, para in st["replaced"]:
        if de:
            out.setdefault(de, f"replaced by {para}")
    return [(n, why) for n, why in out.items()
            if n not in busy and n != st["arbiter"] and n != SUBAGENT]


def team(d: Path) -> list[str]:
    """Who must be in the arbiter's group: the open Tasks' executors and reviewers, then him."""
    st = state(d)
    names = [st["roles"].get(t, {}).get(k) for t in st["open"] for k in ("executor", "par")]
    return [n for n in dict.fromkeys(names) if n and n != st["arbiter"] and n != SUBAGENT] + [st["arbiter"]]


def _event_line(ev: dict) -> str:
    parts = [ev["tipo"]]
    if "task" in ev:
        parts.append(f"T{ev['task']}")
    if "rodada" in ev:
        parts.append(f"r{ev['rodada']}")
    for k in ("resultado", "fase", "commit", "sessao", "ate", "executor", "par", "de", "para", "motivo"):
        if k in ev:
            parts.append(f"{k}={ev[k]}")
    if ev.get("reincide"):
        parts.append("reincide")
    return " ".join(parts)


def send(target: str, text: str, tmux: bool = False, painel: bool = False) -> None:
    """Wakes a session through hangar-send, keeping the caller's identity; `painel` sends the text
    as a panel notice instead, without the caller's `[de: …]`. ORQ_SEND: tests."""
    cmd = [os.environ.get("ORQ_SEND", "hangar-send")]
    if tmux:
        cmd.append("--tmux")
    # Always explicit: a value inherited from the caller's environment must not relabel a session's message.
    env = {**os.environ, "HANGAR_SEND_PAINEL": "1" if painel else "0"}
    try:
        r = subprocess.run(cmd + [target, text], capture_output=True, text=True,
                           timeout=SEND_TIMEOUT_S, env=env)
    except subprocess.TimeoutExpired:
        raise SendError(f"hangar-send {target} did not answer in {SEND_TIMEOUT_S}s") from None
    if r.returncode != 0:
        raise SendError(f"hangar-send {target} failed (rc={r.returncode}): {r.stderr.strip()[:200]}")


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
    elif res == "corrige":
        ex = st["roles"].get(task, {}).get("executor")
        _send_after_event(ex, f"CORRIGE Task {task} round {rnd}: the reviewer's patch {ev['patch']} "
                              f"fixes the blockers. Run `orq apply-patch --task {task}` in your worktree "
                              "(plus `--repo <your worktree>` in a wave); nothing else to decide. If it "
                              f"fails, the patch is your recipe: fix it yourself and deliver round {rnd + 1} "
                              "as usual.", st["arbiter"])
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


def plan_text(path) -> str:
    try:
        return Path(path).expanduser().read_text(encoding="utf-8")
    except FileNotFoundError:
        raise OrqError(f"plan not found: {path}") from None


def snapshot_plan(d: Path, path: str | None = None) -> None:
    """Preserva o plano no fluxo de escrita; consultar o histórico não grava nada."""
    temporary = None
    try:
        if path is None:
            if not (d / "orq.json").exists():
                return
            path = json.loads((d / "orq.json").read_text(encoding="utf-8")).get("plan")
        if not path:
            return
        text = plan_text(path)
        spec = importlib.util.spec_from_file_location("orq_atomic", HERE.parents[2] / "backend" / "app" / "atomico.py")
        atomic = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(atomic)
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=d, prefix=".plan-snapshot-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(text)
        atomic.substituir(temporary, d / "plan.snapshot.md")
    except Exception as exc:
        _journal_or_warn(d, f"falha ao preservar plano: {exc}")
    finally:
        if temporary is not None:
            try:
                temporary.unlink(missing_ok=True)
            except OSError as exc:
                _journal_or_warn(d, f"falha ao remover cópia temporária do plano: {exc}")


def _section(text: str, name: str) -> list[str]:
    out, inside = [], False
    for line in text.splitlines():
        m = SECTION.match(line)
        if m:
            inside = m.group(1).strip() == name
            continue
        if inside:
            out.append(line)
    return out


def projeto(text: str) -> dict:
    """The plan's `## Projeto`: a missing line stays None, so plan-check can name it."""
    raw: dict[str, str] = {}
    for line in _section(text, "Projeto"):
        k, sep, v = line.partition(":")
        if sep:
            raw[k.strip().lower()] = v.strip()
    out: dict = {k: None for k in PROJETO_KEYS}
    for key, label in PROJETO_KEYS.items():
        v = raw.get(label.lower())
        if v is None:
            continue
        if key in ("checagens", "integracao"):
            out[key] = re.findall(r"`([^`]+)`", v)
        elif key == "prova":
            m = re.fullmatch(r"(nenhuma|por-task|manual)|lote\((\d+)\)", v)
            out[key] = ((m.group(1), 0) if m.group(1) else ("lote", int(m.group(2)))) if m else ("?", 0)
        elif key == "paralelo":
            m = re.fullmatch(r"sequencial|at[eé] (\d+)", v)
            out[key] = (1 if not m.group(1) else int(m.group(1))) if m else 0
        elif key == "revisao":
            out[key] = {"subagente": "subagente", "sessão": "sessao", "sessao": "sessao"}.get(v, "?")
        else:
            m = re.search(r"\d+", v)
            out[key] = int(m.group()) if m else 0
    return out


def plan_tasks(text: str) -> list[dict]:
    rows = [l for l in _section(text, "Tasks") if l.strip().startswith("|")]
    if len(rows) < 2:
        return []
    head = [c.strip().lower() for c in rows[0].strip().strip("|").split("|")]
    col = {name: head.index(name) for name in ("#", "what it is", "files", "verification", "wave",
                                               "risk", "roteiro") if name in head}
    out = []
    for row in rows[2:]:
        cells = [c.strip() for c in row.strip().strip("|").split("|")]
        get = lambda name: cells[col[name]] if name in col and col[name] < len(cells) else ""
        if not get("#").isdigit():
            continue
        rot = get("roteiro").strip("`")
        risk = get("risk").strip("`").lower()
        out.append({"n": int(get("#")), "title": get("what it is"),
                    "files": re.findall(r"`([^`]+)`", get("files")),
                    "verification": get("verification"), "wave": get("wave"),
                    "risk": "" if risk in ("", "—", "-") else risk,
                    "roteiro": "" if rot in ("", "—", "-") else rot})
    return out


def plan_sha(text: str) -> str:
    """Over the text without the `Preparado:` line and with blank runs collapsed, so stamping
    (which inserts that line) never changes the sha it writes."""
    body = re.sub(r"\n{3,}", "\n\n", PREPARADO.sub("", text))
    return hashlib.sha256(body.encode("utf-8")).hexdigest()[:12]


PLAN_TASK_HEAD = re.compile(r"^### Task \d+:", re.MULTILINE)
# Closing fence: same character, at least as long, up to 3 spaces of indent (CommonMark).
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})[^\n]*\n.*?^ {0,3}\1[`~]*[ \t]*$", re.MULTILINE | re.DOTALL)
OPEN_STEP = re.compile(r"^\s*- \[ \] \*\*Step", re.MULTILINE)
DONE_STEP = re.compile(r"^\s*- \[[xX]\] \*\*Step", re.MULTILINE)


def _repo_files(repo: Path, problems: list[str], *args: str) -> list[str]:
    # Exit 1 of `git grep` is "no match" and a folder outside git has no plan: both are empty, not
    # errors. Anything else (timeout, dubious ownership, no git) is reported, never read as "no plan".
    try:
        r = subprocess.run(["git", "-c", "core.quotePath=false", "-C", str(repo), *args],
                           capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired) as e:
        problems.append(f"git {args[0]}: {e}")
        return []
    if r.returncode == 0:
        return [line for line in r.stdout.splitlines() if line]
    if not (r.returncode == 1 and args[0] == "grep") and "not a git repository" not in r.stderr:
        problems.append(f"git {args[0]}: {r.stderr.strip()[:160]}")
    return []


def find_plan(repo: Path) -> dict:
    """The plan a run would start from, best first: stamped, stamped then edited, an unstamped
    `.orq.md`, then any plan in the Task/Step format, newest first. `path`/`state` only when one
    is found; `problems` = what could not be read (a plan may be hiding there); `finished` = the
    newest plan with every step ticked."""
    problems: list[str] = []
    orq = _repo_files(repo, problems, "ls-files", "-co", "--exclude-standard", "*.orq.md")
    tasks = _repo_files(repo, problems, "grep", "-l", "--untracked", "-E", r"^### Task [0-9]+:", "--", "*.md")
    # The superpowers plans folder is usually gitignored, so git sees none of it.
    kept = sorted(str(p.relative_to(repo)) for p in (repo / "docs/superpowers/plans").glob("*.md"))
    rank = {"stamped": 0, "changed": 1, "unstamped": 2, "tasks": 3}
    found, done = [], []
    for rel in dict.fromkeys(orq + tasks + kept):
        path = repo / rel
        try:
            text, mtime = path.read_text(encoding="utf-8"), path.stat().st_mtime
        except (OSError, UnicodeDecodeError) as e:
            problems.append(f"{path}: {type(e).__name__}")
            continue
        m = PREPARADO.search(text)
        # A fenced `### Task` is a doc showing the format, not a plan; every step ticked is work done.
        body = FENCE.sub("", text)
        if DONE_STEP.search(body) and not OPEN_STEP.search(body):
            done.append((mtime, str(path)))
            continue
        if not m and not rel.endswith(".orq.md") and not PLAN_TASK_HEAD.search(body):
            continue
        state = ("stamped" if m.group(1) == plan_sha(text) else "changed") if m else \
            "unstamped" if rel.endswith(".orq.md") else "tasks"
        found.append((rank[state], -mtime, str(path), state))
    out: dict = {}
    if found:
        _, _, path, state = min(found)
        out = {"path": path, "state": state}
    if problems:
        out["problems"] = problems
    if done:
        out["finished"] = max(done)[1]
    return out


def cmd_readiness(a) -> int:
    print(json.dumps(find_plan(Path(a.repo).expanduser().resolve()), ensure_ascii=False))
    return 0


def plan_of(d: Path) -> dict:
    """`{}` for a run started before plans were stamped: every plan-driven gate stays off."""
    p = config(d).get("plan")
    return projeto(plan_text(p)) if p else {}


def run_checks(repo: str, cmds: list[str], log: Path, beat: Path | None = None) -> tuple[bool, str]:
    """Each declared command in the repo, output to `log`; (ok, first failure's line). `beat` is
    touched before each command, so its age tells a slow pass from a stuck one."""
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("w", encoding="utf-8") as f:
        for c in cmds:
            _touch(beat)
            f.write(f"$ {c}\n")
            f.flush()
            try:
                r = subprocess.run(c, shell=True, cwd=repo, stdout=f, stderr=subprocess.STDOUT,
                                   timeout=CHECK_TIMEOUT_S)
                rc = r.returncode
            except subprocess.TimeoutExpired:
                rc = "timeout"
            if rc != 0:
                return False, f"check failed: `{c}` (rc={rc}), log {log}"
    return True, ""


def record_check(d: Path, task: int, commit: str, ok: bool, log: str) -> None:
    with (d / "checks.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"ts": now(), "task": task, "commit": commit, "ok": ok, "log": log}) + "\n")


def check_ok(d: Path, task: int, commit: str) -> bool:
    p = d / "checks.jsonl"
    # An empty prefix would match every recorded commit of the Task.
    if not commit or not p.exists():
        return False
    last = None
    for line in p.read_text(encoding="utf-8").splitlines():
        try:
            c = json.loads(line)
        except ValueError:
            continue
        got = c.get("commit") or ""
        if c.get("task") == task and got and (got.startswith(commit) or commit.startswith(got)):
            last = c
    return bool(last and last.get("ok"))


def cmd_check(a) -> int:
    d = base_dir(a.dir)
    cfg = config(d)
    repo = a.repo or cfg["repo"]
    full = git(repo, "rev-parse", "--verify", f"{a.commit}^{{commit}}").strip()
    # The checks judge the frozen round: a tree that moved since would make the log lie.
    if subprocess.run(["git", "-C", repo, "diff", "--quiet", full, "--"], capture_output=True).returncode != 0:
        raise OrqError(f"worktree differs from {full[:12]}: freeze the round first, then check")
    cmds = plan_of(d).get("checagens") or []
    log = d / "checks" / f"task{a.task}-{full[:12]}.log"
    if not cmds:
        record_check(d, a.task, full, True, "")
        print(f"check T{a.task}: nothing declared")
        return 0
    ok, why = run_checks(repo, cmds, log)
    record_check(d, a.task, full, ok, str(log))
    journal_append(d, f"check T{a.task} {full[:12]}: {'ok' if ok else why}")
    print(f"check T{a.task} ok {len(cmds)}/{len(cmds)}, log {log}" if ok else why)
    return 0 if ok else 1


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
    try:
        old = json.loads((d / "orq.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        old = None
    old = old if isinstance(old, dict) else None
    # A re-init that forgets --auto must not turn the run plain: the orchestrator would stop.
    auto = a.auto or (old or {}).get("auto") is True
    if auto:
        if not a.plan:
            raise OrqError("--auto needs --plan: the orchestrator reads the Tasks from it")
        cfg.update(auto=True, jev=a.jev or (old or {}).get("jev") or AUTO_JEV_DEFAULT,
                   regex=a.regex or (old or {}).get("regex") or "shadow")
    elif a.jev or a.regex:
        raise OrqError("--jev and --regex only apply with --auto")
    if a.plan:
        plan = Path(a.plan).expanduser().resolve()
        ptext = plan_text(plan)
        m = PREPARADO.search(ptext)
        if not m:
            raise OrqError("plan not prepared: run the preparar-plano agent, which ends with "
                           "`orq plan-check <plan> --repo <repo> --stamp`")
        if m.group(1) != plan_sha(ptext):
            raise OrqError("plan changed after preparation: run `orq plan-check <plan> --repo <repo> "
                           "--stamp` again")
        cfg["plan"] = str(plan)
    else:
        # Only a run started before plans were stamped re-inits without one, and stays plan-less.
        if old is None or "plan" in old:
            raise OrqError("plan required: pass --plan <stamped orchestration plan>; only a run "
                           "started without a plan re-inits without one")
    if old and old.get("arbiter") == a.arbiter and old.get("arbiter_identity"):
        # Reconfigurar a execução não associa uma sessão recriada com o mesmo nome.
        cfg["arbiter_identity"] = old["arbiter_identity"]
    elif old and old.get("arbiter") == a.arbiter:
        journal_append(d, f"session identity unavailable: {a.arbiter}: previous init has no recorded identity")
    elif identity := capture_identities(d, (a.arbiter,)).get(a.arbiter):
        cfg["arbiter_identity"] = identity
    (d / "orq.json").write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")
    snapshot_plan(d, cfg.get("plan"))
    journal_append(d, f"orq init: arbiter={a.arbiter} repo={cfg['repo']}"
                      + (f" auto jev={cfg['jev']} regex={cfg['regex']}" if auto else ""))
    if auto:
        _record_session(d, a.arbiter, "arbitro", None)
    print("ok")
    return 0


def cmd_plan_check(a) -> int:
    path = Path(a.plan).expanduser().resolve()
    text = plan_text(path)
    repo = Path(a.repo).expanduser().resolve()
    problems: list[str] = []
    pj = projeto(text)
    if not _section(text, "Projeto"):
        problems.append("missing section: ## Projeto")
    for key, label in PROJETO_KEYS.items():
        if pj[key] is None:
            problems.append(f"## Projeto: missing line '{label}:'")
    if pj["prova"] and pj["prova"][0] == "?":
        problems.append("## Projeto: Prova must be nenhuma | por-task | manual | lote(N)")
    if pj["paralelo"] == 0:
        problems.append("## Projeto: Paralelo must be sequencial | até N")
    if pj["revisao"] == "?":
        problems.append("## Projeto: Revisão must be subagente | sessão")
    # A command typed without backticks parses to no command: it would disable the check silently.
    for line in _section(text, "Projeto"):
        k, sep, v = line.partition(":")
        label = k.strip()
        if (sep and label.lower() in ("checagens", "integração") and v.strip() not in ("—", "-")
                and not re.search(r"`[^`]+`", v)):
            problems.append(f"## Projeto: {label} has no `command`")
    tasks = plan_tasks(text)
    if not tasks:
        problems.append("missing table: ## Tasks with columns #, Files, Verification, Wave, Roteiro")
    waves: dict[str, int] = {}
    for t in tasks:
        n = t["n"]
        if not t["files"]:
            problems.append(f"T{n}: no Files")
        for f in t["files"]:
            if not (repo / f).exists() and not (repo / f).parent.is_dir():
                problems.append(f"T{n}: neither the file nor its directory exists: {f}")
        if not t["verification"]:
            problems.append(f"T{n}: no Verification")
        if not t["wave"].isdigit():
            problems.append(f"T{n}: Wave is not a number: {t['wave'] or '(empty)'}")
        else:
            waves[t["wave"]] = waves.get(t["wave"], 0) + 1
        if t["roteiro"]:
            if pj["prova"] and pj["prova"][0] == "nenhuma":
                problems.append(f"T{n}: roteiro given but Prova: nenhuma")
            elif not (path.parent / t["roteiro"]).exists() and not Path(t["roteiro"]).exists():
                problems.append(f"T{n}: roteiro not found: {t['roteiro']}")
    if pj["paralelo"]:
        for w, count in sorted(waves.items()):
            if count > pj["paralelo"]:
                problems.append(f"wave {w} has {count} Tasks, Paralelo allows {pj['paralelo']}")
    for i, c in enumerate(pj["checagens"] or [], 1):
        ok, why = run_checks(str(repo), [c], path.parent / f"plan-check-{i}.log")
        if not ok:
            problems.append(why)
    if problems:
        print("\n".join(problems))
        return 1
    print("plan-check ok")
    if a.stamp:
        prova = pj["prova"]
        prova_txt = f"lote({prova[1]})" if prova[0] == "lote" else prova[0]
        par = "sequencial" if pj["paralelo"] == 1 else str(pj["paralelo"])
        body = PREPARADO.sub("", text)
        body = re.sub(r"\n{3,}", "\n\n", body)
        lines = body.splitlines(keepends=True)
        stamp = (f"Preparado: {datetime.now().date().isoformat()} · paralelo {par} · prova {prova_txt}"
                 f" · plan-check limpo · sha {plan_sha(body)}\n")
        lines.insert(1, stamp)
        path.write_text("".join(lines), encoding="utf-8")
        print(stamp.strip())
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
    if ev.get("tipo") == "espera":
        st = state(d)
        # Evento de Task após execucao_fim indica retomada; espera não reabre a execução.
        if st["ended"]:
            raise OrqError("the run has ended (execucao_fim): no wait to record")
        if "task" in ev and ev["task"] not in st["roles"]:
            raise OrqError(f"Task {ev['task']} has no task_inicio: a wait names a Task that started")
        # roles mantém Tasks fechadas; sua espera esconderia a Task aberta da mesma sessão.
        if "task" in ev and ev["task"] not in st["open"]:
            raise OrqError(f"Task {ev['task']} is not open: a wait names a started Task not yet closed")
        if "task" in ev:
            r = st["roles"][ev["task"]]
            known = {st["arbiter"], r.get("executor"), r.get("par")} - {None, SUBAGENT}
            if ev.get("sessao") not in known:
                raise OrqError(f"{ev.get('sessao')} is neither the arbiter nor an executor or reviewer "
                               f"of Task {ev['task']}")
        known = {st["arbiter"]} | {n for r in st["roles"].values() for n in r.values() if n and n != SUBAGENT}
        if ev.get("sessao") not in known:
            raise OrqError(f"{ev.get('sessao')} is neither the arbiter nor a session of this run")
        if not _until_future(ev.get("ate")):
            raise OrqError("--ate must be a future ISO-8601 time with offset (date -Iseconds -d '+30 min')")
    if ev.get("tipo") == "entrega" and ev.get("fase") != "prova" and plan_of(d).get("checagens"):
        if not ev.get("commit"):
            raise OrqError("the plan declares checks: deliver with `--commit <stash>`, the object "
                           f"`orq check --task {ev.get('task')}` passed on")
        if not check_ok(d, ev.get("task"), ev["commit"]):
            raise OrqError(f"run `orq check --task {ev.get('task')} --commit {ev['commit']}` "
                           "first: the plan declares checks and none passed on this object")
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
    if ev.get("tipo") == "veredito" and ev.get("resultado") == "corrige":
        ev["patch"] = str(Path(ev.get("patch") or "").expanduser().resolve()) if ev.get("patch") else ""
        _check_patch(d, ev)
    if ev.get("tipo") == "veredito" and state(d)["roles"].get(ev.get("task"), {}).get("par") == SUBAGENT:
        obj = round_object(d, ev["task"], ev.get("rodada"))
        repo = a.repo or config(d)["repo"]
        if obj and subprocess.run(["git", "-C", repo, "diff", "--quiet", obj, "--"],
                                  capture_output=True).returncode != 0:
            raise OrqError(f"worktree changed during the review of round {ev['rodada']}: a reviewer "
                           "never edits code; restore the round and judge again")
    ev = event_append(d, ev)
    journal_append(d, _event_line(ev))
    if ev["tipo"] == "entrega" and ev.get("fase") != "prova":
        commit = (ev.get("commit") or "")[:7]
        timeline(d, "advance", f"T{ev.get('task')} entregou a rodada {ev.get('rodada')}"
                 + (f" · {commit}" if commit else ""), ev.get("task"))
    if ev["tipo"] == "sessao_trocada" and ev.get("para"):
        _record_session(d, ev["para"], ev.get("papel"), ev.get("task"))
    # Before the notices: a failed send must not cost the orchestrator its trigger.
    if ev["tipo"] in ("entrega", "veredito") and config(d).get("auto"):
        if err := spawn_advance(d):
            _journal_or_warn(d, f"{ev['tipo']} T{ev.get('task')}: orq advance not started: {err}")
            _advance_not_started(d, f"{ev['tipo']} T{ev.get('task')}", ev.get("task"), err)
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
        own = sections.get(a.task, "")
        if too_big:
            # Never empty: a session without its contract works blind and says nothing.
            print(f"WARNING: {too_big}")
        print(common.rstrip())
        print("\n" + own.rstrip() if own else f"\n(no '## Task {a.task}' section in the contract)")
        return 3 if too_big else 0
    j = d / "registro.md"
    lines = j.read_text(encoding="utf-8").splitlines() if j.exists() else []
    pat = re.compile(rf"\bT{a.task}\b|\bTask {a.task}\b") if a.task is not None else None
    start = max(0, len(lines) - a.last)
    print("\n".join(l for i, l in enumerate(lines) if i >= start or (pat and pat.search(l))))
    return 0


def cmd_ball(a) -> int:
    st = state(base_dir(a.dir))
    if a.coverage:
        # all dispensa também o alarme da trilha; owners dispensa só o alarme coletivo.
        owners_covered = all(o in st["waiting"] for o in st["owners"])
        print("all" if owners_covered and st["arbiter"] in st["waiting"]
              else "owners" if owners_covered and st["owners"] else "none")
        return 0
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


def _lock_path(d: Path, resource: str) -> Path:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", resource):
        raise OrqError(f"resource name must be letters, digits, '.', '_' or '-': {resource!r}")
    return d / ("screen.lock" if resource == "screen" else f"lock-{resource}.lock")


def cmd_lock(a) -> int:
    d = base_dir(a.dir)
    lock = _lock_path(d, a.resource)
    if a.action == "release":
        held = _lock_owner(lock)
        if held != a.owner:
            print(f"not yours: held by {held or 'nobody'}")
            return 1
        lock.unlink()
        journal_append(d, f"{a.resource} released by {a.owner}")
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
            if age > LOCK_STALE_S:
                lock.unlink(missing_ok=True)
                journal_append(d, f"{a.resource} lock of {held} stale ({int(age // 60)} min), taken by {a.owner}")
                continue
            if time.time() >= deadline:
                print(f"held by {held} for {int(age // 60)} min")
                return 1
            time.sleep(min(5.0, max(0.1, deadline - time.time())))
            continue
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(a.owner)
        journal_append(d, f"{a.resource} taken by {a.owner}")
        print("taken")
        return 0


MARK = re.compile(r"^\s*\[(aviso|decis[aã]o)\]", re.IGNORECASE)


def _touch(p: Path | None) -> None:
    if p is None:
        return
    try:
        os.utime(p)
    except OSError:
        pass   # a heartbeat, not a step: the vigia alarms late at worst


def _run_bounded(cmd: list[str], timeout: float) -> subprocess.CompletedProcess:
    """subprocess.run with the whole process group killed on timeout: a hook's child keeping the
    pipe open would otherwise hold run() past its timeout forever."""
    with subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          start_new_session=True) as p:
        try:
            out, err = p.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass   # the group ended between the timeout and the kill
            try:
                p.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                # A process outside the group still holds the pipe: drop it, never wait forever.
                p.stdout.close()
                p.stderr.close()
            raise
    return subprocess.CompletedProcess(cmd, p.returncode, out, err)


def git(repo: str, *args: str, timeout: float | None = None) -> str:
    # Unquoted paths: an escaped accented name would slip past the untouchables glob.
    cmd = ["git", "-c", "core.quotePath=false", "-C", repo, *args]
    try:
        r = _run_bounded(cmd, timeout) if timeout else subprocess.run(cmd, capture_output=True, text=True)
    except subprocess.TimeoutExpired:
        raise OrqError(f"git {' '.join(args)}: timed out after {timeout}s, killed") from None
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


def round_object(d: Path, task: int, rnd: int) -> str | None:
    return next((ev.get("commit") for ev in reversed(events(d)) if ev.get("tipo") == "entrega"
                 and ev.get("task") == task and ev.get("rodada") == rnd), None)


def _check_patch(d: Path, ev: dict) -> None:
    """A reviewer's patch is small and stays inside the round, or it is an ordinary rejection."""
    limit = plan_of(d).get("correcao") or 0
    if not limit:
        raise OrqError("corrige is off in the plan (Correção pelo revisor: até 0 linhas): "
                       "reject the round as usual")
    task, rnd = ev.get("task"), ev.get("rodada")
    if any(x.get("tipo") == "veredito" and x.get("task") == task and x.get("resultado") == "corrige"
           for x in events(d)):
        raise OrqError("one corrige per Task: reject the round as usual")
    patch = ev.get("patch") or ""
    if not patch or not Path(patch).is_file():
        raise OrqError(f"corrige needs --patch <existing file>, got {patch or 'none'}")
    obj = round_object(d, task, rnd)
    if not obj:
        raise OrqError(f"round {rnd} of Task {task} was never delivered")
    repo = config(d)["repo"]
    stat = subprocess.run(["git", "-C", repo, "apply", "--numstat", str(Path(patch).resolve())],
                          capture_output=True, text=True)
    if stat.returncode != 0:
        raise OrqError(f"patch unreadable: {stat.stderr.strip()[:200]}")
    rows = [l.split("\t") for l in stat.stdout.splitlines() if l.count("\t") >= 2]
    inside = set(git(repo, "diff", "--name-only", "--no-renames", f"{obj}^1", f"{obj}^2").splitlines())
    outside = sorted({p for _, _, p in rows} - inside)
    if outside:
        raise OrqError(f"patch touches files outside the round: {outside}")
    # A binary change reports "-" for both counts, so the line limit cannot measure it.
    if not all(a.isdigit() and b.isdigit() for a, b, _ in rows):
        raise OrqError("corrige does not take binary changes: reject the round as usual")
    total = sum(int(a) + int(b) for a, b, _ in rows)
    if total > limit:
        raise OrqError(f"patch changes {total} lines, the plan's limit is {limit}: reject the round as usual")


def cmd_apply_patch(a) -> int:
    """Round R + the reviewer's patch becomes round R+1, frozen and checked, back to the reviewer."""
    d = base_dir(a.dir)
    cfg = config(d)
    repo = a.repo or cfg["repo"]
    ver = next((ev for ev in reversed(events(d)) if ev.get("tipo") == "veredito"
                and ev.get("task") == a.task and ev.get("resultado") == "corrige"), None)
    if not ver:
        raise OrqError(f"no corrige for Task {a.task}")
    rnd, patch = ver["rodada"], ver["patch"]
    if round_object(d, a.task, rnd + 1):
        raise OrqError(f"round {rnd + 1} of Task {a.task} already delivered")
    obj = round_object(d, a.task, rnd)
    if subprocess.run(["git", "-C", repo, "diff", "--quiet", obj, "--"], capture_output=True).returncode != 0:
        raise OrqError(f"worktree differs from round {rnd} ({obj[:12]}): the patch was made for that round")
    ap = subprocess.run(["git", "-C", repo, "apply", "--index", patch], capture_output=True, text=True)
    if ap.returncode != 0:
        print(f"patch does not apply: {ap.stderr.strip()[:300]}; the patch is your recipe now: "
              f"fix it yourself and deliver round {rnd + 1} as usual.")
        return 1
    # Frozen before the checks: a check that writes files cannot leak into round R+1.
    h = git(repo, "stash", "create").strip()
    git(repo, "stash", "store", "-m", f"task-{a.task} round {rnd + 1} (reviewer patch)", h)
    log = d / "checks" / f"task{a.task}-r{rnd + 1}-patch.log"
    ok, why = run_checks(repo, plan_of(d).get("checagens") or [], log)
    if not ok:
        undo = subprocess.run(["git", "-C", repo, "apply", "-R", "--index", patch],
                              capture_output=True, text=True)
        if undo.returncode != 0:
            print(f"{why}. Worktree NOT restored to round {rnd}: {undo.stderr.strip()[:300]}")
            return 1
        print(f"{why}. Worktree back to round {rnd}; the patch is your recipe now: fix it yourself "
              f"and deliver round {rnd + 1} as usual.")
        return 1
    record_check(d, a.task, h, True, str(log))
    ev = {"tipo": "entrega", "task": a.task, "rodada": rnd + 1, "commit": h,
          "motivo": f"round {rnd} + reviewer patch {patch}"}
    # The patch changed code: a proof round goes back to code review and the proof runs again.
    ent = next((x for x in reversed(events(d)) if x.get("tipo") == "entrega"
                and x.get("task") == a.task and x.get("rodada") == rnd), {})
    if ent.get("fase"):
        ev["fase"] = "codigo"
    ev = event_append(d, ev)
    journal_append(d, _event_line(ev))
    par = state(d)["roles"].get(a.task, {}).get("par")
    if par == SUBAGENT:
        print(f"round {rnd + 1} frozen: {h}. Now run `orq review-package --task {a.task} --rodada "
              f"{rnd + 1}` and dispatch a NEW revisor-orq with that path: it judges the patch fresh.")
        return 0
    send(par, f"Task {a.task} round {rnd + 1} = round {rnd} + your patch {patch}, checks ok, object "
              f"{h}. Review the patch with a clean-context subagent (revisor.md, \"Round of your own "
              f"patch\") and give the verdict of round {rnd + 1}.")
    print(f"round {rnd + 1} frozen: {h}")
    return 0


def _jsonl(p: Path) -> list[dict]:
    if not p.exists():
        return []
    out = []
    for line in p.read_text(encoding="utf-8").splitlines():
        try:
            v = json.loads(line)
        except ValueError:
            continue
        if isinstance(v, dict):
            out.append(v)
    return out


def cmd_review_package(a) -> int:
    """Everything a subagent reviewer sees, built by orq: the executor cannot choose it."""
    d = base_dir(a.dir)
    cfg = config(d)
    repo = a.repo or cfg["repo"]
    obj = round_object(d, a.task, a.rodada)
    if not obj:
        raise OrqError(f"round {a.rodada} of Task {a.task} was never delivered")
    try:
        common, sections = _contract_parts(Path(cfg["contract"]).read_text(encoding="utf-8"))
    except FileNotFoundError:
        common, sections = "(no contract)", {}
    plan = cfg.get("plan")
    ptext = plan_text(plan) if plan else ""
    row = next((t for t in plan_tasks(ptext) if t["n"] == a.task), None)
    # The raw row keeps "What it is" and "Where in their plan": the requirement the reviewer judges.
    table = [l for l in _section(ptext, "Tasks") if l.strip().startswith("|")]
    raw = next((l for l in table[2:] if l.strip().strip("|").split("|")[0].strip() == str(a.task)), None)
    users_plan = next((l for l in ptext.splitlines() if l.startswith("User's plan:")), "")
    plan_row = "\n".join(x for x in (users_plan, *table[:2], raw) if x) if raw else "(no plan row)"
    check = next((c for c in reversed(_jsonl(d / "checks.jsonl"))
                  if c.get("task") == a.task and c.get("commit", "-").startswith(obj[:12])), None)
    evs = events(d)
    ent = next((ev for ev in reversed(evs) if ev.get("tipo") == "entrega"
                and ev.get("task") == a.task and ev.get("rodada") == a.rodada), {})
    patch_of = ent.get("motivo") or ""
    fase = ent.get("fase")
    # A patch round's fresh reviewer checks these blockers; a repeated cause needs --reincide.
    earlier = [f"- round {ev.get('rodada')}: {ev.get('resultado')}, report {ev.get('motivo') or 'none'}"
               + (f", patch {ev['patch']}" if ev.get("patch") else "")
               for ev in evs if ev.get("tipo") == "veredito" and ev.get("task") == a.task
               and isinstance(ev.get("rodada"), int) and ev["rodada"] < a.rodada]
    # Plan-relative in the plan; the reviewer runs elsewhere and needs a path it can open.
    roteiro = (row or {}).get("roteiro")
    if roteiro:
        roteiro = str(Path(plan).expanduser().resolve().parent / roteiro)
    report = Path(a.report).read_text(encoding="utf-8") if a.report else "(no report given)"
    diff = git(repo, "diff", "--stat", f"{obj}^1", obj) + "\n" + git(repo, "diff", f"{obj}^1", obj)
    out = d / "review" / f"task{a.task}-r{a.rodada}.md"
    out.parent.mkdir(parents=True, exist_ok=True)
    verdict = (f"python3 {Path(__file__).resolve()} --dir {d} event veredito --task {a.task} "
               f"--rodada {a.rodada} --resultado <aprova|reprova|devolvido|corrige> --sessao revisor-orq "
               f"--motivo {d}/pareceres/task{a.task}-r{a.rodada}.md --repo {repo}"
               + (f" --fase {fase}" if fase else ""))
    out.write_text("\n".join([
        f"# Review package — Task {a.task}, round {a.rodada}",
        f"Object: {obj}  Base: {obj}^1  Repo: {repo}  Durable dir: {d}",
        f"Phase: {fase or 'none'}",
        f"This round is {patch_of}." if "reviewer patch" in patch_of else "",
        "## Contract", common.rstrip(), sections.get(a.task, f"(no '## Task {a.task}' section)").rstrip(),
        "## Plan row", plan_row,
        "## Roteiro", roteiro or "none",
        "## Check log", (check or {}).get("log") or "none",
        "## Earlier verdicts", "\n".join(earlier) or "none",
        "## Round report", report.rstrip(),
        "## Diff", "```diff", diff.rstrip(), "```",
        "## How to answer",
        f"Write the report to {d}/pareceres/task{a.task}-r{a.rodada}.md "
        f"(and the patch, for corrige, to {d}/pareceres/task{a.task}-r{a.rodada}.patch, passing --patch). "
        f"Then run:\n{verdict}",
    ]) + "\n", encoding="utf-8")
    print(out)
    return 0


def pending_proofs(d: Path) -> list[dict]:
    taken = {t for b in _jsonl(d / "prova-lotes.jsonl") for t in b.get("tasks", [])}
    return [f for f in _jsonl(d / "prova-fila.jsonl") if f.get("task") not in taken]


def _queue_proof(d: Path, task: int, full: str) -> str:
    """lote/manual: the committed Task's proof waits in line; the text to add to the close notice."""
    pj = plan_of(d)
    prova = pj.get("prova")
    if not prova or prova[0] not in ("lote", "manual"):
        return ""
    tasks = plan_tasks(plan_text(config(d)["plan"]))
    me = next((t for t in tasks if t["n"] == task), None)
    fila = d / "prova-fila.jsonl"
    queued = {f.get("task") for f in _jsonl(fila)}
    # A repeated `orq commit` must not count the same Task twice toward N.
    if me and me["roteiro"] and task not in queued:
        with fila.open("a", encoding="utf-8") as f:
            f.write(json.dumps({"ts": now(), "task": task, "roteiro": me["roteiro"], "hash": full}) + "\n")
    # Manual: the arbiter takes the queue at the end, for the user's own test; nothing to announce.
    if prova[0] == "manual":
        return ""
    pend = pending_proofs(d)
    if not pend:
        return ""
    closed = _closed(d)
    # By wave, not by open Tasks: a sequential run has none open at every commit. A Task without
    # roteiro may still be the one that ends the wave.
    wave_over = not me or all(t["n"] in closed for t in tasks if t["wave"] == me["wave"])
    if len(pend) >= prova[1] or wave_over:
        names = ", ".join(f"T{p['task']}" for p in pend)
        return (f" Proof batch ready: {names}. Run `orq batch take` and open one proof session "
                "for those roteiros on the integrated code.")
    if not (me and me["roteiro"]):
        return ""
    return f" Proof queued ({len(pend)}/{prova[1]})."


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
    # closed.jsonl is already written: an unreadable plan must not swallow the close notice.
    queue_err = None
    try:
        extra = _queue_proof(d, a.task, full)
    except OrqError as e:
        extra, queue_err = f" (proof queue skipped: {e})", str(e)
    if cfg.get("auto"):
        # The orchestrator integrates and releases the next Task; the arbiter wakes only if it cannot.
        if queue_err:
            _wake_or_journal(d, f"[decisao] Task {a.task} closed, but its proof was NOT queued "
                                f"({queue_err}): no batch will carry it. Run it by hand or take it "
                                "to the user.",
                             f"falhou ao enfileirar a prova da T{a.task}: {queue_err[:200]}", a.task)
        err = spawn_advance(d)
        journal_append(d, f"commit T{a.task}: orq advance "
                          + (f"not started ({err})" if err else "started") + extra)
        if err:
            _advance_not_started(d, f"commit T{a.task}", a.task, err)
        print("ok")
        return 0
    send(state(d)["arbiter"], f"[decisao] Task {a.task} closed and checked: {full[:12]}, "
                              f"{len(files)} file(s), tip = hash, matches the approved round. "
                              "Release the next ready Task(s)." + extra)
    print("ok")
    return 0


def take_batch(d: Path, dry: bool = False, pend: list[dict] | None = None) -> str | None:
    """The pending proofs (or `pend`) become the next batch; its line, or None when nothing is
    pending. `dry`: the line only, nothing recorded."""
    pend = pending_proofs(d) if pend is None else pend
    if not pend:
        return None
    # Plan-relative in the plan; the proof session runs elsewhere and needs a path it can open.
    base = Path(config(d)["plan"]).expanduser().resolve().parent
    n = len(_jsonl(d / "prova-lotes.jsonl")) + 1
    if not dry:
        with (d / "prova-lotes.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps({"ts": now(), "lote": n, "tasks": [p["task"] for p in pend]}) + "\n")
        journal_append(d, f"proof batch {n} taken: " + " ".join(f"T{p['task']}" for p in pend))
    return f"lote {n}: " + " ".join(f"T{p['task']} {base / p['roteiro']} {p['hash'][:12]}" for p in pend)


def cmd_batch(a) -> int:
    print(take_batch(base_dir(a.dir)) or "no pending proof")
    return 0


def _json_object(p: Path) -> dict:
    try:
        v = json.loads(p.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return v if isinstance(v, dict) else {}


def jev_config(auto: bool = False) -> dict:
    """The Jev's key, url and model. The environment wins; an auto run then reads the server's
    runtime-config.json, where the Hangar settings screen writes them, and an OpenRouter key alone
    implies its endpoint. A plain run keeps the environment-only lookup: orquestrar stays as it was."""
    rc = _json_object(Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude")
                      / "runtime-config.json") if auto else {}
    env = _json_object(Path.home() / ".claude" / "settings.json").get("env")
    legacy = env.get("TYPESAFE_API_KEY") if isinstance(env, dict) else None

    def first(*vals) -> str:
        return next((v.strip() for v in vals if isinstance(v, str) and v.strip()), "")

    key = first(os.environ.get("TYPESAFE_API_KEY"), rc.get("jev_api_key"), legacy)
    url = first(os.environ.get("ORQ_JEV_URL"), os.environ.get("JEV_ENDPOINT"), rc.get("jev_endpoint"))
    model = first(os.environ.get("JEV_MODEL"), rc.get("jev_model"))
    if auto and not url and key.startswith("sk-or-"):
        url, model = OPENROUTER_JEV_URL, model or OPENROUTER_JEV_MODEL
    return {"key": key, "url": url or JEV_URL, "model": model or JEV_MODEL}


def jev_ask(text: str, auto: bool = False) -> dict:
    """{'choice', 'p', 'veto', 'probs'} or {'error'}; never raises — an error never lets the Jev drop a message."""
    c = jev_config(auto)
    if not c["key"]:
        return {"error": "no key"}
    body = json.dumps({"model": c["model"], "state": text[-20_000:], "questions": JEV_QUESTIONS}).encode()
    req = urllib.request.Request(c["url"], data=body,
                                 headers={"authorization": f"Bearer {c['key']}",
                                          "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=JEV_TIMEOUT_S) as r:
            answers = json.load(r)["answers"]
        choice = answers["kind"].get("choice")
        # Only a "nothing" winner can drop; its probability, not `confidence`.
        p = (answers["kind"].get("probabilities") or {}).get("nothing") if choice == "nothing" else 0.0
        veto = {k: float(answers[k]["noul"]) for k in JEV_VETOES}
        probs = {str(k): float(v) for k, v in (answers["kind"].get("probabilities") or {}).items()
                 if isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)}
    except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError, KeyError,
            TypeError, AttributeError) as e:
        return {"error": f"{type(e).__name__}: {str(e)[:120]}"}
    return {"choice": choice, "p": float(p) if isinstance(p, (int, float)) else 0.0, "veto": veto,
            "probs": probs}


def triage(d: Path, text: str, alarm: bool) -> str:
    """Unmarked message: 'drop' only in mode `on`, when the Jev is sure it asks nothing and no veto
    fires. Shadow (default) asks and records, and the arbiter wakes the same. Every real alarm
    needs action, so alarms wake without asking."""
    mode = os.environ.get("ORQ_JEV", "shadow")
    if mode == "off" or alarm:
        return "wake"
    r = _jev_record(d, text, mode)
    return "drop" if mode == "on" and r["would_drop"] else "wake"


def _jev_record(d: Path, text: str, mode: str, auto: bool = False) -> dict:
    """Asks the Jev and appends the answer to jev-shadow.jsonl; the answer gains `would_drop`."""
    r = jev_ask(text, auto)
    r["would_drop"] = ("error" not in r and r["choice"] == "nothing" and r["p"] >= DISCARD_P
                       and all(v <= VETO_P for v in r["veto"].values()))  # NaN never drops
    try:
        with (d / "jev-shadow.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps({"ts": now(), "mode": mode, "alarm": False, "text": text[:500], **r},
                               ensure_ascii=False) + "\n")
    except OSError as e:
        # Losing the shadow record never costs the arbiter the message.
        print(f"orq: jev-shadow.jsonl not written: {e}", file=sys.stderr)
    return r


def _regex():
    spec = importlib.util.spec_from_file_location("orq_triage", HERE / "orq_triage.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def triage_auto(d: Path, cfg: dict, text: str) -> tuple[str, str, str]:
    """Auto runs: the Jev decides when it has a key and answers; without a key or on an error the
    measured regex does. Returns (verdict, source, why): verdict "drop", "would_drop" (the mode
    only records) or "wake"; source "jev" or "regex"; why names the regex category."""
    if os.environ.get("ORQ_JEV") != "off" and jev_config(auto=True)["key"]:
        mode = cfg.get("jev", "shadow")
        r = _jev_record(d, text, mode, auto=True)
        if "error" not in r:
            if not r["would_drop"]:
                return "wake", "jev", "jev"
            return ("drop" if mode == "on" else "would_drop"), "jev", "jev"
    verdict, category = _regex().decide(text)
    if verdict == "wake":
        return "wake", "regex", "regex"
    return ("drop" if cfg.get("regex", "shadow") == "on" else "would_drop"), "regex", f"regex: {category}"


def cmd_notify(a) -> int:
    d = base_dir(a.dir)
    m = MARK.match(a.text)
    if m and m.group(1).lower() == "aviso":
        journal_append(d, f"aviso: {a.text}")
        print("journal")
        return 0
    who = _sender(a.alarm) if is_auto(d) else None
    kind, line = "woke", f"acordou o árbitro: {a.text}"
    if not m and not a.alarm and is_auto(d):
        verdict, source, why = triage_auto(d, config(d), a.text)
        if verdict == "drop":
            journal_append(d, f"({source}: no action) {a.text}")
            timeline(d, "dropped", f"recado registrado sem acordar o árbitro ({why}): {a.text}",
                     notify=True, sender=who)
            print(f"journal ({source})")
            return 0
        if verdict == "would_drop":
            kind, line = "would_drop", f"teria descartado ({why}); acordou o árbitro: {a.text}"
    elif not m and triage(d, a.text, a.alarm) == "drop":
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
        timeline(d, "failed", f"recado ao árbitro não entregue ({e}): {a.text}", notify=True, sender=who)
        raise
    timeline(d, kind, line, notify=True, sender=who)
    print("arbiter woken")
    return 0


def cmd_log(a) -> int:
    d = base_dir(a.dir)
    journal_append(d, (f"T{a.task} " if a.task is not None else "") + a.text)
    print("ok")
    return 0


# ---- orq advance: the orchestrator of an `auto` run -------------------------------------------

JEV_PICK_P = 0.85  # below it the rule decides; not calibrated yet, like the triage thresholds were
JEV_RED = {"red": {"type": "choice", "instructions": (
    "A software team merged a finished Task into its main line and ran the integration commands. "
    "They failed; the failing command and the end of its output follow. What should happen?"),
    "criteria": {
        "back": "the failure comes from the merged change: the Task goes back to its executor to fix",
        "retry": ("the failure is transient and unrelated to the change (network, timeout, a busy "
                  "port or device): run the same commands once more"),
        "wake": "it needs a person: the environment is broken, or it is unclear whose failure it is",
        "none": "none of these",
    }}}
RED_PT = {"back": "devolver à Task", "retry": "rodar de novo", "wake": "acordar o árbitro"}
PASSO_PT = {"integrate": "integrar", "open": "abrir", "batch": "anunciar o lote de prova",
            "final": "pedir a revisão final", "advance": "rodar a passada"}


def _at_or_after(ts, ref) -> bool:
    """`ts` at or after `ref`; ts has second precision, so the same second counts. A ref missing
    or that does not parse lets every event count."""
    if ref is None:
        return True
    try:
        return datetime.fromisoformat(ts) >= datetime.fromisoformat(ref)
    except (TypeError, ValueError):
        return True


def _closes(d: Path) -> dict[int, dict]:
    """Task → its latest closed.jsonl line, ordered by when it closed: the merge order."""
    out: dict[int, dict] = {}
    for c in _jsonl(d / "closed.jsonl"):
        if isinstance(c.get("task"), int) and c.get("hash"):
            out.pop(c["task"], None)
            out[c["task"]] = c
    return out


def _outcome(evs: list[dict], n: int, close: dict) -> str | None:
    """How integrating this close ended, None while not tried. A new close (the fix of a red
    integration, or a correction after a conflict) starts over."""
    out = None
    for ev in evs:
        if ev.get("task") != n or not _at_or_after(ev.get("ts"), close.get("ts")):
            continue
        t = ev.get("tipo")
        if t in ("integrada", "conflito", "integracao_vermelha") or (
                t == "advance_falhou" and ev.get("passo") == "integrate"):
            out = t
    return out


def _integrated(d: Path, evs: list[dict]) -> set[int]:
    return {n for n, c in _closes(d).items() if _outcome(evs, n, c) == "integrada"}


def _failed_since(evs: list[dict], passo: str, task: int | None, since) -> bool:
    return any(ev.get("tipo") == "advance_falhou" and ev.get("passo") == passo
               and ev.get("task") == task and _at_or_after(ev.get("ts"), since) for ev in evs)


def additive_files(text: str) -> list[str]:
    """The optional `Aditivos:` line of `## Projeto`: globs of files where Tasks insert at declared
    anchors, whose positional conflicts the orchestrator resolves by union."""
    for line in _section(text, "Projeto"):
        k, sep, v = line.partition(":")
        if sep and k.strip().lower() == "aditivos":
            return re.findall(r"`([^`]+)`", v)
    return []


def orchestrator_tag(d: Path) -> str:
    """The panel's notice form: nobody answers it as if it were a session."""
    return f"[painel: orquestrador {_gid(d)}] "


def _gid(d: Path) -> str:
    return _run_start(d).get("gid") or d.name


def _run_start(d: Path) -> dict:
    return next((ev for ev in reversed(events(d)) if ev.get("tipo") == "execucao_inicio"), {})


def _notice_once(d: Path, text: str, task: int | None = None) -> None:
    """A wait the orchestrator keeps passing over: one timeline line per episode, not per pass."""
    last = (_jsonl(d / f"timeline-{d.resolve().name}.jsonl") or [{}])[-1]
    if last.get("text") != text:
        timeline(d, "notice", text, task)


def _off_branch(d: Path, repo: str) -> str | None:
    """Why the main checkout is not where the run merges and opens, in pt-BR; None = it is. The
    user may switch the checkout mid-run, and a merge would land on whatever is checked out."""
    want = _run_start(d).get("branch")
    cur = git(repo, "branch", "--show-current").strip()
    if cur and (not want or cur == want):
        return None
    where = f"na branch {cur}" if cur else "em HEAD destacado"
    return (f"o checkout principal está {where}, não na {want or 'branch'} da execução; integrar "
            "e abrir Tasks esperam ele voltar")


def _say(d: Path, target: str, text: str) -> None:
    send(target, orchestrator_tag(d) + text, painel=True)


def _wake(d: Path, text: str, line: str, task: int | None = None, kind: str = "woke") -> None:
    """The arbiter, from the orchestrator. Journal and timeline first: a failed send still leaves
    the trail."""
    journal_append(d, f"orchestrator → arbiter: {text}")
    timeline(d, kind, line, task)
    _say(d, state(d)["arbiter"], text)


def _fail(d: Path, passo: str, task: int | None, err: str) -> str:
    """A step that broke: recorded and the arbiter woken, once. Callers skip a (step, Task) that
    already failed, so it never repeats in a loop."""
    ev = {"tipo": "advance_falhou", "passo": passo, "motivo": err[:1200]}
    if task is not None:
        ev["task"] = task
    event_append(d, ev)
    where = passo + (f" T{task}" if task is not None else "")
    try:
        _wake(d, f"[decisao] orq advance failed at {where}: {err[:1200]}. It will not retry this "
                 "step: do it by hand or take it to the user.",
              f"falhou ao {PASSO_PT[passo]}" + (f" a T{task}" if task is not None else "")
              + f": {err[:200]}", task, kind="failed")
    except OrqError as e:
        journal_append(d, f"advance: arbiter not woken about {where}: {e}")
    return f"failed {where}: {err}"


def jev_pick(text: str, questions: dict) -> str | None:
    """The winner of the one choice question in `questions`, when sure; None on doubt, `none`, no
    key or any failure — the caller's rule decides then."""
    jc = jev_config(auto=True)
    if not jc.get("key"):
        return None
    (qname,) = questions
    body = json.dumps({"model": jc.get("model"), "state": text[-20_000:],
                       "questions": questions}).encode()
    req = urllib.request.Request(jc.get("url") or JEV_URL, data=body,
                                 headers={"authorization": f"Bearer {jc['key']}",
                                          "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=JEV_TIMEOUT_S) as r:
            ans = json.load(r)["answers"][qname]
        choice = ans.get("choice")
        p = float((ans.get("probabilities") or {}).get(choice, 0.0))
    except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError, KeyError,
            TypeError, AttributeError):
        return None
    ok = choice in questions[qname]["criteria"] and choice != "none" and p >= JEV_PICK_P
    return choice if ok else None


def _red_choice(d: Path, cfg: dict, n: int, why: str, log: Path) -> str:
    """The code builds the options and the Jev picks. No key, an error, doubt or shadow mode →
    the rule: back to the Task, never run again."""
    if not jev_config(auto=True).get("key"):
        return "back"
    try:
        tail = log.read_text(encoding="utf-8", errors="replace")[-8000:]
    except OSError:
        tail = ""
    pick = jev_pick(f"{why}\n\n{tail}", JEV_RED)
    if cfg.get("jev") == "on":
        return pick or "back"
    if pick:
        timeline(d, "notice", f"T{n}: o Jev escolheria \"{RED_PT[pick]}\"; só anotando, a regra "
                              "devolve à Task", n)
    return "back"


def _prove_additive(path: str, base: bytes, ours: bytes, theirs: bytes, merged: bytes) -> str | None:
    """A union is accepted only by content, against the base (empty when both sides added the
    file): a JSON object holds exactly the keys of both sides minus what either side deleted, each
    with its value; any other file holds exactly base + each side's changes, so an old line kept
    beside its edited version is refused."""
    if path.endswith(".json"):
        try:
            b = json.loads(base) if base.strip() else {}
            o, t, m = (json.loads(x) for x in (ours, theirs, merged))
        except ValueError as e:
            return f"{path}: the union is not valid JSON ({e})"
        if not all(isinstance(x, dict) for x in (b, o, t, m)):
            return f"{path}: not a JSON object"
        want = (set(o) | set(t)) - (set(b) - set(o)) - (set(b) - set(t))
        if set(m) != want:
            return f"{path}: keys {sorted(set(m) ^ want)} differ from both sides' changes"
        changed = sorted(k for k in m if (k in o and m[k] != o[k]) or (k in t and m[k] != t[k]))
        return f"{path}: values changed for {changed}" if changed else None
    b, o, t, m = (Counter(x.decode("utf-8", "replace").splitlines())
                  for x in (base, ours, theirs, merged))
    want = o + t - b
    if m != want:
        return (f"{path}: the union holds {sum((m - want).values())} line(s) too many and misses "
                f"{sum((want - m).values())}")
    return None


def _union_resolve(repo: str, path: str) -> str | None:
    """A positional conflict in a declared additive file: both sides kept (`git merge-file
    --union`), staged only when the content proves it. None = resolved; else why not."""
    sides = {}
    for stage in (1, 2, 3):
        r = subprocess.run(["git", "-C", repo, "show", f":{stage}:{path}"], capture_output=True)
        sides[stage] = r.stdout if r.returncode == 0 else None
    if sides[2] is None or sides[3] is None:
        return f"{path}: deleted on one side, no union"
    with tempfile.TemporaryDirectory() as tmp:
        names = []
        for stage in (2, 1, 3):  # current, base, other
            p = Path(tmp) / str(stage)
            p.write_bytes(sides[stage] or b"")  # no base: both sides added the file
            names.append(str(p))
        r = subprocess.run(["git", "merge-file", "-p", "--union", *names], capture_output=True)
    if r.returncode != 0:
        return f"{path}: git merge-file failed ({r.stderr.decode(errors='replace').strip()[:120]})"
    why = _prove_additive(path, sides[1] or b"", sides[2], sides[3], r.stdout)
    if why:
        return why
    (Path(repo) / path).write_bytes(r.stdout)
    git(repo, "add", "--", path)
    return None


def _journal_or_warn(d: Path, text: str) -> None:
    """A journal that cannot be written must not cost the caller its next step."""
    try:
        journal_append(d, text)
    except OSError as e:
        print(f"orq: warning: journal not written ({e}): {text}", file=sys.stderr)


def _wake_or_journal(d: Path, text: str, line: str, task: int | None = None) -> None:
    """A failure only the arbiter can act on; a send that fails too stays in the journal. Never
    raises: the caller still has its notice or its advance to run."""
    try:
        _wake(d, text, line, task, kind="failed")
    except (OrqError, OSError, ValueError, KeyError) as e:
        _journal_or_warn(d, f"arbiter not woken: {e}")


def _advance_not_started(d: Path, after: str, task: int | None, err: str) -> None:
    """Nobody else integrates or releases the next Task, so the arbiter hears of it."""
    _wake_or_journal(d, f"[decisao] After {after}: orq advance did not start ({err}). The watchdog's "
                        "next cycle retries it; if this repeats, run `orq advance` by hand.",
                     f"não consegui iniciar o orq advance depois de {after}: {err[:200]}", task)


def _abort_merge(repo: str) -> None:
    """The user's checkout never stays half-merged; an abort that fails is raised, never hidden."""
    if subprocess.run(["git", "-C", repo, "rev-parse", "-q", "--verify", "MERGE_HEAD"],
                      capture_output=True).returncode != 0:
        return
    r = subprocess.run(["git", "-C", repo, "merge", "--abort"], capture_output=True, text=True)
    if r.returncode != 0:
        raise OrqError(f"git merge --abort failed, the main line is mid-merge: "
                       f"{(r.stderr or r.stdout).strip()[:200]}")


def _undo_killed_merge(repo: str, before: str) -> None:
    """A merge killed before writing MERGE_HEAD can leave its files in the tree, where
    `merge --abort` does not reach."""
    if git(repo, "status", "--porcelain") == before:
        return
    r = subprocess.run(["git", "-C", repo, "reset", "--merge"], capture_output=True, text=True)
    if r.returncode != 0:
        left = subprocess.run(["git", "-C", repo, "status", "--porcelain"], capture_output=True,
                              text=True).stdout.strip()
        raise OrqError(f"git reset --merge failed after a killed merge "
                       f"({(r.stderr or r.stdout).strip()[:200]}); the main line is dirty: {left[:600]}")


def _merge(d: Path, repo: str, n: int, h: str) -> tuple[list[str], str] | None:
    """`git merge --no-ff` of the verified commit. None = merged (additive conflicts resolved);
    else the conflicting files and the reason, with the merge aborted."""
    before = git(repo, "status", "--porcelain")
    try:
        r = _run_bounded(["git", "-C", repo, "merge", "--no-ff", "-m", f"Merge Task {n} ({h[:12]})", h],
                         MERGE_TIMEOUT_S)
    except subprocess.TimeoutExpired:
        _abort_merge(repo)
        _undo_killed_merge(repo, before)
        raise OrqError(f"git merge {h[:12]}: timed out after {MERGE_TIMEOUT_S}s, killed and "
                       "aborted") from None
    if r.returncode == 0:
        return None
    merged = False
    try:
        files = git(repo, "diff", "--name-only", "--diff-filter=U").splitlines()
        if not files:  # not a conflict: a bad hash, or a hook that refused the merge commit
            raise OrqError(f"git merge {h[:12]}: {(r.stderr or r.stdout).strip()[:300]}")
        plan = config(d).get("plan")
        pats = additive_files(plan_text(plan)) if plan else []
        others = [f for f in files if not any(fnmatch.fnmatch(f, p) for p in pats)]
        whys = [] if others else [w for f in files if (w := _union_resolve(repo, f))]
        if others or whys:
            return others or files, "; ".join(whys)
        git(repo, "commit", "--no-edit", timeout=MERGE_TIMEOUT_S)
        merged = True
    finally:
        if not merged:
            _abort_merge(repo)
    journal_append(d, f"T{n}: additive conflict in {', '.join(files)} resolved by union, "
                      "proven by content")
    return None


def _collided_with(d: Path, files: list[str], n: int) -> int | None:
    """The latest integrated Task whose plan Files cover a conflicting file."""
    plan = config(d).get("plan")
    owned = {t["n"]: t["files"] for t in plan_tasks(plan_text(plan))} if plan else {}
    for ev in reversed(events(d)):
        t = ev.get("task")
        if ev.get("tipo") != "integrada" or t == n:
            continue
        if any(f == p or f.startswith(p.rstrip("/") + "/") for f in files for p in owned.get(t, [])):
            return t
    return None


def _back_to_executor(d: Path, repo: str, n: int, why: str, log: Path) -> None:
    """The page's rule for a red integration: the Task goes back to its executor, who fixes it on
    the main line; the reviewer judges before the new commit."""
    ini = next((ev for ev in reversed(events(d))
                if ev.get("tipo") == "task_inicio" and ev.get("task") == n), None)
    roles = state(d)["roles"].get(n, {})  # after swaps
    if not ini or not roles.get("executor"):
        raise OrqError(f"Task {n} has no task_inicio: executor unknown")
    close_ts = _closes(d)[n].get("ts")
    # A task_inicio in the close's own second would not reopen the Task (second precision).
    deadline = time.time() + 1.5
    while close_ts and now() <= close_ts and time.time() < deadline:
        time.sleep(0.2)
    ev = event_append(d, {"tipo": "task_inicio", "task": n, "titulo": ini.get("titulo") or f"Task {n}",
                          "executor": roles["executor"], "par": roles.get("par")})
    journal_append(d, _event_line(ev))
    _say(d, roles["executor"], f"[decisao] Integration red after Task {n} was merged: {why} (log "
                               f"{log}). Fix it on the main line, {repo}, not in a worktree: freeze "
                               "the round and deliver it to your reviewer as usual; the commit closes "
                               f"with `orq commit --task {n} --hash <hash>`, without --repo.")
    timeline(d, "advance", f"T{n}: integração vermelha → devolvida ao executor {roles['executor']}", n)


def _integrate(d: Path, cfg: dict, n: int, h: str, acts: list[str]) -> bool:
    """Merge when the commit is not on the main line yet (a worktree Task), then the plan's
    `Integração:`. True = green, or a conflict that left the main line untouched; False = the main
    line waits for someone."""
    repo = cfg["repo"]
    # Git merges over uncommitted changes it does not touch, and Integração: would test them too.
    if git(repo, "status", "--porcelain", "--untracked-files=no").strip():
        _notice_once(d, f"T{n}: a linha principal tem mudanças não commitadas; a integração espera "
                        "a árvore limpa", n)
        return False
    needs_merge = subprocess.run(["git", "-C", repo, "merge-base", "--is-ancestor", h, "HEAD"],
                                 capture_output=True).returncode != 0
    beat = d / "advance.lock"
    if needs_merge:
        _touch(beat)
        conflict = _merge(d, repo, n, h)
        if conflict:
            files, why = conflict
            other = _collided_with(d, files, n)
            en, pt, names = (f"T{other}" if other else "the main line",
                             f"T{other}" if other else "a linha principal", ", ".join(files))
            event_append(d, {"tipo": "conflito", "task": n,
                             "motivo": f"{names} with {en}" + (f" ({why})" if why else "")})
            acts.append(f"conflict T{n}: {names}")
            try:
                _wake(d, f"[decisao] T{n} conflicted with {en} in {names}" + (f" ({why})" if why else "")
                         + ". Merge aborted: the main line is as before.",
                      f"acordou o árbitro: T{n} conflitou com {pt} em {names}", n)
            except OrqError as e:
                journal_append(d, f"advance: arbiter not woken about the T{n} conflict: {e}")
            return True
    head = git(repo, "rev-parse", "HEAD").strip()
    cmds = plan_of(d).get("integracao") or []
    log = d / "checks" / f"integration-t{n}-{head[:12]}.log"
    ok, why = run_checks(repo, cmds, log, beat)
    choice = "back"
    if not ok:
        choice = _red_choice(d, cfg, n, why, log)
        if choice == "retry":
            timeline(d, "advance", f"T{n}: integração vermelha, rodando de novo uma vez", n)
            log = log.with_name(log.stem + "-retry.log")
            ok, why = run_checks(repo, cmds, log, beat)
            choice = "back"  # a second red never runs again
    if ok:
        event_append(d, {"tipo": "integrada", "task": n, "commit": head})
        journal_append(d, f"integrated T{n} at {head[:12]}" + (" (merge)" if needs_merge else ""))
        timeline(d, "advance", f"T{n} fechada → " + ("merge → " if needs_merge else "") + "integração verde", n)
        acts.append(f"integrated T{n} {head[:12]}")
        return True
    event_append(d, {"tipo": "integracao_vermelha", "task": n, "motivo": str(log)})
    journal_append(d, f"integration red after T{n}: {why}")
    if choice == "wake":
        _wake(d, f"[decisao] Integration red after T{n}: {why}. The main line keeps the merge; "
                 "decide: back to its executor, run again, or something else.",
              f"acordou o árbitro: integração vermelha depois da T{n} (o Jev pediu)", n)
        acts.append(f"red T{n}: arbiter woken")
        return False
    _back_to_executor(d, repo, n, why, log)
    acts.append(f"red T{n}: back to its executor")
    return False


def _integrate_all(d: Path, cfg: dict, acts: list[str]) -> bool:
    """Every closed Task not integrated yet, in closing order. False = the main line waits (red, or
    a failed step) and nothing else moves."""
    evs = events(d)
    for n, close in _closes(d).items():
        out = _outcome(evs, n, close)
        if out in ("integrada", "conflito"):
            continue
        if out is not None:
            return False
        try:
            if not _integrate(d, cfg, n, close["hash"], acts):
                return False
        except (OrqError, OSError, subprocess.SubprocessError) as e:
            acts.append(_fail(d, "integrate", n, str(e)))
            return False
    return True


KICKOFF_DIR = Path(os.environ.get("ORQ_KICKOFF_DIR")
                   or HERE.parents[1] / "orquestrar-auto" / "references")


def render_kickoff(role: str, ctx: dict) -> str:
    if role not in ("executor", "revisor"):
        raise OrqError(f"no kick-off mold for role {role!r}")
    path = KICKOFF_DIR / f"kickoff-{role}.md"
    try:
        mold = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        raise OrqError(f"kick-off mold missing: {path}") from None
    try:
        return mold.format_map(ctx)
    except (KeyError, IndexError, ValueError) as e:
        raise OrqError(f"kick-off mold {path.name}: bad placeholder {e}") from None


def _norm(cell: str) -> str:
    """Compared the way backend/app/orq_md.py does: no accents, no markdown, lower case."""
    s = unicodedata.normalize("NFKD", cell.strip().strip("`").replace("**", "").strip())
    return re.sub(r"\s+", " ", "".join(c for c in s if not unicodedata.combining(c))).lower()


TEAM_KEYS = ("papel", "vez", "sessao", "provider", "conta", "modelo", "esforco", "janela", "abertura")


def team_rows(text: str) -> list[dict]:
    """`## Quem é quem`, read as backend/app/orq_papeis.py reads it: orq stays stdlib and runs
    outside the backend. Keys without accents, `-` read as empty."""
    lines = [l for l in _section(text, "Quem é quem") if l.strip().startswith("|")]
    if len(lines) < 2:
        return []
    cells = lambda l: [c.strip() for c in l.strip().strip("|").split("|")]
    head = [_norm(c) for c in cells(lines[0])]
    out = []
    for l in lines[2:]:
        # orq_md.limpar: bold, then backticks; `-` is empty.
        vals = [re.sub(r"^\*\*(.*)\*\*$", r"\1", c).strip().strip("`").strip() for c in cells(l)]
        row = dict.fromkeys(TEAM_KEYS, "") | {h: ("" if v in ("-", "—") else v) for h, v in zip(head, vals)}
        if row["papel"]:
            out.append(row)
    return out


def role_row(rows: list[dict], role: str, n: int, risk: str) -> dict:
    """The role's row for Task N: numeric `vez` rotates by (N-1) % total; low/high follows the
    Task's Risk (arbitro-lancamento.md, "A rotating role")."""
    mine = [r for r in rows if _norm(r["papel"]) == role]
    if not mine:
        raise OrqError(f"no '{role}' row in ## Quem é quem")
    if len(mine) == 1:
        return mine[0]
    vez = [r.get("vez", "") for r in mine]
    if all(v.isdigit() for v in vez):
        return mine[(n - 1) % len(mine)]
    hit = [r for r in mine if r.get("vez", "").lower() == risk]
    if len(hit) == 1:
        return hit[0]
    raise OrqError(f"cannot pick the {role} row for Task {n} (vez {vez}, Risk {risk or 'none'})")


def session_name(pattern: str, n: int) -> str:
    """One session per Task: `x-t*` → `x-t4`; a fixed name gets the Task as suffix, so two Tasks of
    a wave never share a session."""
    if not pattern:
        raise OrqError("a row without a session name")
    raw = pattern.replace("*", str(n)) if "*" in pattern else f"{pattern}-t{n}"
    # backend/app/names.py: the name the session really gets, so events, sends and the sidecar
    # lookup all use it.
    ascii_name = unicodedata.normalize("NFKD", raw).encode("ascii", "ignore").decode()
    return re.sub(r"[^A-Za-z0-9_-]", "-", ascii_name.strip()).strip("-")


def open_flags(row: dict, read_only: bool) -> list[str]:
    """The row as `hangar-send --new` flags (arbitro-lancamento.md, "Opening a session"); the
    `abertura` cell goes last, as written."""
    prov = (row.get("provider") or "claude").lower()
    flags = ["--provider", prov]
    conta = row.get("conta", "")
    if prov in ("claude", "codex") and conta not in ("", "padrao", "default"):
        flags += ["--conta", conta]
    if row.get("modelo") not in (None, "", "default"):
        flags += ["--model", row["modelo"]]
    if row.get("esforco") and prov != "kimi":
        flags += ["--effort", row["esforco"]]
    extra = shlex.split(row.get("abertura", ""))
    # O backend recusa proteção com --headless; a proteção pedida deve ser preservada.
    if (read_only or "--read-only" in extra) and "--headless" in extra:
        raise OrqError(f"row `{row.get('sessao', '?')}`: a read-only session cannot open with --headless; "
                       "remove --headless from its `abertura` cell")
    if read_only and "--read-only" not in extra:
        flags.append("--read-only")
    if (read_only or "--read-only" in extra) and "--terminal" not in extra:
        flags.append("--terminal")
    # Only auto runs open sessions: the key the settings screen wrote counts too.
    if jev_config(auto=True).get("key") and not {"--jev", "--sem-jev"} & set(extra):
        flags.append("--jev")
    return flags + extra


def _model_matches(want: str, got: str | None) -> bool:
    """`opus[1m]` is born as `opus` plus a context window, and a live session may carry the full
    id: the base name on either side is enough."""
    base = re.sub(r"\[.*?\]$", "", want).lower()
    return bool(got) and (base in got.lower() or got.lower() in base)


def prove_born(name: str, row: dict) -> str | None:
    """What was born, never what was asked: the sidecar of a session without terminal, or the
    pane's start command, carries the row's provider, model and service tier. None = it matches."""
    prov = (row.get("provider") or "claude").lower()
    want = row.get("modelo") if row.get("modelo") not in (None, "", "default") else ""
    extra = shlex.split(row.get("abertura", ""))
    tier = extra[extra.index("--service-tier") + 1] if "--service-tier" in extra[:-1] else ""
    for sub, dflt in (("claude-headless", "claude"), ("codex-sessions", "codex")):
        f = Path.home() / ".hangar" / sub / f"{name}.json"
        if f.exists():
            sc = json.loads(f.read_text(encoding="utf-8"))
            got = sc.get("provider") or dflt
            if got != prov:
                return f"{name}: born on {got}, row says {prov}"
            if want and not _model_matches(want, sc.get("model")):
                return f"{name}: born on model {sc.get('model')}, row says {want}"
            if tier and (sc.get("service_tier") or "default") != tier:
                return f"{name}: born on service tier {sc.get('service_tier') or 'default'}, row says {tier}"
            return None
    r = subprocess.run(["tmux", "display", "-p", "-t", f"={name}:", "#{pane_start_command}"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        return f"{name}: no sidecar and no tmux pane to prove what was born"
    cmd = r.stdout.strip()
    if prov not in cmd or (want and not _model_matches(want, cmd)):
        return f"{name}: pane started `{cmd[:160]}`, row says {prov} {want}".rstrip()
    # Só o prefixo do hangar-engine, antes do `--`: o resto do comando pode ter qualquer texto.
    born = re.search(r"--service-tier (\S+)", cmd.split(" -- ", 1)[0])
    born_tier = born.group(1).strip("'\"") if born else "default"
    if tier and born_tier != tier:
        return f"{name}: born on service tier {born_tier}, row says {tier}"
    return None


def hangar_new(name: str, cwd: str, flags: list[str]) -> None:
    cmd = [os.environ.get("ORQ_SEND", "hangar-send"), "--new", name, cwd, *flags]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=SEND_TIMEOUT_S)
    except subprocess.TimeoutExpired:
        raise OrqError(f"hangar-send --new {name} did not answer in {SEND_TIMEOUT_S}s") from None
    if r.returncode != 0:
        raise OrqError(f"hangar-send --new {name} failed (rc={r.returncode}): "
                       f"{(r.stderr or r.stdout).strip()[:300]}")


def _open_task(d: Path, cfg: dict, pj: dict, t: dict) -> str:
    """Worktree (in a wave), executor and reviewer from `## Quem é quem`, proof of what was born,
    `task_inicio`, then the kick-offs: the arbiter's handoff order (arbitro.md, step 2)."""
    n, repo = t["n"], cfg["repo"]
    title = t["title"] or f"Task {n}"
    rows = team_rows(Path(cfg["contract"]).read_text(encoding="utf-8"))
    ex_row = role_row(rows, "executor", n, t["risk"])
    rev_row = None if pj.get("revisao") == "subagente" else role_row(rows, "revisor", n, t["risk"])
    ex = session_name(ex_row.get("sessao", ""), n)
    rev = session_name(rev_row.get("sessao", ""), n) if rev_row else SUBAGENT
    branch = git(repo, "branch", "--show-current").strip()
    if not branch:
        raise OrqError(f"{repo} is on a detached HEAD: no branch to work on")
    wave = (pj.get("paralelo") or 1) > 1
    # The gid keeps a later run on the same branch from meeting this run's leftovers.
    gid = _gid(d)
    cwd = str(Path(repo).parent / f"{Path(repo).name}-{gid}-t{n}") if wave else repo
    if wave:
        branch = f"{branch}-{gid}-t{n}"
    # The executor's Expected HEAD: the main line after the merges so far, and the wave's base.
    base = git(repo, "rev-parse", "HEAD").strip()
    ctx = {"task": n, "title": title, "worktree": cwd, "branch": branch, "run_dir": str(d),
           "plan": cfg["plan"], "contract": cfg["contract"], "executor": ex, "reviewer": rev,
           "arbiter": state(d)["arbiter"], "base": base,
           # One by one, never "the ones in the contract" (arbitro-lancamento.md, "Kick-off").
           "untouchables": "\n".join(f"- {u}" for u in cfg.get("untouchables") or []) or "- none"}
    # Rendered before anything opens: a broken mold costs no session.
    kicks = [(role, target, render_kickoff(role, ctx))
             for role, target in (("revisor", rev), ("executor", ex)) if target != SUBAGENT]
    made: list[str] = []
    recorded = False
    try:
        if wave:
            git(repo, "worktree", "add", cwd, "-b", branch, base)
            made.append(f"worktree {cwd} on branch {branch}")
            journal_append(d, f"T{n} worktree {cwd}, branch {branch}, base {base[:12]}")
        for name, row, read_only in ((ex, ex_row, False), (rev, rev_row, True)):
            if row is None:
                continue
            hangar_new(name, cwd, open_flags(row, read_only))
            made.append(f"session {name} in {cwd}")
            why = prove_born(name, row)
            if why:
                raise OrqError(f"born wrong: {why}")
            _record_session(d, name, "revisor" if read_only else "executor", n)
        ev = event_append(d, {"tipo": "task_inicio", "task": n, "titulo": title, "executor": ex, "par": rev})
        recorded = True
        journal_append(d, _event_line(ev))
        for role, target, text in kicks:
            path = d / "kickoffs" / f"task{n}-{role}.md"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
            made.append(f"kick-off {path} (not yet sent to {target})")
            _say(d, target, text)
            made[-1] = f"kick-off {path} sent to {target}"
    except (OrqError, OSError, ValueError, subprocess.SubprocessError) as e:
        if not made and not recorded:
            raise
        # Like _send_after_event: the arbiter finishes or undoes by hand, knowing what exists.
        left = [x for x in made if not x.startswith("kick-off")]
        left.append("task_inicio recorded" if recorded else "task_inicio NOT recorded")
        left += [x for x in made if x.startswith("kick-off")] or ["no kick-off written"]
        raise OrqError(f"{e}. Left behind (finish or clean by hand): {'; '.join(left)}") from None
    who = [f"{name} ({row.get('provider') or 'claude'}, {row.get('modelo') or 'modelo padrão'})"
           for name, row in ((ex, ex_row), (rev, rev_row)) if row]
    timeline(d, "advance", f"T{n}: abriu {' e '.join(who)} e entregou o kick-off", n)
    return f"opened T{n}: {ex} + {rev}"


def _open_ready(d: Path, cfg: dict, acts: list[str]) -> None:
    """The current wave — the first with a Task not integrated — opens its Tasks while
    `Paralelo:` has room; a later wave waits for the whole wave before it."""
    ptext = plan_text(cfg["plan"])
    pj, tasks = projeto(ptext), plan_tasks(ptext)
    evs = events(d)
    started = {ev.get("task") for ev in evs if ev.get("tipo") == "task_inicio"}
    done = _integrated(d, evs)
    in_wave = lambda t, w: t["wave"].isdigit() and int(t["wave"]) == w
    waves = sorted({int(t["wave"]) for t in tasks if t["wave"].isdigit()})
    wave = next((w for w in waves if any(t["n"] not in done for t in tasks if in_wave(t, w))), None)
    if wave is None:
        return
    # A failed opening keeps its slot: the arbiter may open it by hand at any time.
    failed = {ev.get("task") for ev in evs if ev.get("tipo") == "advance_falhou" and ev.get("passo") == "open"}
    room = (pj.get("paralelo") or 1) - len((started | failed) - done)
    for t in tasks:
        if room <= 0:
            return
        if not in_wave(t, wave) or t["n"] in started or _failed_since(evs, "open", t["n"], None):
            continue
        room -= 1
        try:
            acts.append(_open_task(d, cfg, pj, t))
        except (OrqError, OSError, ValueError, subprocess.SubprocessError) as e:
            acts.append(_fail(d, "open", t["n"], str(e)))


def _announce_batch(d: Path, cfg: dict, acts: list[str]) -> None:
    """`Prova: lote(N)`: the batch goes out full, or when a wave is over, and only on integrated
    code; the arbiter opens the proof session (prova-lote.md)."""
    prova = plan_of(d).get("prova")
    pend = pending_proofs(d)
    if not prova or prova[0] != "lote" or not pend:
        return
    if any(p.get("task") not in _integrated(d, events(d)) for p in pend):
        return
    tasks = plan_tasks(plan_text(cfg["plan"]))
    wave = {t["n"]: t["wave"] for t in tasks}
    closed = _closed(d)
    wave_over = any(all(t["n"] in closed for t in tasks if t["wave"] == wave.get(p.get("task")))
                    for p in pend)
    if len(pend) < prova[1] and not wave_over:
        return
    # Taken only once the arbiter heard of it: a failed send keeps the proofs pending.
    line = take_batch(d, dry=True, pend=pend)
    names = ", ".join(f"T{p['task']}" for p in pend)
    _wake(d, f"[decisao] Proof batch ready — {line}. Open one proof session for those roteiros on "
             "the integrated code (prova-lote.md).",
          f"acordou o árbitro: lote de prova pronto ({names})")
    take_batch(d, pend=pend)   # the Tasks announced, even if a commit queued one meanwhile
    acts.append(f"batch: {line}")


def _final_review(d: Path, cfg: dict, acts: list[str]) -> None:
    tasks = plan_tasks(plan_text(cfg["plan"]))
    evs = events(d)
    # Nova integração após o aviso muda a versão que a revisão final precisa conferir.
    last_integrated = max((i for i, ev in enumerate(evs) if ev.get("tipo") == "integrada"), default=-1)
    if not tasks or any(ev.get("tipo") == "tudo_integrado" for ev in evs[last_integrated + 1:]):
        return
    if not {t["n"] for t in tasks} <= _integrated(d, evs):
        return
    _wake(d, "[decisao] Every Task of the plan is integrated: your turn for the final review "
             "(arbitro-encerramento.md, \"Phase 4\").",
          "acordou o árbitro: todas as Tasks integradas, hora da revisão final")
    # After the send: recorded first, a failed send would never be tried again.
    event_append(d, {"tipo": "tudo_integrado"})
    acts.append("all integrated: arbiter woken for the final review")


def _send_retry(d: Path, passo: str) -> bool:
    """True = a failed wake of this step is tried again on a later pass; the SEND_TRIES-th failure
    is final and resets the count."""
    f = d / f"send-tries-{passo}"
    try:
        n = int(f.read_text(encoding="utf-8")) + 1
    except (OSError, ValueError):
        n = 1
    if n >= SEND_TRIES:
        f.unlink(missing_ok=True)
        return False
    f.write_text(str(n), encoding="utf-8")
    return True


def _pass(d: Path, cfg: dict, acts: list[str]) -> None:
    """Integration first: nothing opens or goes to proof on a main line that is red or waiting."""
    off = _off_branch(d, cfg["repo"])
    if off:
        _notice_once(d, off)
        return
    if not _integrate_all(d, cfg, acts):
        return
    evs = events(d)
    # A step that failed as a whole waits for a new close before it is tried again; a failed
    # opening of one Task carries its number and is skipped inside _open_ready.
    since = max((c.get("ts") or "" for c in _closes(d).values()), default=None) or None
    for passo, step in (("batch", _announce_batch), ("open", _open_ready), ("final", _final_review)):
        if _failed_since(evs, passo, None, since):
            continue
        try:
            step(d, cfg, acts)
            (d / f"send-tries-{passo}").unlink(missing_ok=True)
        except SendError as e:
            if _send_retry(d, passo):
                acts.append(f"{passo}: arbiter not reached ({e}), retried on the next pass")
            else:
                acts.append(_fail(d, passo, None, str(e)))
        except (OrqError, OSError, ValueError, subprocess.SubprocessError) as e:
            acts.append(_fail(d, passo, None, str(e)))


def advance(d: Path) -> list[str]:
    """The orchestrator's pass: every step the rule knows, each one an event, a journal line and a
    timeline line. Single by a file lock; a caller that finds it held leaves a mark, and the holder
    runs one more pass for it."""
    import fcntl  # POSIX only: the other commands keep working without it
    cfg = config(d)
    if not cfg.get("auto"):
        return []
    again = d / "advance.again"
    again.touch()
    acts: list[str] = []
    # "a", not "w": a caller that finds it held must not wipe the holder's pid.
    with (d / "advance.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return []
        # The pid shows who holds it; the lock's mtime is what Hangar reads as "working".
        lock.truncate(0)
        lock.write(f"{os.getpid()}\n")
        lock.flush()
        # ponytail: a mark left between the holder's last check and its unlock waits for the next
        # trigger (the watchdog's cycle at worst).
        while again.exists():
            again.unlink()
            _touch(d / "advance.lock")
            try:
                if state(d)["ended"]:
                    break
                # A crashed pass waits for a new close, like any failed step: the arbiter was told
                # it will not be retried and may be doing it by hand.
                since = max((c.get("ts") or "" for c in _closes(d).values()), default=None) or None
                if _failed_since(events(d), "advance", None, since):
                    break
                _pass(d, cfg, acts)
            except Exception as e:  # noqa: BLE001 — a crash must reach the arbiter
                acts.append(_fail(d, "advance", None, f"{type(e).__name__}: {e}"))
                break
    return acts


def spawn_advance(d: Path) -> str | None:
    """`orq advance` in its own session, output in <dir>/advance.log: the caller's turn never waits
    on merges and checks, and a run that dies is picked up by the watchdog's next cycle. None when
    started; otherwise the error, already warned on stderr (the watchdog's cycle retries)."""
    try:
        with (d / "advance.log").open("a", encoding="utf-8") as log:
            # cwd: the run dir, never the caller's: a Task worktree removed at the end of the run
            # must not be the working directory of a pass still running.
            subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--dir", str(d.resolve()),
                              "advance"],
                             stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                             start_new_session=True, cwd=str(d))
    except OSError as e:
        print(f"orq: warning: orq advance not started: {e}", file=sys.stderr)
        return str(e)
    return None


def cmd_advance(a) -> int:
    d = base_dir(a.dir)
    if not config(d).get("auto"):
        return 0
    if a.detach:
        return 1 if spawn_advance(d) else 0
    try:
        lines = advance(d)
    except Exception as e:  # noqa: BLE001 — outside the pass (lock, recording a failure): still the arbiter's to know
        err = f"{type(e).__name__}: {e}"
        try:
            print(_fail(d, "advance", None, err))
        except Exception as e2:  # noqa: BLE001
            print(f"orq: advance failed ({err}) and the arbiter was not told: {e2}", file=sys.stderr)
        return 1
    for line in lines:
        print(line)
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
    s.add_argument("--plan", help="required, except to re-init a run started without a plan")
    s.add_argument("--auto", action="store_true", help="an orquestrar-auto run: the orchestrator does the routine")
    s.add_argument("--jev", choices=["on", "shadow"], help="with --auto: Jev drops (on) or only records (shadow)")
    s.add_argument("--regex", choices=["on", "shadow"], help="with --auto and no Jev key: same, for the regex")
    s = sub.add_parser("readiness", help="which plan a run starts from, and whether it is stamped (JSON)")
    s.add_argument("--repo", required=True)
    s = sub.add_parser("plan-check", help="check the orchestration plan's structure; --stamp marks it prepared")
    s.add_argument("plan")
    s.add_argument("--repo", required=True)
    s.add_argument("--stamp", action="store_true")
    s = sub.add_parser("event", help="validate and append one eventos.jsonl line")
    s.add_argument("tipo")
    for k in EVENT_FIELDS_INT:
        s.add_argument(f"--{k}", type=int)
    for k in EVENT_FIELDS_STR:
        s.add_argument(f"--{k}")
    s.add_argument("--reincide", action="store_true")
    s.add_argument("--repo", help="the Task's worktree; default orq.json's")
    s = sub.add_parser("check", help="run the plan's checks on a frozen round")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--commit", required=True)
    s.add_argument("--repo", help="the Task's worktree; default orq.json's")
    s = sub.add_parser("apply-patch", help="apply the reviewer's patch as the next round")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--repo", help="the Task's worktree; default orq.json's")
    s = sub.add_parser("review-package", help="build the one file a subagent reviewer reads")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--rodada", type=int, required=True)
    s.add_argument("--repo", help="the Task's worktree; default orq.json's")
    s.add_argument("--report", help="the executor's round report")
    s = sub.add_parser("read", help="the contract's common part + one Task, or the journal's tail")
    s.add_argument("what", choices=["contract", "journal"])
    s.add_argument("--task", type=int)
    s.add_argument("--last", type=int, default=15)
    s = sub.add_parser("ball", help="who owes work now")
    s.add_argument("--with-arbiter", action="store_true", help="append the current arbiter, last")
    s.add_argument("--coverage", action="store_true",
                   help="all | owners | none: who of the owners and the arbiter has a wait in force")
    sub.add_parser("done", help="sessions whose part is over (the watchdog closes them)")
    sub.add_parser("team", help="who must be in the arbiter's group (the watchdog joins them)")
    for name, extra in (("lock", True), ("screen", False)):
        s = sub.add_parser(name, help="a lock on a shared resource" if extra else "alias of `lock … screen`")
        s.add_argument("action", choices=["take", "release"])
        if extra:
            s.add_argument("resource")
        else:
            s.set_defaults(resource="screen")
        s.add_argument("--owner", required=True)
        s.add_argument("--wait-min", type=float, default=20)
    s = sub.add_parser("commit", help="check the Task's commit and close it")
    s.add_argument("--task", type=int, required=True)
    s.add_argument("--hash", required=True)
    s.add_argument("--repo", help="the checkout holding the commit (a batch worktree); default orq.json's")
    s = sub.add_parser("batch", help="take the pending proofs as one batch")
    s.add_argument("action", choices=["take"])
    s = sub.add_parser("notify", help="the only path of a message to the arbiter")
    s.add_argument("--alarm", action="store_true")
    s.add_argument("text")
    s = sub.add_parser("log", help="a decision entry in the journal")
    s.add_argument("--task", type=int)
    s.add_argument("text")
    s = sub.add_parser("advance", help="the orchestrator's pass: integrate, open, announce (auto runs only)")
    s.add_argument("--detach", action="store_true", help="run in the background (the watchdog's call)")
    return p


CMDS = {"init": cmd_init, "readiness": cmd_readiness, "plan-check": cmd_plan_check, "event": cmd_event, "check": cmd_check,
        "read": cmd_read, "ball": cmd_ball, "done": cmd_done, "team": cmd_team,
        "lock": cmd_lock, "screen": cmd_lock, "commit": cmd_commit, "notify": cmd_notify, "log": cmd_log,
        "apply-patch": cmd_apply_patch, "batch": cmd_batch, "review-package": cmd_review_package,
        "advance": cmd_advance}


def main(argv: list[str] | None = None) -> int:
    a = build_parser().parse_args(argv)
    try:
        return CMDS[a.cmd](a)
    except OrqError as e:
        print(f"orq: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
