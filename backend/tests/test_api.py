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


# ---------------------------------------------------------------------------
# Input/broadcast/interrupt por provider (Task 7 — tornar o Codex conversavel)
# ---------------------------------------------------------------------------
from unittest.mock import AsyncMock, MagicMock


def _fake_codex_adapter(deliverable=True, send_result="sent"):
    fake = MagicMock()
    fake.deliverable = AsyncMock(return_value=deliverable)
    fake.send_prompt = AsyncMock(return_value=send_result)
    fake.interrupt = AsyncMock(return_value=True)
    return fake


def test_input_codex_idle_sends_via_adapter(api_client):
    # Sessao Codex ociosa: /input entrega via adapter.send_prompt (turn/start), NAO via terminal
    # (tmux), e registra na fila duravel marcando entregue.
    fake = _fake_codex_adapter(deliverable=True, send_result="sent")
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api.terminal.send_prompt") as term_sp, \
         patch("app.pqueue.PromptQueue.append", return_value={"id": "x1"}) as ap, \
         patch("app.pqueue.PromptQueue.set_delivered") as sd:
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    fake.send_prompt.assert_awaited_once_with("cx", "oi")
    term_sp.assert_not_called()
    ap.assert_called_once()
    sd.assert_called_once_with("x1", True)


def test_input_codex_working_stays_pending(api_client):
    # Turno em andamento (deliverable=False): a entrada fica pendente na fila e NAO chama send_prompt
    # agora — o drain-on-complete entrega quando o turno terminar.
    fake = _fake_codex_adapter(deliverable=False)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", return_value={"id": "x1"}) as ap:
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    fake.send_prompt.assert_not_awaited()
    ap.assert_called_once_with("oi", delivered=False)


def test_input_claude_untouched_by_codex_path(api_client):
    # Caminho Claude intacto: usa terminal.send_prompt e NUNCA toca o adapter Codex.
    fake = _fake_codex_adapter()
    with patch("app.api.terminal.send_prompt", return_value="sent") as term_sp, \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append"):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    term_sp.assert_called_once_with("cc", "oi")
    fake.send_prompt.assert_not_awaited()


def test_broadcast_codex_uses_adapter(api_client):
    fake = _fake_codex_adapter(deliverable=True)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api.terminal.send_prompt") as term_sp, \
         patch("app.pqueue.PromptQueue.append", return_value={"id": "x1"}), \
         patch("app.pqueue.PromptQueue.set_delivered"):
        r = api_client.post("/api/broadcast", json={"names": ["cx1", "cx2"], "text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert fake.send_prompt.await_count == 2
    term_sp.assert_not_called()


def test_interrupt_codex_calls_adapter(api_client):
    fake = _fake_codex_adapter()
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api.terminal.interrupt") as term_int:
        r = api_client.post("/api/sessions/cx/interrupt", headers=_h())
    assert r.status_code == 200
    fake.interrupt.assert_awaited_once_with("cx")
    term_int.assert_not_called()


def test_interrupt_claude_uses_terminal(api_client):
    with patch("app.api.terminal.interrupt") as term_int:
        r = api_client.post("/api/sessions/cc/interrupt", headers=_h())
    assert r.status_code == 200
    term_int.assert_called_once_with("cc", clear=False)


def test_limits_codex_returns_normalized_snapshot(api_client):
    fake = _fake_codex_adapter()
    fake.read_rate_limits = AsyncMock(return_value={
        "limitId": "codex", "limitName": None,
        "primary": {"usedPercent": 42, "windowDurationMins": 10080, "resetsAt": 1784494806},
        "secondary": None, "credits": None, "individualLimit": None,
        "planType": "plus", "rateLimitReachedType": None,
    })
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake):
        r = api_client.get("/api/sessions/cx/limits", headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["primary"] == {"usedPercent": 42, "windowMins": 10080, "resetsAt": 1784494806}
    assert body["secondary"] is None
    assert body["planType"] == "plus"
    fake.read_rate_limits.assert_awaited_once_with("cx")


def test_limits_codex_returns_neutral_when_adapter_has_no_snapshot(api_client):
    # app-server indisponivel/recusou (read_rate_limits devolve None) -> resposta neutra, sem 500.
    fake = _fake_codex_adapter()
    fake.read_rate_limits = AsyncMock(return_value=None)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake):
        r = api_client.get("/api/sessions/cx/limits", headers=_h())
    assert r.status_code == 200
    assert r.json() == {"primary": None, "secondary": None, "planType": None}


def test_limits_claude_rejected_with_400(api_client):
    # Claude nao tem account/rateLimits/read (tem o proprio chip) -> erro claro, nao 500/vazio silencioso.
    fake = _fake_codex_adapter()
    with patch("app.api.get_adapter", return_value=fake):
        r = api_client.get("/api/sessions/cc/limits", headers=_h())
    assert r.status_code == 400
    fake.read_rate_limits.assert_not_called()


# ---------------------------------------------------------------------------
# Modelo + reasoning effort do Codex (Task C) — GET/POST /model(s)
# ---------------------------------------------------------------------------

def test_codex_models_returns_list_and_current(api_client):
    fake = _fake_codex_adapter()
    fake.list_models = AsyncMock(return_value=[{
        "model": "gpt-5-codex", "displayName": "GPT-5 Codex", "description": "padrao",
        "efforts": [{"value": "high", "description": "mais capaz"}], "defaultEffort": "medium",
    }])
    fake.current_model = MagicMock(return_value={"model": "gpt-5-codex", "effort": "high"})
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake):
        r = api_client.get("/api/sessions/cx/models", headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["models"][0]["model"] == "gpt-5-codex"
    assert body["current"] == {"model": "gpt-5-codex", "effort": "high"}
    fake.list_models.assert_awaited_once_with("cx")
    fake.current_model.assert_called_once_with("cx")


def test_codex_models_claude_rejected_with_400(api_client):
    fake = _fake_codex_adapter()
    with patch("app.api.get_adapter", return_value=fake):
        r = api_client.get("/api/sessions/cc/models", headers=_h())
    assert r.status_code == 400


def test_set_codex_model_calls_adapter(api_client):
    fake = _fake_codex_adapter()
    fake.set_model = AsyncMock(return_value=None)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake):
        r = api_client.post(
            "/api/sessions/cx/model", json={"model": "gpt-5-codex", "effort": "high"}, headers=_h()
        )
    assert r.status_code == 200
    assert r.json() == {"ok": True}
    fake.set_model.assert_awaited_once_with("cx", "gpt-5-codex", "high")


