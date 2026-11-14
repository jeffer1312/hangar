import time
import pytest
from types import SimpleNamespace
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings
from app import pair, tmux
from app import registry as registry_mod
import app.api as api_mod


@pytest.fixture(autouse=True)
def _pair_dir_isolado(tmp_path, monkeypatch):
    # registry.list() varre pareamento (Task 8) a cada chamada, inclusive dentro de rotas exercitadas
    # via TestClient — sem isto a suite varria o .hangar-pair REAL de quem roda (achado do review).
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    # api.py registra este gancho no import (dispara thread real de _drain_session, que chama
    # registry.list() de novo) — sem neutraliza-lo aqui, um sweep achando ausente de verdade nesta
    # suite dispararia essa thread fora da janela isolada (achado do review de Task 8).


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
from unittest.mock import ANY, patch
from app.models import SessionInfo


@pytest.fixture
def api_client(monkeypatch):
    settings.auth_token = "secret"
    # Guarda de existência do /input//broadcast (sessão morta -> 404): os testes de rota fabricam
    # sessões que não existem no tmux/Codex reais — a guarda não é o que eles testam.
    import app.api as api_mod
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    from app.api import app
    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}


def test_resposta_json_grande_usa_gzip(api_client):
    async def payload():
        return {"texto": "x" * 4096}

    api_client.app.add_api_route("/api/test-gzip", payload)
    try:
        r = api_client.get("/api/test-gzip", headers={**_h(), "Accept-Encoding": "gzip"})
    finally:
        api_client.app.router.routes[:] = [
            route for route in api_client.app.router.routes
            if getattr(route, "path", None) != "/api/test-gzip"
        ]
    assert r.status_code == 200
    assert r.headers["content-encoding"] == "gzip"
    assert "Accept-Encoding" in r.headers["vary"]


def test_input_eager_send_marks_delivered(api_client):
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp, \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    sp.assert_called_once_with("cc", "oi", "claude", pane_id=None)
    # ts=ANY: o _send_one carimba o instante do ENVIO (capturado antes do send_prompt) — o valor e
    # relogio, o que importa aqui e o delivered.
    ap.assert_called_once_with("oi", delivered=True, ts=ANY)


def test_input_defer_on_overlay_marks_pending(api_client):
    with patch("app.api.terminal.send_prompt", return_value="deferred"), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    ap.assert_called_once_with("oi", delivered=False, ts=ANY)


def test_input_deferred_com_fila_indisponivel_falha(api_client):
    # Fila morta (disco cheio/permissao) + send NAO digitou (overlay/picker aberto) = a msg nao esta
    # na TUI nem na fila: sumiu. Antes o OSError era engolido e a rota devolvia 200 "na fila" — o
    # front anunciava entrega de uma msg que nao existia em lugar nenhum.
    with patch("app.api.terminal.send_prompt", return_value="deferred"), \
         patch("app.pqueue.PromptQueue.append", side_effect=OSError("No space left on device")):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    d = r.json()["detail"]
    assert d["code"] == "erro_fila_nao_digitada"
    assert "No space left" in d["params"]["erro"]   # a causa tecnica viaja no params, nao no texto


def test_input_sent_com_fila_indisponivel_ainda_ok(api_client):
    # Caminho oposto: o texto FOI digitado na TUI, entao o envio nao falhou — perder o sidecar so
    # desliga a confirmacao de entrega. 200 continua correto (o erro vai pro log, nao pro usuario).
    with patch("app.api.terminal.send_prompt", return_value="sent"), \
         patch("app.pqueue.PromptQueue.append", side_effect=OSError("No space left on device")), \
         patch("app.api.threading.Timer"):   # nao deixa o Timer de 8.5s (nao-daemon) segurar o exit
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert r.json()["delivered"] is True


def test_input_control_char_400_without_queue(api_client):
    with patch("app.api.terminal.send_prompt",
               side_effect=ValueError("control characters not allowed")), \
         patch("app.pqueue.PromptQueue.append") as ap:
        r = api_client.post("/api/sessions/cc/input", json={"text": "bad"}, headers=_h())
    assert r.status_code == 400
    ap.assert_not_called()   # validado no send_prompt ANTES de enfileirar


