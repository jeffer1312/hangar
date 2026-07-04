import pytest
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings
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


def test_create_rejects_unknown_config_dir(api_client, monkeypatch):
    monkeypatch.setattr(api_mod, "list_config_dirs",
                        lambda: [api_mod.ConfigDirInfo(path="/h/.claude-work", label="work", active=True)])
    r = api_client.post("/api/sessions", headers=_h(),
                        json={"name": "x", "cwd": "/tmp", "config_dir": "/h/.evil"})
    assert r.status_code == 400