def test_set_codex_model_effort_optional(api_client):
    fake = _fake_codex_adapter()
    fake.set_model = AsyncMock(return_value=None)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake):
        r = api_client.post("/api/sessions/cx/model", json={"model": "gpt-5-codex"}, headers=_h())
    assert r.status_code == 200
    fake.set_model.assert_awaited_once_with("cx", "gpt-5-codex", None)


def test_set_codex_model_claude_rejected_with_400(api_client):
    fake = _fake_codex_adapter()
    fake.set_model = AsyncMock(return_value=None)
    with patch("app.api.get_adapter", return_value=fake):
        r = api_client.post("/api/sessions/cc/model", json={"model": "opus"}, headers=_h())
    assert r.status_code == 400
    fake.set_model.assert_not_awaited()


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
# Task 6: POST /api/sessions ramifica por `provider` (default "claude" preserva o caminho de
# hoje). Codex e async (create_codex, mocado -- nao spawna o app-server real); Claude continua
# indo pro registry.create sincrono (agora via asyncio.to_thread, ver docstring do endpoint).
# ---------------------------------------------------------------------------
def test_create_default_provider_routes_to_claude_create(api_client):
    with patch("app.api.registry.create",
              return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = api_client.post("/api/sessions", headers=_h(), json={"name": "x", "cwd": "/tmp"})
    assert r.status_code == 200
    assert r.json()["provider"] == "claude"
    cr.assert_called_once_with("x", "/tmp", None)


def test_create_explicit_claude_provider_routes_to_claude_create(api_client):
    with patch("app.api.registry.create",
              return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "x", "cwd": "/tmp", "provider": "claude"})
    assert r.status_code == 200
    cr.assert_called_once_with("x", "/tmp", None)


def test_create_codex_provider_routes_to_create_codex(api_client):
    from unittest.mock import AsyncMock
    fake = AsyncMock(return_value=SessionInfo(name="cx", cwd="/tmp", provider="codex"))
    with patch("app.api.registry.create_codex", fake), \
         patch("app.api.registry.create") as claude_create:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "cx", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 200
    assert r.json()["provider"] == "codex"
    fake.assert_awaited_once_with("cx", "/tmp")
    claude_create.assert_not_called()   # nao passa pelo caminho tmux/Claude


def test_create_rejects_unknown_provider(api_client):
    with patch("app.api.registry.create") as cr, \
         patch("app.api.registry.create_codex") as cc:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "x", "cwd": "/tmp", "provider": "gemini"})
    assert r.status_code == 400
    cr.assert_not_called()
    cc.assert_not_called()


def test_create_codex_conflict_maps_to_409(api_client):
    from unittest.mock import AsyncMock
    fake = AsyncMock(side_effect=ValueError("ja existe uma sessao com esse nome"))
    with patch("app.api.registry.create_codex", fake):
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "cx", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 409


