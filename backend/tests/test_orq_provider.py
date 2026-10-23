"""Provider `orq`: a linha do orquestrador na lista, a linha do tempo como conversa e as recusas."""
import asyncio
import json
import os
import time
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app import orq_timeline, pair, pqueue, registry
from app.adapters import get_adapter
from app.adapters.codex import sessions as codex_sessions
from app.adapters.orq import runs
from app.adapters.orq.adapter import parse_line
from app.config import settings
from app.models import SessionInfo
from app.registry import SessionRegistry

H = {"Authorization": "Bearer secret"}
LINES = [
    {"ts": "2026-09-28T10:00:00-03:00", "kind": "advance",
     "text": "T1 aprovada → merge → integração verde", "task": 1},
    {"ts": "2026-09-28T10:01:00-03:00", "kind": "would_drop",
     "text": "teria descartado (regex: entrega parada): recado de t2-exec", "task": 2},
]


def _run(root: Path, name: str, gid: str, auto: bool = True, ended: bool = False) -> Path:
    d = root / name
    d.mkdir()
    (d / "orq.json").write_text(json.dumps({"arbiter": "arb", "repo": str(root), "auto": auto}),
                                encoding="utf-8")
    evs = [{"tipo": "execucao_inicio", "ts": "2026-09-28T10:00:00-03:00", "gid": gid,
            "plano": "p.md", "branch": "main"}]
    if ended:
        evs.append({"tipo": "execucao_fim", "ts": "2026-09-28T11:00:00-03:00", "resultado": "ok"})
    (d / "eventos.jsonl").write_text("".join(json.dumps(e) + "\n" for e in evs), encoding="utf-8")
    return d


@pytest.fixture
def root(tmp_path, monkeypatch):
    r = tmp_path / "orq"
    r.mkdir()
    monkeypatch.setattr(runs, "root", lambda: r)
    # Only "alive" and the arbiter from the heartbeat matter here; orq_conductor has its own tests.
    monkeypatch.setattr(runs.orq_conductor, "watchdog",
                        lambda d, units: {"alive": not d.name.endswith("-dead"), "arbiter": "arb2"})
    return r


def test_list_has_one_row_per_live_auto_run(root, tmp_path, monkeypatch):
    _run(root, "2026-09-28-g1", "g1")
    _run(root, "2026-09-28-manual", "manual", auto=False)
    _run(root, "2026-09-28-done", "done", ended=True)
    _run(root, "2026-09-28-dead", "dead")
    (root / "jev-calibracao").mkdir()   # pasta sem orq.json fica fora, sem erro
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions")
    with patch.object(registry.tmux, "list_panes_all", return_value={}), \
         patch.object(registry, "_proc_children_map", return_value={}):
        out = SessionRegistry(projects_dir=tmp_path).list()
    orqs = [i for i in out if i.provider == "orq"]
    assert [i.name for i in orqs] == ["g1-orq"]
    row = orqs[0]
    assert row.pair_gid == "g1" and row.orq_arbiter == "arb2"
    assert row.tracked is True   # o celular desliga linha sem vínculo e mostra "Retomar"
    assert row.cwd == str(root)
    assert row.jsonl == str(root / "2026-09-28-g1" / "timeline-2026-09-28-g1.jsonl")


def test_backend_reads_the_timeline_file_orq_py_writes_through_a_symlinked_run(root, tmp_path):
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        "orq_timeline_mod", Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py")
    orq_py = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(orq_py)
    real = _run(tmp_path, "2026-09-28-real", "g1")
    link = root / "2026-09-28-link"
    link.symlink_to(real)
    orq_py.timeline(link, "advance", "linha")
    assert runs.timeline_path(link).read_text(encoding="utf-8").count("linha") == 1


def test_list_with_state_reads_activity_and_never_touches_tmux(root, tmp_path):
    d = _run(root, "2026-09-28-g1", "g1")
    tl = d / "timeline-2026-09-28-g1.jsonl"
    tl.write_text("", encoding="utf-8")
    reg = SessionRegistry(projects_dir=tmp_path)

    def state():
        info = SessionInfo(name="g1-orq", jsonl=str(tl), provider="orq", tracked=True)
        return asyncio.run(reg.list_with_state([info]))[0]

    with patch.object(registry.tmux, "capture_pane", side_effect=AssertionError("tmux")):
        fresh = state()
        assert fresh.state == "working"
        assert fresh.last_activity == pytest.approx(tl.stat().st_mtime)
        old = time.time() - 600
        os.utime(tl, (old, old))
        assert state().state == "idle"
        (d / "advance.lock").touch()   # advance rodando conta como atividade
        assert state().state == "working"


def test_a_held_advance_lock_is_working_whatever_its_age(tmp_path):
    # A merge plus the plan's Integração: outlasts ACTIVE_S; the lock is held the whole time.
    import fcntl
    tl = tmp_path / "timeline-x.jsonl"
    tl.write_text("", encoding="utf-8")
    lock = tmp_path / "advance.lock"
    lock.write_text("1\n", encoding="utf-8")
    old = time.time() - 600
    for p in (tl, lock):
        os.utime(p, (old, old))
    with lock.open("a") as f:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert runs.activity(str(tl))[0] == "working"
    assert runs.activity(str(tl))[0] == "idle"


