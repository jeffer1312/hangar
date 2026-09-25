"""Painel do condutor da orquestração: o vigia está vivo? o que passou pelo `orq`?

Fontes, todas só de leitura: `<dir>/vigia.json` (batimento que o vigia grava a cada volta), a
unidade systemd `vigia-*` cujo `-e` aponta pra pasta, e o feed montado de `registro.md`,
`jev-shadow.jsonl` e `eventos.jsonl`. Linha que não se lê é contada em `skipped`, nunca derruba
a resposta.
"""
from __future__ import annotations

import json
import logging
import math
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timedelta
from pathlib import Path

from app import orq

_log = logging.getLogger(__name__)

JOURNAL_TAIL = 300
FEED_CAP = 500
# A sombra é gravada no mesmo `notify` que a linha do registro, segundos antes dela.
MATCH_S = 10
SYSTEMCTL_TIMEOUT_S = 3
# O mesmo VETO_P de skills/orquestrar/scripts/orq.py: veto acima dele manteve o árbitro acordado.
JEV_VETO_P = 0.40
# (prefixo, tipo, tira o prefixo do texto). Os que ficam com ele se explicam sozinhos no feed.
_PREFIXES = (
    ("notify → arbiter: ", "woke", True),
    ("(jev: no action) ", "dropped", True),
    ("aviso: ", "notice", True),
    ("alarm: ", "alarm", True),
    ("notify FAILED: ", "alarm", False),
    ("close failed: ", "alarm", False),
    ("join failed: ", "alarm", False),
    ("closed session: ", "notice", False),
    ("joined group: ", "notice", False),
)
_LINE = re.compile(r"^- (\S+) · (.*)$")
_TASK = re.compile(r"\bT(\d+)\b|\bTask (\d+)\b")
_EXEC_DIR = re.compile(r"(?:^|\s)(?:-e|--eventos)\s+(\S+)")


def _when(v) -> datetime | None:
    # Linha antiga sem fuso vira hora local: naive contra aware levantaria no sort.
    # Data na borda do calendário estoura na conversão: a linha é pulada, nunca um 500.
    try:
        return datetime.fromisoformat(str(v)).astimezone()
    except (ValueError, OverflowError):
        return None


def _num(v) -> float | None:
    # NaN do Jev iria pro JSON, e o Starlette recusa NaN com 500.
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v):
        return None
    return float(v)


def _int(v) -> int | None:
    try:
        return int(v)
    except (TypeError, ValueError):
        return None


def _lines(p: Path) -> list[str]:
    try:
        return p.read_text(encoding="utf-8", errors="replace").splitlines()
    except (OSError, ValueError):
        return []


def _task_of(text: str) -> int | None:
    m = _TASK.search(text)
    return int(m.group(1) or m.group(2)) if m else None


def _journal(d: Path) -> tuple[list[dict], int]:
    lines = _lines(d / "registro.md")
    start = max(0, len(lines) - JOURNAL_TAIL)
    items: list[dict] = []
    skipped = 0
    for n, line in enumerate(lines[start:], start + 1):
        if not line.strip():
            continue
        m = _LINE.match(line)
        when = _when(m.group(1)) if m else None
        if when is None:
            skipped += 1
            continue
        body = m.group(2)
        # Linha sem prefixo (evento, trava da tela, init) fica fora: o evento vem do eventos.jsonl.
        for prefix, kind, strip in _PREFIXES:
            if body.startswith(prefix):
                text = body[len(prefix):] if strip else body
                items.append({"id": f"r{n}", "when": when, "ts": m.group(1), "kind": kind,
                              "text": text, "task": _task_of(text), "jev": None})
                break
    return items, skipped


def _shadow(d: Path) -> tuple[list[dict], int]:
    # Cada linha da sombra tem a sua no registro: fora da cauda dele, não casaria com nada.
    out: list[dict] = []
    skipped = 0
    for line in _lines(d / "jev-shadow.jsonl")[-JOURNAL_TAIL:]:
        if not line.strip():
            continue
        try:
            r = json.loads(line)
        except ValueError:
            skipped += 1
            continue
        when = _when(r.get("ts")) if isinstance(r, dict) else None
        if when is None or not isinstance(r.get("text"), str):
            skipped += 1
            continue
        out.append({"when": when, "norm": " ⏎ ".join(r["text"].strip().splitlines()),
                    "cut": len(r["text"]) >= 500, "row": r})
    return out, skipped


def _jev(r: dict) -> dict:
    raw = r.get("veto") if isinstance(r.get("veto"), dict) else {}
    veto = {str(k): _num(v) for k, v in raw.items()}
    return {"mode": r.get("mode"), "choice": r.get("choice"), "p": _num(r.get("p")), "veto": veto,
            # Valor ilegível conta como veto que segurou, como no orq (NaN nunca descarta).
            "held": [k for k, v in veto.items() if v is None or v > JEV_VETO_P],
            "would_drop": r.get("would_drop"), "error": r.get("error")}


def _attach_jev(items: list[dict], shadow: list[dict]) -> None:
    used: set[int] = set()
    for it in items:
        if it["kind"] not in ("woke", "dropped"):
            continue
        for i, s in enumerate(shadow):
            if i in used or abs((s["when"] - it["when"]).total_seconds()) > MATCH_S:
                continue
            if s["norm"] == it["text"] or (s["cut"] and it["text"].startswith(s["norm"])):
                it["jev"] = _jev(s["row"])
                used.add(i)
                break