# ---------------------------------------------------------------------------
# Task 6: /events e /history descobrem o provider da sessao (via registry.list()) e repassam pro
# merged_events / merged_history -- sem isto TODA sessao caia no default "claude" desses helpers
# e o caminho Codex (parser do rollout, Adapter certo) nunca era usado de verdade.
# ---------------------------------------------------------------------------
def test_events_route_passes_session_provider_to_merged_events():
    # Chama a coroutine da rota DIRETO (nao via TestClient): EventSourceResponse so consome o
    # generator ao ser efetivamente streamado, mas abrir a conexao SSE de verdade no TestClient
    # pendura o teste esperando o stream fechar. Confirma so o roteamento (provider certo pro
    # merged_events), que e o que o Task 6 mudou aqui.
    import asyncio
    info = SessionInfo(name="cx", cwd="/tmp", jsonl="/r/cx.jsonl", provider="codex")

    async def _fake_merged_events(name, jsonl, provider="claude"):
        return
        yield  # pragma: no cover -- nunca alcancado; so torna isto um async generator

    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.api.merged_events", side_effect=_fake_merged_events) as me:
        resp = asyncio.run(api_mod.events("cx"))
    assert resp is not None
    me.assert_called_once_with("cx", "/r/cx.jsonl", provider="codex")


def test_history_route_passes_session_provider_to_merged_history(api_client):
    info = SessionInfo(name="cx", cwd="/tmp", jsonl="/r/cx.jsonl", provider="codex")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=[]) as mh:
        r = api_client.get("/api/sessions/cx/history", headers=_h())
    assert r.status_code == 200
    mh.assert_called_once_with("cx", "/r/cx.jsonl", provider="codex")


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


def test_transcribe_route_salva_audio_e_transcreve(api_client, monkeypatch, tmp_path):
    # Wiring do /transcribe: acha a sessao, SALVA o audio no cwd e chama transcribe.
    info = SessionInfo(name="cc", cwd=str(tmp_path))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "transcribe", lambda data, fn: "ola mundo")
    r = api_client.post(
        "/api/sessions/cc/transcribe",
        content=b"\x00audio-bytes",
        headers={**_h(), "X-Filename": "gravacao.webm", "Content-Type": "audio/webm"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["text"] == "ola mundo"
    assert body["path"].endswith(".webm")
    # o audio foi mesmo gravado no disco (pra ser anexado no chat)
    from pathlib import Path
    assert Path(body["path"]).read_bytes() == b"\x00audio-bytes"


def test_transcribe_route_sem_chave_da_503(api_client, monkeypatch, tmp_path):
    info = SessionInfo(name="cc", cwd=str(tmp_path))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(settings, "groq_api_key", "")
    r = api_client.post(
        "/api/sessions/cc/transcribe",
        content=b"audio",
        headers={**_h(), "X-Filename": "a.webm"},
    )
    assert r.status_code == 503


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


# ---------------------------------------------------------------------------
# /answer: fallback por texto quando o drive da TUI falha (DriveError)
# ---------------------------------------------------------------------------
from app import terminal_input as ti_mod


def test_askq_fallback_text_pairs_questions_and_answers(monkeypatch):
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: AskQuestion(questions=[
        AskQuestionItem(header="h1", question="Como validar?", options=[AskOption(label="Type-check")]),
        AskQuestionItem(header="h2", question="Commit + push?", options=[AskOption(label="Sim")]),
    ]))
    text = api_mod._askq_fallback_text(
        [{"kind": "option", "labels": ["Type-check"]}, {"kind": "option", "labels": ["Sim"]}],
        "/x/u.jsonl",
    )
    assert "Como validar? → Type-check" in text and "Commit + push? → Sim" in text


def test_askq_fallback_text_without_sidecar_and_chat_kind(monkeypatch):
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)
    text = api_mod._askq_fallback_text(
        [{"kind": "chat"}, {"kind": "text", "value": "minha resposta"}], "/x/u.jsonl")
    assert "minha resposta" in text and "chat" not in text
    # so chat -> sem texto (o Escape do fallback ja poe o usuario no chat)
    assert api_mod._askq_fallback_text([{"kind": "chat"}], None) == ""


def test_answer_drive_error_falls_back_to_text(api_client):
    # DriveError no drive -> Escape (interrupt) + resposta como texto (_send_one) + 200 fallback:true.
    info = SessionInfo(name="s1", cwd="/x", jsonl="/x/u.jsonl")
    with patch.object(ti_mod, "answer_questions", side_effect=ti_mod.DriveError("nav drift")), \
         patch("app.api.registry.list", return_value=[info]), \
         patch.object(api_mod, "read_pending_askq", return_value=None), \
         patch.object(api_mod.terminal, "interrupt") as intr, \
         patch.object(api_mod, "_send_one", return_value={"ok": True, "error": None}) as send, \
         patch.object(api_mod, "clear_pending_askq") as clear:
        r = api_client.post("/api/sessions/s1/answer", headers=_h(),
                            json={"answers": [{"kind": "option", "indices": [1], "labels": ["Sim"]}]})
    assert r.status_code == 200 and r.json()["fallback"] is True
    intr.assert_called_once_with("s1")
    assert "Sim" in send.call_args[0][1]
    clear.assert_called_once()


def test_answer_validation_error_still_409(api_client):
    with patch.object(ti_mod, "answer_questions", side_effect=ValueError("indices required")), \
         patch("app.api.registry.list", return_value=[]):
        r = api_client.post("/api/sessions/s1/answer", headers=_h(),
                            json={"answers": [{"kind": "option", "labels": []}]})
    assert r.status_code == 409
