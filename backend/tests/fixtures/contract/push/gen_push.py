"""Golden do aviso no celular que o Rust passou a decidir (`crates/hangar-server/src/push/`), tirado
do Python que decidia (`push.py`, `stall_watch.py`, `background_open.py` e as transições do `api.py`):

- `summary`: `api._resumo_resposta`;
- `quiet`: `push._in_quiet_hours` com a janela gravada e a hora dada;
- `reset`: `stall_watch._parse_reset_delay` com o agora fixo;
- `payloads`: o JSON de cada inscrição do Web Push e o que vai ao Expo, por aviso;
- `scan`: `background_open.scan` sobre transcripts montados aqui;
- `sweep`: os avisos do `stall_watch._tick` numa sequência de listas;
- `series`: o início da série de turnos (`_working_started`) depois de cada transição.

Uso, de backend/: uv run python tests/fixtures/contract/push/gen_push.py
"""
import json
import subprocess
import sys
import tempfile
from concurrent.futures import Future
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
from app import api, background_open, push, push_expo_bridge, runtime_config, stall_watch  # noqa: E402
from app.config import settings  # noqa: E402

SUMMARY = [None, "", "   ", "Pronto.", "Feito: **tudo** `ok` [o link](http://x) e _nome_ de snake_case __dunder__ fim",
           "# Título\n\n> citação | tabela", "a" * 170, ("palavra " * 30).strip(), "x " * 80 + "fim", "_a b_ c__d e__"]

QUIET = [
    {"prefs": {}, "now": "03:00"},
    {"prefs": {"quiet_hours": {"start": "22:00", "end": "07:00"}}, "now": "23:30"},
    {"prefs": {"quiet_hours": {"start": "22:00", "end": "07:00"}}, "now": "06:59"},
    {"prefs": {"quiet_hours": {"start": "22:00", "end": "07:00"}}, "now": "07:00"},
    {"prefs": {"quiet_hours": {"start": "09:00", "end": "18:00"}}, "now": "12:00"},
    {"prefs": {"quiet_hours": {"start": "09:00", "end": "18:00"}}, "now": "18:00"},
    {"prefs": {"quiet_hours": {"start": "09:00", "end": "09:00"}}, "now": "09:00"},
]

RESET = ["3pm", "3:30pm", "12am", "12pm", "15:05", "9", " 7 AM ", "14:30", "25:00", "3:61", "amanhã", ""]
NOW = datetime(2026, 10, 23, 14, 30)

SUBS = [
    {"subscription": {"endpoint": "e-pt"}, "label": "Casa", "serverId": "s1", "locale": "pt"},
    {"subscription": {"endpoint": "e-en"}, "label": "Work", "serverId": "s2", "locale": "en"},
    {"subscription": {"endpoint": "e-old"}, "label": "", "serverId": "s3"},
]


class _Pool:
    def submit(self, fn, *args):
        f = Future()
        f.set_result(fn(*args))
        return f


def payloads():
    out = []
    sent, expo = [], []
    def fake_webpush(subscription_info, data, **_):
        sent.append({"endpoint": subscription_info["endpoint"], "payload": json.loads(data)})
    def fake_expo(session, title, body, list_view):
        expo.append({"session": session, "title": title, "body": body, "list": list_view})
    calls = [
        ("awaiting_question", lambda: push.notify_awaiting("s", "Qual ambiente?")),
        ("awaiting_default", lambda: push.notify_awaiting("s", None)),
        ("awaiting_coalesced", lambda: (push.notify_awaiting("b", "x"), push.notify_awaiting("a", None))),
        ("finished_reply", lambda: push.notify_finished("s", "Tudo pronto")),
        ("finished_plain", lambda: push.notify_finished("s")),
        ("dead", lambda: push.notify_dead("s")),
        ("stalled", lambda: push.notify_stalled("s")),
        ("limited_reset", lambda: push.notify_limited("s", "3pm")),
        ("limited_plain", lambda: push.notify_limited("s")),
    ]
    with patch.object(settings, "vapid_private", "k"), patch.object(settings, "vapid_public", "k"), \
         patch.object(push, "_load", lambda: [dict(s) for s in SUBS]), patch.object(push, "_save", lambda _s: None), \
         patch.object(push, "_suppressed", lambda _n: False), patch("pywebpush.webpush", fake_webpush), \
         patch.object(push_expo_bridge, "configured", lambda: True), patch.object(push_expo_bridge, "send", fake_expo), \
         patch.object(push, "_expo_pool", _Pool()):
        for case, call in calls:
            sent.clear()
            expo.clear()
            call()
            push._flush_coalesce()
            out.append({"case": case, "web": list(sent), "expo": list(expo)})
    return out


def _line(kind, uid, ts, content):
    role = "assistant" if kind == "assistant" else "user"
    return json.dumps({"type": kind, "uuid": uid, "timestamp": ts, "message": {"role": role, "content": content}})


REPLY = _line("assistant", "a1", "2026-10-23T12:00:00.000Z", [{"type": "text", "text": "Pronto, **feito**."}])
LAUNCH = _line("assistant", "a2", "2026-10-23T12:01:00.000Z",
               [{"type": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "sleep 99", "run_in_background": True}}])
STARTED = _line("user", "u1", "2026-10-23T12:01:01.000Z",
                [{"type": "tool_result", "tool_use_id": "tu1", "content": "Command running in background with ID: bash_7"}])
