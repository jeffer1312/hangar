import pytest
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings
from app import tmux
import app.api as api_mod


@pytest.fixture
def client():
    settings.auth_token = "secret"
    app = FastAPI()

    @app.get("/ping", dependencies=[Depends(require_auth)])
    def ping():
        return {"ok": True}

    with TestClient(app) as c:
        yield c


def test_rejects_without_token(client):
    assert client.get("/ping").status_code == 401


def test_accepts_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200


def test_accepts_cookie(client):
    client.cookies.set("cp_token", "secret")
    r = client.get("/ping")
    assert r.status_code == 200


def test_rejects_wrong_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer wrong"})
    assert r.status_code == 401


def test_rejects_wrong_cookie(client):
    client.cookies.set("cp_token", "wrong")
    r = client.get("/ping")
    assert r.status_code == 401


# ---------------------------------------------------------------------------
# Route tests
# ---------------------------------------------------------------------------
from unittest.mock import patch
from app.models import SessionInfo


@pytest.fixture
def api_client():
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}


def test_list_sessions_route(api_client):
    with patch("app.api.registry.list", return_value=[SessionInfo(name="cc", cwd="/p")]):
        r = api_client.get("/api/sessions", headers=_h())
    assert r.status_code == 200
    assert r.json()[0]["name"] == "cc"


def test_input_eager_send_marks_delivered(api_client):
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp, \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    sp.assert_called_once_with("cc", "oi")
    ap.assert_called_once_with("oi", delivered=True)


def test_input_defer_on_overlay_marks_pending(api_client):
    with patch("app.api.terminal.send_prompt", return_value="deferred"), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    ap.assert_called_once_with("oi", delivered=False)


def test_input_control_char_400_without_queue(api_client):
    with patch("app.api.terminal.send_prompt",
               side_effect=ValueError("control characters not allowed")), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "bad"}, headers=_h())
    assert r.status_code == 400
    ap.assert_not_called()   # validado no send_prompt ANTES de enfileirar


def test_broadcast_invokes_send_once_per_name(api_client):
    # POST /api/broadcast pra N nomes precisa rodar a MESMA sequencia do /input (send_prompt +
    # PromptQueue.append) uma vez por nome — nao um mecanismo de entrega novo.
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp, \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post(
            "/api/broadcast", json={"names": ["a", "b", "c"], "text": "oi"}, headers=_h()
        )
    assert r.status_code == 200
    assert sp.call_count == 3
    assert ap.call_count == 3
    results = r.json()["results"]
    assert set(results.keys()) == {"a", "b", "c"}
    assert all(v["ok"] for v in results.values())


def test_broadcast_reports_per_name_failure_without_aborting_others(api_client):
    def fake_send(name, text):
        if name == "bad":
            raise ValueError("control characters not allowed")
        return "sent"

    with patch("app.api.terminal.send_prompt", side_effect=fake_send), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post(
            "/api/broadcast", json={"names": ["bad", "ok"], "text": "oi"}, headers=_h()
        )
    assert r.status_code == 200
    results = r.json()["results"]
    assert results["bad"]["ok"] is False
    assert results["ok"]["ok"] is True
    ap.assert_called_once_with("oi", delivered=True)   # so a sessao "ok" enfileirou


def test_broadcast_rejects_slash_commands(api_client):
    with patch("app.api.terminal.send_prompt") as sp:
        r = api_client.post("/api/broadcast", json={"names": ["a"], "text": "/clear"}, headers=_h())
    assert r.status_code == 400
    sp.assert_not_called()


def test_select_route(api_client):
    with patch("app.api.terminal.select") as sel:
        r = api_client.post("/api/sessions/cc/select", json={"option": 2}, headers=_h())
    assert r.status_code == 200
    sel.assert_called_once_with("cc", 2)


def test_routes_require_auth(api_client):
    assert api_client.get("/api/sessions").status_code == 401


# ---------------------------------------------------------------------------
# Testes de config dirs (Task 4)
# ---------------------------------------------------------------------------

def test_claude_configs_endpoint(api_client, monkeypatch):
    monkeypatch.setattr(api_mod, "list_config_dirs",
                        lambda: [api_mod.ConfigDirInfo(path="/h/.claude-work", label="work", active=True)])
    r = api_client.get("/api/claude-configs", headers=_h())
    assert r.status_code == 200
    assert r.json() == [{"path": "/h/.claude-work", "label": "work", "active": True}]