def test_input_com_surrogate_solto_e_aceito_e_vai_pro_sidecar(api_client, tmp_path, monkeypatch):
    # Corpo JSON PERFEITAMENTE VÁLIDO com meio emoji escapado — é o que o browser manda quando
    # fatia uma string (UTF-16) no meio de um par antes do JSON.stringify. O json.loads monta o
    # str, o send_keys/write_text estouravam UnicodeEncodeError e a msg do usuário sumia (500 no
    # /input, ou 400 "control characters" vindo do subprocess do tmux). `content=` cru porque nem o
    # httpx consegue serializar um surrogate solto — o corpo já vem escapado, como na rede.
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    with patch("app.api.terminal.send_prompt", return_value="sent"), \
         patch("app.api.threading.Timer"):    # não deixa o Timer de 8.5s segurar o exit
        r = api_client.post("/api/sessions/cc/input", content=rb'{"text": "corte \ud83d"}',
                            headers={**_h(), "Content-Type": "application/json"})
    assert r.status_code == 200 and r.json()["delivered"] is True
    q = pqueue.PromptQueue("cc")
    assert "\ud83d" not in q.path.read_text(encoding="utf-8")   # o arquivo lê de volta
    assert q.load()[0]["text"] == "corte �"


# ---------------------------------------------------------------------------
# _send_one / Pi: entrada da fila ANTES do envio, com id estavel (achado ALTA da revisao 02/08/2026)
# ---------------------------------------------------------------------------


def test_input_partial_com_composer_limpo_diz_que_pode_reenviar(api_client, tmp_path, monkeypatch):
    """A mensagem do 'partial' tem que casar com o que _partial() de fato fez no composer: limpou ->
    dizer que limpou e que pode reenviar, nunca mandar conferir um terminal que ja esta vazio."""
    from app import pqueue, terminal_input
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")

    def fake_send_prompt(name, text, provider="claude", pane_id=None, msg_id=None):
        terminal_input._ULTIMA_LIMPEZA.limpou = True   # o que _partial() teria deixado
        return "partial"

    with patch("app.api.terminal.send_prompt", side_effect=fake_send_prompt):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_envio_incompleto_limpo"
    assert not hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"), "a flag tem que ser apagada apos a leitura"


def test_input_partial_sem_composer_limpo_manda_conferir_terminal(api_client, tmp_path, monkeypatch):
    """Sem confirmacao de limpeza (nao limpou, ou a chamada nem passou por _partial), a mensagem
    conservadora de sempre continua valendo: o residuo pode estar mesmo a vista."""
    from app import pqueue, terminal_input
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")

    def fake_send_prompt(name, text, provider="claude", pane_id=None, msg_id=None):
        terminal_input._ULTIMA_LIMPEZA.limpou = False
        return "partial"

    with patch("app.api.terminal.send_prompt", side_effect=fake_send_prompt):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_envio_incompleto_composer"
    assert not hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou")


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
    def fake_send(name, text, provider="claude", pane_id=None):
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
    ap.assert_called_once_with("oi", delivered=True, ts=ANY)   # so a sessao "ok" enfileirou


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


def test_input_codex_idle_sends_via_adapter(api_client, tmp_path, monkeypatch):
    from app import pqueue

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("cx")
    fake = _fake_codex_adapter(deliverable=True, send_result="sent")

    async def send(name, text):
        assert queue.load()[0]["delivered"] is True
        return "sent"

    fake.send_prompt.side_effect = send
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api.terminal.send_prompt") as term_sp:
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200 and r.json()["delivered"] is True
    fake.send_prompt.assert_awaited_once_with("cx", "oi")
    term_sp.assert_not_called()
    rows = queue.load()
    assert len(rows) == 1 and rows[0]["text"] == "oi" and rows[0]["delivered"] is True


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


def test_input_codex_pending_com_fila_indisponivel_falha(api_client):
    # Espelha o test_input_deferred_com_fila_indisponivel_falha do caminho Claude: turno em andamento
    # (deliverable=False) + sidecar morto = a msg nao esta no Codex nem na fila. O 200 "na fila" era a
    # MESMA mentira que o eeba30a tirou do _send_one e que sobreviveu 3 linhas ao lado, no _send_one_codex.
    fake = _fake_codex_adapter(deliverable=False)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", side_effect=OSError("No space left on device")):
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    d = r.json()["detail"]
    assert d["code"] == "erro_fila_nao_entregue"
    assert "No space left" in d["params"]["erro"]
    fake.send_prompt.assert_not_awaited()