def test_state_monitor_follows_the_timeline(tmp_path):
    tl = tmp_path / "timeline-x.jsonl"
    tl.write_text("", encoding="utf-8")

    async def first():
        agen = get_adapter("orq").state_monitor("g1-orq", sid_get=lambda: None,
                                                 transcript_get=lambda: str(tl))
        try:
            return await anext(agen)
        finally:
            await agen.aclose()

    ev = asyncio.run(first())
    assert ev.session == "g1-orq" and ev.state == "working"


def test_history_turns_each_timeline_line_into_a_notice(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue.settings, "projects_dir", str(tmp_path / "projects"))
    tl = tmp_path / "timeline-2026-09-28-g1.jsonl"
    tl.write_text("".join(json.dumps(l, ensure_ascii=False) + "\n" for l in LINES)
                  + '{"ts": "2026-09-28T10:02:00-03:00", "kind": "advance"}\n{meia linha\n',
                  encoding="utf-8")
    evs = pqueue.merged_history("g1-orq", str(tl), "orq")
    assert [(e.kind, e.text) for e in evs] == [("notice", l["text"]) for l in LINES]
    assert evs[0].ts == datetime.fromisoformat(LINES[0]["ts"]).timestamp()
    # Same id from the live tail and from /history: the client merges by id.
    assert [e.id for e in evs] == [parse_line(json.dumps(l, ensure_ascii=False))[0].id for l in LINES]
    assert len({e.id for e in evs}) == 2


@pytest.mark.parametrize("method,path,body", [
    ("post", "/api/sessions/g1-orq/input", {"text": "oi"}),
    ("delete", "/api/sessions/g1-orq", None),
    ("post", "/api/sessions/g1-orq/interrupt", None),
])
def test_orq_row_refuses_input_kill_interrupt(root, monkeypatch, method, path, body):
    _run(root, "2026-09-28-g1", "g1")
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    kw = {"json": body} if body is not None else {}
    r = getattr(TestClient(app), method)(path, headers=H, **kw)
    assert r.status_code == 409, r.text
    assert r.json()["detail"]["code"] == "erro_sessao_orq"


def test_name_ending_in_orq_without_a_live_run_is_not_refused(root, monkeypatch):
    _run(root, "2026-09-28-g1", "g1", ended=True)
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    with patch("app.tmux.has_session", return_value=False):
        r = TestClient(app).post("/api/sessions/g1-orq/input", headers=H, json={"text": "oi"})
    assert r.status_code == 404
    assert r.json()["detail"]["code"] == "erro_sessao_recado_nao_enfileirado"


def test_panel_route_serves_one_snapshot_per_run(root, monkeypatch):
    _run(root, "2026-09-28-g1", "g1")
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    client = TestClient(app)
    r = client.get("/api/sessions/g1-orq/orq/panel", headers=H)
    assert r.status_code == 200, r.text
    assert r.json()["consumption"] is None            # a soma roda numa thread: o 1º pedido não espera
    for lock in list(orq_timeline._consumption_locks.values()):
        with lock:                                     # a thread solta a trava ao terminar
            pass
    r = client.get("/api/sessions/g1-orq/orq/panel", headers=H)
    body = r.json()
    assert body["run"] == "2026-09-28-g1" and body["gid"] == "g1"
    assert body["consumption"]["sessions"]["team"] == 1 and body["automation"]["mode"] == {"jev": "shadow", "regex": "shadow"}


def test_panel_route_without_a_live_run_is_404(root, monkeypatch):
    _run(root, "2026-09-28-g1", "g1", ended=True)
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    with patch("app.tmux.has_session", return_value=False):
        r = TestClient(app).get("/api/sessions/g1-orq/orq/panel", headers=H)
    assert r.status_code == 404 and r.json()["detail"]["code"] == "erro_nao_encontrado"


def test_a_parecer_cited_in_the_timeline_opens_through_the_file_route(root, tmp_path, monkeypatch):
    d = _run(root, "2026-09-28-g1", "g1")
    parecer = tmp_path / "task-4-r1-revisor.md"
    parecer.write_text("# parecer\n", encoding="utf-8")
    line = {"ts": "2026-09-28T10:05:00-03:00", "kind": "woke", "task": None,
            "text": f"acordou o árbitro: [decisao] T4: incluir a tela? Parecer: {parecer}"}
    (d / "timeline-2026-09-28-g1.jsonl").write_text(json.dumps(line, ensure_ascii=False) + "\n", encoding="utf-8")
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    client = TestClient(app)
    panel = client.get("/api/sessions/g1-orq/orq/panel", headers=H).json()
    assert [x["parecer"] for x in panel["decisions"]] == [str(parecer)]
    r = client.get("/api/sessions/g1-orq/file", params={"path": str(parecer)}, headers=H)
    assert r.status_code == 200, r.text


def test_history_panel_remains_readable_after_session_disappears(root, monkeypatch):
    from app import api, orq
    d = _run(root, "2026-09-28-done", "done", ended=True)
    monkeypatch.setattr(settings, "auth_token", "secret")
    monkeypatch.setattr(orq, "raiz_padrao", lambda: root)
    monkeypatch.setattr(orq_timeline, "_consumption", lambda *args: (None, None))
    with patch.object(api, "_cached_info_sync", side_effect=AssertionError("live session queried")):
        response = TestClient(api.app).get(f"/api/orq/{d.name}/panel", headers=H)
    assert response.status_code == 200, response.text
    assert response.json()["timing"]["elapsed_seconds"] == 3600
    assert TestClient(api.app).get("/api/orq/missing/panel", headers=H).status_code == 404
    assert TestClient(api.app).get(f"/api/orq/{d.name}/panel").status_code == 401