# ---------------------------------------------------------------------------
# Testes de _on_hook_transition: pushes de "terminou" (debounce) e "caiu" (Feature #2)
# ---------------------------------------------------------------------------

@pytest.fixture(autouse=False)
def _transition_fixture(monkeypatch):
    """Isola _on_hook_transition: sem tmux real (registry.list vazio) e captura os pushes
    disparados via _notify_async em vez de rodar a thread/rede de verdade."""
    calls = []
    monkeypatch.setattr(api_mod, "_notify_async", lambda sid, fn: calls.append((sid, fn)))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [])
    api_mod._working_started.clear()
    return calls


def test_finish_ping_skipped_on_short_turn(_transition_fixture, monkeypatch):
    calls = _transition_fixture
    monkeypatch.setattr(api_mod.settings, "notify_finished", True)
    monkeypatch.setattr(api_mod.settings, "finish_min_seconds", 45)
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", 1000.0))
    api_mod._on_hook_transition("s1", "working")
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", 1010.0))  # 10s: curto demais
    api_mod._on_hook_transition("s1", "idle")
    assert calls == []


def test_finish_ping_fires_on_long_turn(_transition_fixture, monkeypatch):
    calls = _transition_fixture
    monkeypatch.setattr(api_mod.settings, "notify_finished", True)
    monkeypatch.setattr(api_mod.settings, "finish_min_seconds", 45)
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", 1000.0))
    api_mod._on_hook_transition("s2", "working")
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", 1050.0))  # 50s: dispara
    api_mod._on_hook_transition("s2", "idle")
    assert calls == [("s2", api_mod.push.notify_finished)]


def test_finish_ping_respects_flag(_transition_fixture, monkeypatch):
    calls = _transition_fixture
    monkeypatch.setattr(api_mod.settings, "notify_finished", False)
    monkeypatch.setattr(api_mod.settings, "finish_min_seconds", 45)
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", 1000.0))
    api_mod._on_hook_transition("s3", "working")
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", 1050.0))
    api_mod._on_hook_transition("s3", "idle")
    assert calls == []  # flag desligado -> nunca dispara, mesmo com turno longo


def test_dead_ping_always_fires(_transition_fixture, monkeypatch):
    calls = _transition_fixture
    monkeypatch.setattr(api_mod.settings, "notify_dead", True)
    api_mod._on_hook_transition("s4", "dead")
    assert calls == [("s4", api_mod.push.notify_dead)]


def test_dead_ping_respects_flag(_transition_fixture, monkeypatch):
    calls = _transition_fixture
    monkeypatch.setattr(api_mod.settings, "notify_dead", False)
    api_mod._on_hook_transition("s5", "dead")
    assert calls == []


def test_working_started_cleaned_up_on_idle(_transition_fixture, monkeypatch):
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", 1000.0))
    api_mod._on_hook_transition("s6", "working")
    assert "s6" in api_mod._working_started
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", 1001.0))
    api_mod._on_hook_transition("s6", "idle")
    assert "s6" not in api_mod._working_started


def test_working_started_cleaned_up_on_dead(_transition_fixture, monkeypatch):
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", 1000.0))
    api_mod._on_hook_transition("s7", "working")
    assert "s7" in api_mod._working_started
    api_mod._on_hook_transition("s7", "dead")
    assert "s7" not in api_mod._working_started


# ---------------------------------------------------------------------------
# Feature #12: encadeamento de sessao (_maybe_chain) + kill-switch mestre (automations_enabled)
# ---------------------------------------------------------------------------
from app import chain as chain_mod
from app.chain import ThenLink