def test_input_codex_entregavel_com_fila_indisponivel_ainda_ok(api_client):
    # Caminho oposto: entregavel -> o turn/start leva o texto, entao perder o sidecar so desliga a
    # rede de seguranca. 200 continua correto (o erro vai pro log, nao pro usuario).
    fake = _fake_codex_adapter(deliverable=True, send_result="sent")
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", side_effect=OSError("No space left on device")), \
         patch("app.pqueue.PromptQueue.set_delivered") as sd:
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert r.json()["delivered"] is True
    fake.send_prompt.assert_awaited_once_with("cx", "oi")
    sd.assert_not_called()   # sem entry na fila nao ha id a marcar


def test_input_codex_deferred_sem_entry_falha(api_client):
    # Ultimo furo do mesmo par: entregavel (nao caiu no erro do append) MAS o send_prompt devolveu
    # "deferred" (corrida idle->working entre o deliverable e o send) com o sidecar morto (entry=None).
    # O texto NAO foi digitado E nao ha entrada pro drain-on-complete drenar: a msg nao esta em lugar
    # nenhum, e o 200 "delivered: false" dizia "ta na fila, calma". Nao esta. Vira erro.
    fake = _fake_codex_adapter(deliverable=True, send_result="deferred")
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", side_effect=OSError("No space left on device")):
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_fila_nao_entregue"


def test_input_codex_deferred_com_entry_fica_pendente(api_client):
    # Contraprova do teste acima: MESMO "deferred", mas com a fila viva -> ha entrada pendente pro
    # drain-on-complete entregar no proximo idle. 200 + delivered:false continua a resposta certa.
    fake = _fake_codex_adapter(deliverable=True, send_result="deferred")
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", return_value={"id": "x1"}), \
         patch("app.pqueue.PromptQueue.set_delivered") as sd:
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert r.json()["delivered"] is False
    sd.assert_not_called()   # deferred nao entregou -> nao pode marcar entregue


def test_input_codex_deliverable_quebrado_nao_passa_calado(api_client, caplog):
    # Adapter fora do ar: o `except Exception` engolia tudo e o "delivered: false" ficava
    # indistinguivel de turno em andamento. Agora loga (e o prompt fica seguro na fila pro drain).
    fake = _fake_codex_adapter()
    fake.deliverable = AsyncMock(side_effect=RuntimeError("app-server morreu"))
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append", return_value={"id": "x1"}):
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert r.json()["delivered"] is False        # nao entregue: fica pendente pro drain-on-complete
    assert "codex deliverable falhou" in caplog.text


def test_input_claude_untouched_by_codex_path(api_client):
    # Caminho Claude intacto: usa terminal.send_prompt e NUNCA toca o adapter Codex.
    fake = _fake_codex_adapter()
    with patch("app.api.terminal.send_prompt", return_value="sent") as term_sp, \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.pqueue.PromptQueue.append"):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    term_sp.assert_called_once_with("cc", "oi", "claude", pane_id=None)
    fake.send_prompt.assert_not_awaited()


def test_broadcast_codex_uses_adapter(api_client, tmp_path, monkeypatch):
    from app import pqueue

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    fake = _fake_codex_adapter(deliverable=True)
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api.terminal.send_prompt") as term_sp:
        r = api_client.post("/api/broadcast", json={"names": ["cx1", "cx2"], "text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert fake.send_prompt.await_count == 2
    term_sp.assert_not_called()
    for name in ("cx1", "cx2"):
        rows = pqueue.PromptQueue(name).load()
        assert len(rows) == 1 and rows[0]["text"] == "oi" and rows[0]["delivered"] is True


def test_routes_require_auth(api_client):
    assert api_client.post("/api/sessions/cc/input", json={"text": "oi"}).status_code == 401


# ---------------------------------------------------------------------------
# Testes de config dirs (Task 4)
# ---------------------------------------------------------------------------


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
    # A mesma guarda do /input (pane tmux, sidecar Codex ou sessão sem terminal) decide o alvo.
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: name != "ghost")
    r = api_client.put("/api/sessions/a/then", json={"target": "ghost", "text": "x"}, headers=_h())
    assert r.status_code == 404


def test_clear_then_link_route(api_client, _tmp_chain_dir):
    ThenLink("a").set("b", "x")
    r = api_client.delete("/api/sessions/a/then", headers=_h())
    assert r.status_code == 200
    assert ThenLink("a").get() is None


@pytest.mark.parametrize("headless, named", [(None, True), (True, True), (None, False)])
def test_create_claude_needs_rust(api_client, headless, named):
    # Claude, com e sem terminal, abre no Rust: sem ele a abertura recusa antes de qualquer efeito.
    # Sem nome também: o comando de terminal lê este código para abrir na máquina.
    body = {"cwd": "/tmp", "provider": "claude"} | ({"name": "x"} if named else {})
    if headless is not None:
        body["headless"] = headless
    r = api_client.post("/api/sessions", headers=_h(), json=body)
    assert r.status_code == 503
    assert r.json()["detail"]["code"] == "session_open_needs_rust_server"


def test_history_route_passes_session_provider_to_merged_history(api_client):
    info = SessionInfo(name="cx", cwd="/tmp", jsonl="/r/cx.jsonl", provider="codex")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=[]) as mh:
        r = api_client.get("/api/sessions/cx/history", headers=_h())
    assert r.status_code == 200
    # posicional: a rota agora repassa via asyncio.to_thread(merged_history, name, jsonl, provider, limit)
    mh.assert_called_once_with("cx", "/r/cx.jsonl", "codex", None)