CLOSED = _line("user", "u2", "2026-10-23T12:05:00.000Z",
               "<task-notification>\n<task-id>bash_7</task-id>\n<status>completed</status>\n<summary>Background command \"x\" completed</summary>\n</task-notification>")
WAITING = _line("user", "u3", "2026-10-23T12:05:00.000Z",
                "<task-notification>\n<task-id>bash_7</task-id>\n<status>completed</status>\n<summary>Agent \"x\" is waiting on its own background work</summary>\n</task-notification>")
LATE_REPLY = _line("assistant", "a3", "2026-10-23T12:06:00.000Z", [{"type": "text", "text": "Depois do fundo."}])
NOW_SCAN = datetime(2026, 10, 23, 12, 10, tzinfo=timezone.utc).timestamp()

SCAN = [
    {"case": "so_resposta", "lines": [REPLY]},
    {"case": "fundo_aberto", "lines": [REPLY, LAUNCH, STARTED]},
    {"case": "fundo_fechado", "lines": [REPLY, LAUNCH, STARTED, CLOSED, LATE_REPLY]},
    {"case": "aviso_parcial_nao_fecha", "lines": [REPLY, LAUNCH, STARTED, WAITING]},
    {"case": "vazio", "lines": []},
]


def scans():
    out = []
    for c in SCAN:
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "t.jsonl"
            p.write_text("\n".join(c["lines"]) + ("\n" if c["lines"] else ""), encoding="utf-8")
            for now in (NOW_SCAN, NOW_SCAN + 3600):
                s = background_open.scan(p, now)
                out.append({**c, "now": now, "open": s.open, "last_reply": s.last_reply})
    return out


SWEEP = [
    {"case": "morre_e_volta", "notify_dead": True, "steps": [["a", "b"], ["a"], ["a"], ["a", "b"], ["a"]]},
    {"case": "morte_sem_aviso", "notify_dead": False, "steps": [["a"], []]},
    {"case": "travada_uma_vez", "notify_dead": True, "steps": [["a"], ["a:stalled"], ["a:stalled"], ["a"], ["a:stalled"]]},
    {"case": "limite_uma_vez", "notify_dead": True, "steps": [["a:limited=3pm"], ["a:limited=3pm"], ["a"], ["a:limited"]]},
    {"case": "orquestrador_fora", "notify_dead": True, "steps": [["o:orq", "a"], ["a"]]},
]


def _row(spec):
    name, _, flags = spec.partition(":")
    reset = flags.split("=", 1)[1] if "limited=" in flags else None
    return SimpleNamespace(name=name, jsonl=f"/p/{name}.jsonl", provider="orq" if flags == "orq" else "claude",
                           stalled=flags == "stalled", limited=flags.startswith("limited"), limit_reset=reset)


def sweeps():
    out = []
    for c in SWEEP:
        for s in (stall_watch._notified, stall_watch._notified_dead, stall_watch._notified_limited, stall_watch._armed_limited):
            s.clear()
        stall_watch._seen_live.clear()
        calls = []
        record = lambda fn, *a: calls.append([fn.__name__, *a])  # noqa: E731
        steps = []
        with patch.object(stall_watch, "_send", record), \
             patch.object(runtime_config, "get", lambda k, real=runtime_config.get: c["notify_dead"] if k == "notify_dead" else real(k)):
            import asyncio
            for rows in c["steps"]:
                calls.clear()
                infos = [_row(r) for r in rows]
                async def lst(infos=infos):
                    return infos
                asyncio.run(stall_watch._tick(lst))
                steps.append(list(calls))
        out.append({**c, "notices": steps})
    return out


SERIES = [
    {"case": "turno_unico", "steps": [["working", 100], ["idle", 160]]},
    {"case": "continua_a_serie", "steps": [["working", 100], ["idle", 160], ["working", 200], ["idle", 300]]},
    {"case": "parada_longa_abre_nova", "steps": [["working", 100], ["idle", 160], ["working", 160 + 1801]]},
    {"case": "morte_limpa", "steps": [["working", 100], ["dead", 120], ["working", 130]]},
]


def series():
    out = []
    for c in SERIES:
        api._working_started.clear()
        api._last_idle.clear()
        started = []
        for state, ts in c["steps"]:
            with patch.object(api.hook_state, "get_state", lambda _sid, s=state, t=ts: (s, t)), \
                 patch.object(api.threading, "Thread", lambda *a, **k: SimpleNamespace(start=lambda: None)), \
                 patch.object(api, "_notify_async", lambda *_a, **_k: None):
                api._on_hook_transition("sid", state)
            started.append(api._working_started.get("sid"))
        out.append({**c, "started": started})
    return out


def main():
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    data = {
        "origin": {"commit": commit, "command": "uv run python tests/fixtures/contract/push/gen_push.py"},
        "summary": [{"text": t, "out": api._resumo_resposta(t)} for t in SUMMARY],
        "quiet": [{**c, "out": push_quiet(c)} for c in QUIET],
        "reset": [{"reset": r, "now": NOW.isoformat(), "out": stall_watch._parse_reset_delay(r, NOW)} for r in RESET],
        "payloads": payloads(),
        "scan": scans(),
        "sweep": sweeps(),
        "series": series(),
    }
    (HERE / "cases.json").write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


def push_quiet(c):
    from datetime import time as dtime
    with patch.object(push, "_load_prefs", lambda: c["prefs"]):
        return push._in_quiet_hours(dtime.fromisoformat(c["now"]))


if __name__ == "__main__":
    main()