@pytest.fixture(autouse=False)
def _tmp_chain_dir(tmp_path, monkeypatch):
    # ThenLink usa o mesmo settings.projects_dir do PromptQueue -> redireciona pro tmp (isola do
    # sidecar real da maquina, mesmo padrao de test_chain.py/test_pqueue.py).
    monkeypatch.setattr(chain_mod.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_maybe_chain_noop_without_link(_tmp_chain_dir, monkeypatch):
    monkeypatch.setattr(api_mod.settings, "automations", True)
    with patch("app.pqueue.PromptQueue.append") as ap, patch("app.api._drain_session") as ds:
        api_mod._maybe_chain("a")
    ap.assert_not_called()
    ds.assert_not_called()


def test_maybe_chain_fires_and_clears_link_once(_tmp_chain_dir, monkeypatch):
    monkeypatch.setattr(api_mod.settings, "automations", True)
    ThenLink("a").set("b", "prossiga")
    with patch("app.pqueue.PromptQueue.append") as ap, patch("app.api._drain_session") as ds:
        api_mod._maybe_chain("a")
    ap.assert_called_once_with("prossiga", delivered=False)
    ds.assert_called_once_with("b")
    assert ThenLink("a").get() is None  # one-shot: o vinculo foi consumido


def test_maybe_chain_master_switch_off_skips_and_keeps_link(_tmp_chain_dir, monkeypatch):
    monkeypatch.setattr(api_mod.settings, "automations", False)
    ThenLink("a").set("b", "prossiga")
    with patch("app.pqueue.PromptQueue.append") as ap, patch("app.api._drain_session") as ds:
        api_mod._maybe_chain("a")
    ap.assert_not_called()
    ds.assert_not_called()
    assert ThenLink("a").get() == {"target": "b", "text": "prossiga"}  # nada consumido -> segue armado


def test_set_then_link_route(api_client, _tmp_chain_dir, monkeypatch):
    from app import tmux
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    r = api_client.put("/api/sessions/a/then", json={"target": "b", "text": "prossiga"}, headers=_h())
    assert r.status_code == 200
    assert ThenLink("a").get() == {"target": "b", "text": "prossiga"}


def test_set_then_link_rejects_self_target(api_client, _tmp_chain_dir):
    r = api_client.put("/api/sessions/a/then", json={"target": "a", "text": "x"}, headers=_h())
    assert r.status_code == 400
    assert ThenLink("a").get() is None


def test_set_then_link_rejects_missing_target_session(api_client, _tmp_chain_dir, monkeypatch):
    from app import tmux
    monkeypatch.setattr(tmux, "has_session", lambda name: False)
    r = api_client.put("/api/sessions/a/then", json={"target": "ghost", "text": "x"}, headers=_h())
    assert r.status_code == 404


def test_clear_then_link_route(api_client, _tmp_chain_dir):
    ThenLink("a").set("b", "x")
    r = api_client.delete("/api/sessions/a/then", headers=_h())
    assert r.status_code == 200
    assert ThenLink("a").get() is None


def test_create_rejects_unknown_config_dir(api_client, monkeypatch):
    monkeypatch.setattr(api_mod, "list_config_dirs",
                        lambda: [api_mod.ConfigDirInfo(path="/h/.claude-work", label="work", active=True)])
    r = api_client.post("/api/sessions", headers=_h(),
                        json={"name": "x", "cwd": "/tmp", "config_dir": "/h/.evil"})
    assert r.status_code == 400


# ---------------------------------------------------------------------------
# Feature #5: corpo rico do push de awaiting (askq -> classify -> fallback) + endpoints de mute/quiet-hours
# ---------------------------------------------------------------------------
from types import SimpleNamespace
from app.models import AskQuestion, AskQuestionItem, AskOption


def test_awaiting_body_prefers_askq(monkeypatch):
    info = SimpleNamespace(name="s1", jsonl="/x/u.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: AskQuestion(questions=[
        AskQuestionItem(header="h", question="Qual branch usar?", options=[AskOption(label="a")]),
    ]))
    assert api_mod._awaiting_body(info) == "Qual branch usar?"