# ---------------------------------------------------------------------------
# GET /history?limit=N devolve a CAUDA CRUA do transcript. O corte no 1o user_msg (nao desenhar
# resposta orfa) e do card do quadro e vive no BoardCard.svelte: aqui ele valia pra TODO consumidor
# e matava a espiada do hover da Sidebar, que so quer o ultimo assistant_msg da cauda.
# ---------------------------------------------------------------------------
from app.models import ChatEvent


@pytest.fixture
def fake_session_with_transcript(api_client):
    # Transcript de 7 eventos: a cauda (limit=3) cai no meio de um turno — id5 e um assistant_msg cujo
    # prompt ficou de fora da janela. A rota devolve os 3 crus; quem corta e quem desenha bolha.
    evs = [
        ChatEvent(kind="user_msg", id="1", text="oi"),
        ChatEvent(kind="assistant_msg", id="2", text="ola"),
        ChatEvent(kind="tool_use", id="3", tool_name="Bash"),
        ChatEvent(kind="tool_result", id="4", result="ok"),
        ChatEvent(kind="assistant_msg", id="5", text="pensando"),
        ChatEvent(kind="user_msg", id="6", text="valeu"),
        ChatEvent(kind="assistant_msg", id="7", text="de nada"),
    ]
    info = SessionInfo(name="cc", cwd="/p", jsonl="/r/cc.jsonl", provider="claude")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=evs):
        yield "cc"


def test_history_limit_devolve_so_a_cauda(api_client, fake_session_with_transcript):
    full = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history", headers=_h()).json()
    assert len(full) >= 6

    r = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history?limit=3", headers=_h())
    assert r.status_code == 200
    tail = r.json()
    # cauda CRUA: exatamente os N ultimos, sem corte extra
    assert len(tail) == 3
    assert tail == full[-3:]


def test_history_limit_nao_corta_no_user_msg(api_client, fake_session_with_transcript):
    # Regressao da espiada do hover (Sidebar, HP_TAIL=8): o corte no 1o user_msg da janela vivia AQUI e
    # descartava o assistant_msg anterior a ele — com o proximo prompt ja mandado, o popover perdia a
    # resposta e cacheava o vazio por 30s. A cauda de 3 aqui comeca num assistant_msg orfao (id5) e ele
    # TEM que vir; quem o esconde e o BoardCard.
    tail = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history?limit=3",
                          headers=_h()).json()
    assert [e["id"] for e in tail] == ["5", "6", "7"]
    assert tail[0]["kind"] == "assistant_msg"   # orfao preservado: a rota nao decide renderizacao


def test_history_sem_limit_intacto(api_client, fake_session_with_transcript):
    r = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history", headers=_h())
    assert r.status_code == 200
    assert len(r.json()) == 7  # comportamento atual sem regressao


def test_history_limit_zero_e_negativo_devolvem_tudo(api_client, fake_session_with_transcript):
    full = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history", headers=_h()).json()
    for lim in (0, -5):
        r = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history?limit={lim}", headers=_h())
        assert r.status_code == 200
        assert r.json() == full  # limit<=0 nao filtra


def test_history_limit_maior_que_total_devolve_tudo(api_client, fake_session_with_transcript):
    full = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history", headers=_h()).json()
    r = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history?limit=999", headers=_h())
    assert r.status_code == 200
    assert r.json() == full  # limit >= len(evs) -> transcript inteiro