def _event_text(ev: dict) -> str:
    # O mesmo formato da linha que o orq escreve no registro para o evento.
    parts = [ev["tipo"]]
    if (t := orq._int_ou_none(ev.get("task"))) is not None:
        parts.append(f"T{t}")
    if (r := orq._int_ou_none(ev.get("rodada"))) is not None:
        parts.append(f"r{r}")
    for k in ("resultado", "commit", "sessao", "executor", "par", "de", "para", "motivo"):
        if ev.get(k) not in (None, ""):
            parts.append(f"{k}={ev[k]}")
    if ev.get("reincide") is True:
        parts.append("reincide")
    return " ".join(parts)


def _events(d: Path) -> tuple[list[dict], int]:
    items: list[dict] = []
    skipped = 0
    for n, line in enumerate(_lines(d / "eventos.jsonl"), 1):
        if not line.strip():
            continue
        try:
            ev = json.loads(line)
        except ValueError:
            skipped += 1
            continue
        ok = isinstance(ev, dict) and ev.get("tipo") in orq._TIPOS
        when = _when(ev.get("ts")) if ok else None
        if when is None:
            skipped += 1
            continue
        items.append({"id": f"e{n}", "when": when, "ts": str(ev["ts"]), "kind": "event",
                      "text": _event_text(ev), "task": orq._int_ou_none(ev.get("task")), "jev": None})
    return items, skipped


def feed(d: Path) -> dict:
    journal, s1 = _journal(d)
    shadow, s2 = _shadow(d)
    events, s3 = _events(d)
    _attach_jev(journal, shadow)
    items = sorted(journal + events, key=lambda i: i["when"], reverse=True)
    return {"feed": [{k: v for k, v in i.items() if k != "when"} for i in items[:FEED_CAP]],
            "truncated": len(items) > FEED_CAP, "skipped": s1 + s2 + s3}


def _pid_alive(pid) -> bool:
    # Só no Linux: no Windows o os.kill(pid, 0) MATA o processo, e aqui basta o /proc.
    if sys.platform != "linux" or isinstance(pid, bool) or not isinstance(pid, int):
        return True
    return Path(f"/proc/{pid}").exists()


def _heartbeat(d: Path) -> dict | None:
    try:
        hb = json.loads((d / "vigia.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return hb if isinstance(hb, dict) and _when(hb.get("ts")) else None


def _since(v) -> str | None:
    m = re.fullmatch(r"@(\d+)", v or "")
    return datetime.fromtimestamp(int(m.group(1))).astimezone().isoformat(timespec="seconds") if m else None


def units() -> dict[str, dict] | None:
    """Unidades `vigia-*` pela pasta do `-e <dir>` no ExecStart; None = não há systemd pra
    perguntar. Pasta, unidade e grupo têm nomes diferentes, então a chave é o caminho."""
    if sys.platform != "linux" or not shutil.which("systemctl"):
        return None
    try:
        r = subprocess.run(
            ["systemctl", "--user", "show", "vigia-*", "--timestamp=unix",
             "-p", "Id,ExecStart,ActiveState,ActiveEnterTimestamp,NRestarts"],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            timeout=SYSTEMCTL_TIMEOUT_S)
    except (OSError, subprocess.TimeoutExpired) as e:
        _log.warning("orq: systemctl não respondeu (%s); condutor só pelo batimento", e)
        return None
    if r.returncode != 0:
        _log.warning("orq: systemctl show saiu %s: %s", r.returncode, (r.stderr or "").strip()[:200])
        return None
    out: dict[str, dict] = {}
    for bloco in r.stdout.split("\n\n"):
        props = dict(l.split("=", 1) for l in bloco.splitlines() if "=" in l)
        m = _EXEC_DIR.search(props.get("ExecStart", ""))
        if not m:
            continue
        key = os.path.normpath(os.path.expanduser(m.group(1)))
        # Duas unidades na mesma pasta (rearmada com outro nome): vale a que está no ar.
        if out.get(key, {}).get("ActiveState") != "active":
            out[key] = props
    return out


def watchdog(d: Path, units: dict[str, dict] | None, now: datetime | None = None) -> dict:
    now = now or datetime.now().astimezone()
    hb = _heartbeat(d)
    u = units.get(os.path.normpath(str(d))) if units is not None else None
    state = u.get("ActiveState") if u else None
    if hb:
        iv = hb.get("interval_s")
        iv = iv if isinstance(iv, int) and not isinstance(iv, bool) and iv >= 0 else 60
        fresh = now - _when(hb["ts"]) <= timedelta(seconds=2 * iv + 30)
        # Unidade parada ou processo do batimento já morto não esperam o batimento envelhecer.
        alive = fresh and state in (None, "active", "activating") and _pid_alive(hb.get("pid"))
        source = "heartbeat"
    elif u:
        alive, source = state == "active", "systemd"
    else:
        alive, source = False, ("none" if units is not None else "unavailable")
    watching = hb.get("watching") if hb else None
    return {
        "alive": bool(alive),
        "last_cycle": str(hb["ts"]) if hb else None,
        "since": _since(u.get("ActiveEnterTimestamp")) if u else None,
        "restarts": _int(u.get("NRestarts")) if u else None,
        "unit": u["Id"].removesuffix(".service") if u and u.get("Id") else (hb or {}).get("unit"),
        "unit_state": state,
        "arbiter": (hb or {}).get("arbiter"),
        "watching": [str(w) for w in watching] if isinstance(watching, list) else [],
        "source": source,
    }


def conductor(d: Path, units: dict[str, dict] | None) -> dict:
    return {"watchdog": watchdog(d, units), **feed(d)}