def test_awaiting_body_falls_back_to_classify(monkeypatch):
    import app.state as state_mod
    import app.tmux as tmux_mod
    info = SimpleNamespace(name="s1", jsonl="/x/u.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)  # sem AskUserQuestion nativo
    monkeypatch.setattr(tmux_mod, "capture_pane", lambda name: "pane cru")
    monkeypatch.setattr(state_mod, "classify",
                        lambda pane: ("awaiting_input", None, "Pode sobrescrever o arquivo?", ["a", "b"]))
    assert api_mod._awaiting_body(info) == "Pode sobrescrever o arquivo?"


def test_awaiting_body_fallback_static(monkeypatch):
    import app.state as state_mod
    import app.tmux as tmux_mod
    info = SimpleNamespace(name="s1", jsonl="/x/u.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)
    monkeypatch.setattr(tmux_mod, "capture_pane", lambda name: "pane cru")
    monkeypatch.setattr(state_mod, "classify", lambda pane: ("idle", None, None, None))
    assert api_mod._awaiting_body(info) == "Aguardando sua resposta"


def test_do_notify_awaiting_resolves_name_and_body(monkeypatch):
    calls = []
    info = SimpleNamespace(name="minha-sessao", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "_awaiting_body", lambda i: "corpo rico")
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == [("minha-sessao", "corpo rico")]


def test_do_notify_awaiting_no_match_is_noop(monkeypatch):
    calls = []
    monkeypatch.setattr(api_mod.registry, "list", lambda: [])
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid-nenhuma")
    assert calls == []


def test_push_mute_route(api_client, monkeypatch, tmp_path):
    monkeypatch.setattr(api_mod.push, "_file", lambda: tmp_path / "subs.json")
    r = api_client.post("/api/push/mute", json={"session": "s1", "muted": True}, headers=_h())
    assert r.status_code == 200
    assert api_mod.push.is_muted("s1") is True


def test_push_quiet_hours_route(api_client, monkeypatch, tmp_path):
    monkeypatch.setattr(api_mod.push, "_file", lambda: tmp_path / "subs.json")
    r = api_client.post("/api/push/quiet-hours", json={"start": "22:00", "end": "07:00"}, headers=_h())
    assert r.status_code == 200
    assert api_mod.push.get_push_prefs()["quiet_hours"] == {"start": "22:00", "end": "07:00"}


def test_push_quiet_hours_route_rejects_bad_format(api_client, monkeypatch, tmp_path):
    monkeypatch.setattr(api_mod.push, "_file", lambda: tmp_path / "subs.json")
    r = api_client.post("/api/push/quiet-hours", json={"start": "25:99", "end": "07:00"}, headers=_h())
    assert r.status_code == 422


def test_push_settings_route(api_client, monkeypatch, tmp_path):
    monkeypatch.setattr(api_mod.push, "_file", lambda: tmp_path / "subs.json")
    api_mod.push.set_muted("s1", True)
    r = api_client.get("/api/push/settings", headers=_h())
    assert r.status_code == 200
    assert r.json()["muted"] == ["s1"]


# --- POST /api/archive/{project}/{session_id}/resume: "Retomar conversa" do Arquivo ---

_SID = "11111111-1111-1111-1111-111111111111"


def test_resume_archived_route_derives_name_from_cwd(api_client):
    with patch("app.api.archive_cwd", return_value="/home/u/my-proj"), \
         patch.object(tmux, "has_session", return_value=False), \
         patch("app.api.registry.create",
               return_value=SessionInfo(name="my-proj", cwd="/home/u/my-proj")) as create:
        r = api_client.post(f"/api/archive/-home-u-my-proj/{_SID}/resume", headers=_h())
    assert r.status_code == 200
    assert r.json()["name"] == "my-proj"
    create.assert_called_once_with("my-proj", "/home/u/my-proj", resume_session_id=_SID)


def test_resume_archived_route_suffixes_on_name_collision(api_client):
    # ja existe uma sessao tmux "my-proj" viva -> mesmo esquema de sufixo -2/-3... do CreateSessionSheet.
    with patch("app.api.archive_cwd", return_value="/home/u/my-proj"), \
         patch.object(tmux, "has_session", side_effect=[True, False]), \
         patch("app.api.registry.create",
               return_value=SessionInfo(name="my-proj-2", cwd="/home/u/my-proj")) as create:
        r = api_client.post(f"/api/archive/-home-u-my-proj/{_SID}/resume", headers=_h())
    assert r.status_code == 200
    create.assert_called_once_with("my-proj-2", "/home/u/my-proj", resume_session_id=_SID)


def test_resume_archived_route_422_when_cwd_missing(api_client):
    with patch("app.api.archive_cwd", return_value=None):
        r = api_client.post(f"/api/archive/-home-u-my-proj/{_SID}/resume", headers=_h())
    assert r.status_code == 422


def test_resume_archived_route_404_when_transcript_missing(api_client):
    with patch("app.api.archive_cwd", side_effect=FileNotFoundError()):
        r = api_client.post(f"/api/archive/-home-u-my-proj/{_SID}/resume", headers=_h())
    assert r.status_code == 404