# ---------------------------------------------------------------------------
# GET /history condicional: entrar numa sessao e a leitura mais repetida do app e quase sempre nada
# mudou. O validador (ETag) e o que faz o celular pagar ~200 bytes em vez dos 313 KB da cauda — e o
# que dispensa qualquer regra de "quando invalidar o cache" no cliente.
# ---------------------------------------------------------------------------


@pytest.fixture
def sessao_com_jsonl_real(api_client, tmp_path):
    """Como a fixture acima, mas com um jsonl QUE EXISTE: o validador sai de `stat`, entao sem
    arquivo de verdade nao ha etag nenhum pra testar."""
    jsonl = tmp_path / "cc.jsonl"
    jsonl.write_text('{"a": 1}\n', encoding="utf-8")
    evs = [ChatEvent(kind="user_msg", id="1", text="oi"),
           ChatEvent(kind="assistant_msg", id="2", text="ola")]
    info = SessionInfo(name="cc", cwd="/p", jsonl=str(jsonl), provider="claude")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=evs):
        yield "cc", jsonl


def test_history_com_etag_igual_devolve_304_sem_corpo(api_client, sessao_com_jsonl_real):
    nome, _ = sessao_com_jsonl_real
    r1 = api_client.get(f"/api/sessions/{nome}/history?limit=2", headers=_h())
    assert r1.status_code == 200
    etag = r1.headers.get("etag")
    assert etag

    r2 = api_client.get(f"/api/sessions/{nome}/history?limit=2",
                        headers={**_h(), "If-None-Match": etag})
    assert r2.status_code == 304
    assert r2.content == b""
    assert r2.headers.get("etag") == etag


def test_history_transcript_que_cresceu_nao_serve_304(api_client, sessao_com_jsonl_real):
    nome, jsonl = sessao_com_jsonl_real
    etag = api_client.get(f"/api/sessions/{nome}/history?limit=2", headers=_h()).headers["etag"]
    with jsonl.open("a", encoding="utf-8") as fh:
        fh.write('{"a": 2}\n')
    r = api_client.get(f"/api/sessions/{nome}/history?limit=2",
                       headers={**_h(), "If-None-Match": etag})
    assert r.status_code == 200
    assert r.headers["etag"] != etag


def test_history_fila_durvel_tambem_invalida(api_client, sessao_com_jsonl_real):
    # A resposta funde transcript + sidecar da fila: mensagem enfileirada aparece no historico ANTES
    # de estar no jsonl. Validador so do transcript serviria 304 sobre uma conversa que mudou.
    from app import pqueue
    nome, _ = sessao_com_jsonl_real
    etag = api_client.get(f"/api/sessions/{nome}/history?limit=2", headers=_h()).headers["etag"]
    fila = pqueue._queue_dir() / f"{pqueue._sanitize(nome)}.jsonl"
    fila.write_text('{"text": "na fila"}\n', encoding="utf-8")
    r = api_client.get(f"/api/sessions/{nome}/history?limit=2",
                       headers={**_h(), "If-None-Match": etag})
    assert r.status_code == 200
    assert r.headers["etag"] != etag


def test_history_etag_e_por_representacao(api_client, sessao_com_jsonl_real):
    # `limit` muda a resposta com os mesmos bytes em disco: o etag da cauda nao pode valer como
    # "sem novidade" pro historico inteiro (a fase 2), nem o contrario.
    nome, _ = sessao_com_jsonl_real
    cauda = api_client.get(f"/api/sessions/{nome}/history?limit=2", headers=_h()).headers["etag"]
    inteiro = api_client.get(f"/api/sessions/{nome}/history", headers=_h()).headers["etag"]
    assert cauda != inteiro
    r = api_client.get(f"/api/sessions/{nome}/history",
                       headers={**_h(), "If-None-Match": cauda})
    assert r.status_code == 200


def test_history_sem_transcript_medivel_nao_tem_etag(api_client, fake_session_with_transcript):
    # jsonl inexistente (a fixture aponta pra /r/cc.jsonl): sem `stat` nao ha o que validar. Melhor
    # baixar sempre do que servir 304 sobre um arquivo que nem da pra medir.
    r = api_client.get(f"/api/sessions/{fake_session_with_transcript}/history?limit=3", headers=_h())
    assert r.status_code == 200
    assert "etag" not in {k.lower() for k in r.headers}


# ---------------------------------------------------------------------------
# Feature #5: corpo rico do push de awaiting (askq -> classify -> fallback) + endpoints de mute/quiet-hours
# ---------------------------------------------------------------------------
from types import SimpleNamespace


# --- POST /api/archive/{project}/{session_id}/resume: "Retomar conversa" do Arquivo ---


# --- Retomar conversa CODEX do Arquivo (ticket 08) ---
# A rota recusava com 409. O id da conversa e o uuid do fim do nome do rollout, e e ele que o
# `codex resume` recebe — o mesmo id que o Arquivo ja usa pra listar e abrir a conversa.

# ---------------------------------------------------------------------------
# Loop runner (harness bloco A — task 6)
# ---------------------------------------------------------------------------
from app import loop as loop_mod


# --- Loop goal refiner (claude -p efemero) -----------------------------------
import subprocess as _subprocess


def _completed(returncode=0, stdout="", stderr=""):
    return _subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr=stderr)


@pytest.fixture(autouse=True)
def _claude_resolvido(monkeypatch):
    # O refinador resolve o `claude` pelo PATH; o subprocess e mockado, entao o teste nao pode
    # depender de haver um instalado (o CI nao tem).
    monkeypatch.setattr("app.loop._exe_claude", lambda: "claude")


# --- Nucleo sagrado: caminho de envio isolado do executor default ------------
def test_send_thread_isolated_from_saturated_default_executor():
    # Criterio de aceite (jefferson): com o executor default saturado (simula decoracao/git pendurados),
    # o caminho de envio (_send_thread, pool dedicado) roda MESMO ASSIM — nao disputa recurso com feature.
    import asyncio as _aio
    import threading as _th
    from app import api as api_mod

    async def _run():  # noqa
        release = _th.Event()
        # satura o executor DEFAULT com mais jobs bloqueados que os workers dele
        blockers = [_aio.create_task(_aio.to_thread(release.wait)) for _ in range(64)]
        await _aio.sleep(0.05)   # deixa os blockers ocuparem o pool default
        done = {"ran": False}

        def quick():
            done["ran"] = True
            return "ok"

        # pool DEDICADO -> roda apesar do default saturado
        r = await _aio.wait_for(api_mod._send_thread(quick), timeout=2.0)
        assert r == "ok" and done["ran"]
        release.set()
        await _aio.gather(*blockers)

    _aio.run(_run())


# --- Nucleo: broadcast e group-message tambem no pool dedicado de envio -------
def test_broadcast_uses_send_thread(api_client, monkeypatch):
    # broadcast e trafego hangar-send entre sessoes = nucleo -> tem que ir pelo pool DEDICADO (_send_thread),
    # nao pelo executor default que a decoracao satura.
    from app import api as api_mod
    calls = []

    async def spy(fn, *a):
        calls.append(fn.__name__)
        if fn is api_mod._session_exists:
            return True
        return {"ok": True, "delivered": True}

    monkeypatch.setattr(api_mod, "_send_thread", spy)
    monkeypatch.setattr(api_mod, "_provider_of", lambda n: "claude")
    r = api_client.post("/api/broadcast", json={"names": ["a", "b"], "text": "oi"}, headers=_h())
    assert r.status_code == 200
    # _session_exists e patchado como lambda pelo fixture -> checa o ENVIO real (_send_one) pelas 2 sessoes
    assert calls.count("_send_one") == 2


def test_input_value_error_do_send_vira_envelope(api_client):
    # B3: ValueError do send_prompt (ex.: control chars) -> envelope erro_envio_falhou com a causa
    # em params.erro — antes o detail era a string crua, intraduzivel no front em ingles.
    with patch("app.api._session_exists", return_value=True), \
         patch("app.api.terminal.send_prompt",
               side_effect=ValueError("control characters not allowed")):
        r = api_client.post("/api/sessions/cc/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    d = r.json()["detail"]
    assert d["code"] == "erro_envio_falhou"
    assert d["params"]["erro"] == "control characters not allowed"


def test_input_codex_exception_do_adapter_vira_envelope(api_client):
    # B3: excecao do adapter.send_prompt do Codex -> mesmo envelope.
    fake = _fake_codex_adapter(deliverable=True)
    fake.send_prompt = AsyncMock(side_effect=RuntimeError("adapter broken"))
    with patch("app.api._provider_of", return_value="codex"), \
         patch("app.api.get_adapter", return_value=fake), \
         patch("app.api._session_exists", return_value=True):
        r = api_client.post("/api/sessions/cx/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    d = r.json()["detail"]
    assert d["code"] == "erro_envio_falhou"
    assert d["params"]["erro"] == "adapter broken"
