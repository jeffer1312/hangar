import time
import pytest
from types import SimpleNamespace
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

def test_input_pi_cria_a_entrada_antes_do_envio_com_id_estavel(api_client, tmp_path, monkeypatch):
    """Pra Pi COM LINHA, a entrada da fila tem que existir ANTES do 1o send_prompt, com o id passado
    como msg_id -- e o id que a extensao usa pra reconhecer um retry como o MESMO envio (drain
    reclama a MESMA entrada depois de um 'deferred'). So UMA entrada na fila (nunca um segundo
    append)."""
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr("app.api._pane_info", lambda name: ("pi", "%1"))
    monkeypatch.setattr("app.api.INBOX.tem_linha", lambda pane: True)
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp:
        r = api_client.post("/api/sessions/pisess/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    rows = pqueue.PromptQueue("pisess").load()
    assert len(rows) == 1, "so uma entrada -- append antes, set_delivered depois, nunca um 2o append"
    entry = rows[0]
    assert entry["delivered"] is True
    sp.assert_called_once_with("pisess", "oi", "pi", pane_id="%1", msg_id=entry["id"])


def test_input_pi_deferred_mantem_a_entrada_pendente_com_o_mesmo_id(api_client, tmp_path, monkeypatch):
    """'deferred' (ACK perdido/timeout na linha) -- a entrada fica pendente (delivered=False) pro
    drain reentregar, com o MESMO id que foi usado na 1a tentativa."""
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr("app.api._pane_info", lambda name: ("pi", "%1"))
    monkeypatch.setattr("app.api.INBOX.tem_linha", lambda pane: True)
    with patch("app.api.terminal.send_prompt", return_value="deferred") as sp, \
         patch("app.api.threading.Thread"):   # nao dispara o drain de verdade neste teste
        r = api_client.post("/api/sessions/pisess/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert r.json()["delivered"] is False
    rows = pqueue.PromptQueue("pisess").load()
    assert len(rows) == 1 and rows[0]["delivered"] is False
    sp.assert_called_once_with("pisess", "oi", "pi", pane_id="%1", msg_id=rows[0]["id"])


def test_input_pi_partial_nao_deixa_entrada_orfa_pendente(api_client, tmp_path, monkeypatch):
    """'partial' (fatiamento do Windows) tem que fechar a entrada criada ANTES do envio -- senao ela
    fica delivered=False pra sempre e o proximo drain reentraria digitando em cima do residuo."""
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr("app.api._pane_info", lambda name: ("pi", "%1"))
    monkeypatch.setattr("app.api.INBOX.tem_linha", lambda pane: True)
    with patch("app.api.terminal.send_prompt", return_value="partial"):
        r = api_client.post("/api/sessions/pisess/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 400
    rows = pqueue.PromptQueue("pisess").load()
    assert len(rows) == 1 and rows[0]["delivered"] is True, "fechada, nao fica pendente pro drain"


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


def test_input_pi_slash_command_nao_cria_entrada_antecipada(api_client, tmp_path, monkeypatch):
    """Slash-command nunca entra na fila (nem pra Pi COM linha) -- sem entrada, send_prompt e
    chamado sem msg_id, exatamente como o caminho de sempre."""
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr("app.api._pane_info", lambda name: ("pi", "%1"))
    monkeypatch.setattr("app.api.INBOX.tem_linha", lambda pane: True)
    with patch("app.api.terminal.send_prompt", return_value="sent") as sp:
        r = api_client.post("/api/sessions/pisess/input", json={"text": "/clear"}, headers=_h())
    assert r.status_code == 200
    assert pqueue.PromptQueue("pisess").load() == []
    sp.assert_called_once_with("pisess", "/clear", "pi", pane_id="%1")


def test_input_pi_sem_linha_nao_cria_entrada_antes_do_envio(api_client, tmp_path, monkeypatch):
    """Achado da re-revisao 02/08/2026 (regressao introduzida por este commit): pre-criar a entrada
    ANTES do envio so faz sentido com linha viva -- e o UNICO caso em que o msg_id importa. Sem
    linha, pre-criar abria uma janela de duplo envio pelo TECLADO: o _send_lock so trava DENTRO do
    send_prompt, e o claim_undelivered do drain() usa so o _append_lock da fila (sem relacao com
    aquele) -- um drain() concorrente podia reivindicar a MESMA entrada nessa janela e digitar o
    texto de novo assim que o send_lock liberasse. Prova que, sem linha, a entrada NAO existe no
    instante em que send_prompt roda (a janela nao e alcancavel)."""
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr("app.api._pane_info", lambda name: ("pi", "%1"))
    monkeypatch.setattr("app.api.INBOX.tem_linha", lambda pane: False)   # sem linha -> fallback de tecla

    visto = {}

    def fake_send_prompt(name, text, provider, pane_id=None, **kwargs):
        visto["rows_durante_o_envio"] = pqueue.PromptQueue(name).load()
        visto["kwargs"] = kwargs
        return "sent"

    with patch("app.api.terminal.send_prompt", side_effect=fake_send_prompt):
        r = api_client.post("/api/sessions/pisess/input", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    assert visto["rows_durante_o_envio"] == [], "nenhuma entrada pre-criada sem linha viva"
    assert "msg_id" not in visto["kwargs"], "sem linha nao ha id estavel a oferecer"
    rows = pqueue.PromptQueue("pisess").load()
    assert len(rows) == 1 and rows[0]["delivered"] is True   # so DEPOIS do envio, fluxo de sempre


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


def test_group_message_delivers_to_peers_not_self(api_client):
    # cp-send --group: entrega "[grupo: me] ..." a CADA companheiro, nunca à própria sessão.
    with patch("app.api.PairLink.get", return_value={"peers": ["b", "c"], "task": "", "gid": "g1"}), \
         patch("app.api.terminal.send_prompt", return_value="sent") as sp, \
         patch("app.pqueue.PromptQueue.append"):
        r = api_client.post("/api/sessions/a/group-message", json={"text": "terminei"}, headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["peers"] == ["b", "c"] and body["warning"] is None
    sent_to = [c.args[0] for c in sp.call_args_list]
    assert sorted(sent_to) == ["b", "c"]  # nunca "a"
    assert all(c.args[1] == "[grupo: a] terminei" for c in sp.call_args_list)


def test_group_message_404_when_not_in_group(api_client):
    with patch("app.api.PairLink.get", return_value=None):
        r = api_client.post("/api/sessions/a/group-message", json={"text": "oi"}, headers=_h())
    assert r.status_code == 404


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


def test_select_404_when_session_missing(api_client, monkeypatch):
    # Mesma guarda do /input: sessao morta -> 404, e o terminal.select NEM e chamado. Sem ela a rota
    # respondia {"ok": true} SEMPRE — a cadeia terminal.select -> send_keys -> tmux._run engole a
    # falha do tmux (returncode=1 que ninguem le), entao o "ok" era digitado no vazio.
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: False)
    with patch("app.api.terminal.select") as sel:
        r = api_client.post("/api/sessions/ghost/select", json={"option": 2}, headers=_h())
    assert r.status_code == 404
    assert r.json()["detail"]["code"] == "erro_sessao_opcao_nao_enviada"
    sel.assert_not_called()


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
    from types import SimpleNamespace
    calls = []
    monkeypatch.setattr(api_mod, "_notify_async", lambda sid, fn: calls.append((sid, fn)))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [])
    # O push de "terminou" saiu do caminho sincrono: no Kimi so o `_work` sabe se o "idle" que
    # chegou e de verdade (marcador congelado, ver corrige_ocioso_kimi). `_work` roda em thread —
    # aqui executa em linha pra o teste continuar deterministico.
    monkeypatch.setattr(api_mod.threading, "Thread",
                        lambda target, daemon=None: SimpleNamespace(start=target))
    api_mod._working_started.clear()
    api_mod._recheca_armada.clear()
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
    cr.assert_called_once_with("x", "/tmp", None, provider="claude", engine=None,
                               model=None, effort=None, context_window=None)


def test_create_explicit_claude_provider_routes_to_claude_create(api_client):
    with patch("app.api.registry.create",
              return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "x", "cwd": "/tmp", "provider": "claude"})
    assert r.status_code == 200
    cr.assert_called_once_with("x", "/tmp", None, provider="claude", engine=None,
                               model=None, effort=None, context_window=None)


def test_create_codex_provider_routes_to_create_codex(api_client):
    from unittest.mock import AsyncMock
    fake = AsyncMock(return_value=SessionInfo(name="cx", cwd="/tmp", provider="codex"))
    with patch("app.api.registry.create_codex", fake), \
         patch("app.api.registry.create") as claude_create:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "cx", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 200
    assert r.json()["provider"] == "codex"
    fake.assert_awaited_once_with("cx", "/tmp", None)
    claude_create.assert_not_called()   # nao passa pelo caminho tmux/Claude


def test_create_codex_forwards_wrapper_initial_prompt(api_client):
    from unittest.mock import AsyncMock
    fake = AsyncMock(return_value=SessionInfo(name="cx", cwd="/tmp", provider="codex"))
    with patch("app.api.registry.create_codex", fake):
        r = api_client.post("/api/sessions", headers=_h(), json={
            "name": "cx", "cwd": "/tmp", "provider": "codex",
            "initial_prompt": "revise este projeto",
        })
    assert r.status_code == 200
    fake.assert_awaited_once_with("cx", "/tmp", "revise este projeto")


def test_create_pi_provider_routes_to_claude_create_with_provider(api_client):
    # Pi usa o MESMO registry.create (pane tmux); o provider TEM que chegar la, senao o create
    # spawnaria claude e pre-semearia um transcript do layout errado.
    with patch("app.api.registry.create",
              return_value=SessionInfo(name="p", cwd="/tmp", provider="pi", jsonl=None)) as cr:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "p", "cwd": "/tmp", "provider": "pi"})
    assert r.status_code == 200
    assert r.json()["provider"] == "pi"
    assert r.json()["jsonl"] is None
    cr.assert_called_once_with("p", "/tmp", None, provider="pi", engine=None,
                               model=None, effort=None, context_window=None)


def test_create_pi_with_engine_is_refused(api_client):
    # Motor so faz sentido no Claude: o env do cp-engine e Anthropic-only.
    with patch("app.api.registry.create") as cr:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "p", "cwd": "/tmp", "provider": "pi", "engine": "kimi"})
    assert r.status_code == 400
    cr.assert_not_called()


def test_create_rejects_unknown_provider(api_client):
    with patch("app.api.registry.create") as cr, \
         patch("app.api.registry.create_codex") as cc:
        r = api_client.post("/api/sessions", headers=_h(),
                            json={"name": "x", "cwd": "/tmp", "provider": "gemini"})
    assert r.status_code == 400
    # B3 do parecer task 11: provider fora da lista nao pode reusar o codigo do /api/model-options
    # (que orienta so claude/pi) — a criacao aceita codex/kimi e o texto tem que ser generico.
    assert r.json()["detail"]["code"] == "erro_provider_sessao_invalido"
    cr.assert_not_called()
    cc.assert_not_called()


def test_rename_falha_usa_a_mesma_chave_da_sidebar(api_client):
    # B2 do parecer task 11: a rota de rename nao pode ter chave propria duplicando a traducao
    # que a Sidebar ja usa no fallback dela — o code e o contrato, os dois apontam pra mesma frase.
    with patch("app.tmux.has_session", side_effect=lambda n: n == "cc"), \
         patch("app.tmux.rename_session", return_value=False):
        r = api_client.post("/api/sessions/cc/rename", headers=_h(), json={"new": "cx"})
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "sessao_falha_renomear"


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

    async def _fake_merged_events(name, jsonl, provider="claude", start_offset=None):
        return
        yield  # pragma: no cover -- nunca alcancado; so torna isto um async generator

    req = SimpleNamespace(query_params={}, headers={})
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.api.merged_events", side_effect=_fake_merged_events) as me:
        resp = asyncio.run(api_mod.events("cx", req))
    assert resp is not None
    me.assert_called_once_with("cx", "/r/cx.jsonl", provider="codex", start_offset=None)


def test_events_route_forwards_last_event_id_as_offset():
    # O browser reenvia o Last-Event-ID sozinho na reconexao do EventSource -> vira o offset de
    # retomada do tail. Sem isso a reconexao so tinha o backfill de 200 linhas (~2 min de trabalho
    # pesado) e o miolo de uma queda mais longa se perdia.
    import asyncio
    info = SessionInfo(name="cx", cwd="/tmp", jsonl="/r/cx.jsonl", provider="claude")

    async def _fake(name, jsonl, provider="claude", start_offset=None):
        return
        yield  # pragma: no cover

    def _req(qs=None, hdr=None):
        return SimpleNamespace(query_params=qs or {}, headers=hdr or {})

    def _run(req):
        with patch("app.api.registry.list", return_value=[info]), \
             patch("app.api.merged_events", side_effect=_fake) as me:
            asyncio.run(api_mod.events("cx", req))
        return me.call_args.kwargs["start_offset"]

    # O app fecha e recria o EventSource no proprio retry, e objeto novo nunca manda o header ->
    # o caminho REAL e o query param. Header segue aceito (cliente que use retry nativo).
    assert _run(_req(qs={"last_event_id": "cx:4096"})) == 4096
    assert _run(_req(hdr={"last-event-id": "cx:4096"})) == 4096

    # STEM DE OUTRO TRANSCRIPT (pos-/clear): honrar isso daria seek no meio do arquivo novo e
    # PULARIA calado o inicio da conversa -> tem que cair no backfill.
    assert _run(_req(qs={"last_event_id": "outro-uuid:4096"})) is None
    # id sem stem / lixo / vazio -> backfill, nunca derruba a conexao.
    assert _run(_req(qs={"last_event_id": "4096"})) is None
    assert _run(_req(qs={"last_event_id": "cx:abc"})) is None
    assert _run(_req()) is None


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
    # None = fallback: quem resolve o texto e o push, no idioma da inscricao (nao texto fixo em pt)
    assert api_mod._awaiting_body(info) is None


def test_do_notify_awaiting_resolves_name_and_body(monkeypatch):
    calls = []
    info = SimpleNamespace(name="minha-sessao", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)
    monkeypatch.setattr(api_mod, "_pane_wants_input", lambda name: True)  # menu real no pane
    monkeypatch.setattr(api_mod, "_awaiting_body", lambda i: "corpo rico")
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == [("minha-sessao", "corpo rico")]


def test_do_notify_awaiting_skips_idle_notification(monkeypatch):
    # Notification de "idle 60s" (state_hook mapeia pra awaiting): sem askq pendente e sem menu no
    # pane (nas 2 checagens) -> NENHUM push. Era o push falso "Aguardando sua resposta" em toda
    # sessao parada >60s.
    calls = []
    info = SimpleNamespace(name="parada", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)
    monkeypatch.setattr(api_mod, "_pane_wants_input", lambda name: False)
    monkeypatch.setattr(api_mod, "_AWAITING_PUSH_RETRY_S", 0)
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == []


def test_do_notify_awaiting_retry_catches_late_menu(monkeypatch):
    # Corrida de render: 1a checagem sem menu, retry acha o menu -> push sai (permissao legitima).
    calls = []
    seen = {"n": 0}
    info = SimpleNamespace(name="perm", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: None)

    def _late_menu(name):
        seen["n"] += 1
        return seen["n"] >= 2

    monkeypatch.setattr(api_mod, "_pane_wants_input", _late_menu)
    monkeypatch.setattr(api_mod, "_AWAITING_PUSH_RETRY_S", 0)
    monkeypatch.setattr(api_mod, "_awaiting_body", lambda i: "Pode rodar X?")
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == [("perm", "Pode rodar X?")]


def test_do_notify_awaiting_retry_catches_late_askq(monkeypatch):
    # Sidecar askq pode ser o que atrasa (nao so o render do pane): 1a leitura None, retry acha a
    # pergunta -> push sai. O retry re-le askq ALEM do pane.
    calls = []
    reads = {"n": 0}
    info = SimpleNamespace(name="ask", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])

    def _late_askq(jsonl):
        reads["n"] += 1
        if reads["n"] < 2:
            return None
        return AskQuestion(questions=[
            AskQuestionItem(header="h", question="Qual branch?", options=[AskOption(label="a")]),
        ])

    monkeypatch.setattr(api_mod, "read_pending_askq", _late_askq)
    monkeypatch.setattr(api_mod, "_pane_wants_input", lambda name: False)
    monkeypatch.setattr(api_mod, "_AWAITING_PUSH_RETRY_S", 0)
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == [("ask", "Qual branch?")]


def test_do_notify_awaiting_askq_pending_pushes_without_pane(monkeypatch):
    # AskUserQuestion pendente no sidecar = awaiting real por definicao — push sem depender do pane.
    calls = []
    info = SimpleNamespace(name="ask", jsonl="/x/uuid1.jsonl", cwd="/x")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "read_pending_askq", lambda jsonl: AskQuestion(questions=[
        AskQuestionItem(header="h", question="Qual branch?", options=[AskOption(label="a")]),
    ]))
    monkeypatch.setattr(api_mod, "_pane_wants_input",
                        lambda name: (_ for _ in ()).throw(AssertionError("nao devia capturar pane")))
    monkeypatch.setattr(api_mod.push, "notify_awaiting", lambda name, body: calls.append((name, body)))
    api_mod._do_notify_awaiting("uuid1")
    assert calls == [("ask", "Qual branch?")]


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
    # A chave vem do runtime_config (arquivo editável pela UI), não de settings: sem mockar aqui,
    # a rota usava a chave REAL da máquina e chamava a Groq de verdade — 502, não 503.
    from app import runtime_config
    monkeypatch.setattr(runtime_config, "get", lambda campo: "" if campo == "groq_api_key" else getattr(settings, campo, None))
    r = api_client.post(
        "/api/sessions/cc/transcribe",
        content=b"audio",
        headers={**_h(), "X-Filename": "a.webm"},
    )
    assert r.status_code == 503


def test_transcribe_sem_limpar_nao_chama_a_limpeza(api_client, monkeypatch, tmp_path):
    # Audio ANEXADO (arquivo de dez minutos) passa por esta mesma rota e nao pode pagar limpeza:
    # sem `limpar=1` na query, `narrar.limpar_ditado` nem e chamada, e a resposta e a de sempre.
    info = SessionInfo(name="cc", cwd=str(tmp_path))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "transcribe", lambda data, fn: "ola mundo")
    monkeypatch.setattr(
        api_mod.narrar, "limpar_ditado",
        lambda texto: (_ for _ in ()).throw(AssertionError("nao devia limpar")),
    )
    r = api_client.post(
        "/api/sessions/cc/transcribe",
        content=b"audio",
        headers={**_h(), "X-Filename": "a.webm"},
    )
    assert r.status_code == 200
    assert r.json() == {"path": ANY, "text": "ola mundo"}


def test_transcribe_com_limpar_devolve_o_cru_junto(api_client, monkeypatch, tmp_path):
    # `raw` volta pro botao de desfazer do front; `aviso` explica quando a limpeza nao valeu.
    info = SessionInfo(name="cc", cwd=str(tmp_path))
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "transcribe", lambda data, fn: "ola mundo cru")
    monkeypatch.setattr(api_mod.narrar, "limpar_ditado", lambda texto: ("Olá, mundo.", "aviso teste"))
    r = api_client.post(
        "/api/sessions/cc/transcribe?limpar=1",
        content=b"audio",
        headers={**_h(), "X-Filename": "a.webm"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["text"] == "Olá, mundo."
    assert body["raw"] == "ola mundo cru"
    assert body["aviso"] == "aviso teste"


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
    create.assert_called_once_with("my-proj", "/home/u/my-proj", resume_session_id=_SID, engine=None)


def test_resume_archived_route_suffixes_on_name_collision(api_client):
    # ja existe uma sessao tmux "my-proj" viva -> mesmo esquema de sufixo -2/-3... do CreateSessionSheet.
    with patch("app.api.archive_cwd", return_value="/home/u/my-proj"), \
         patch.object(tmux, "has_session", side_effect=[True, False]), \
         patch("app.api.registry.create",
               return_value=SessionInfo(name="my-proj-2", cwd="/home/u/my-proj")) as create:
        r = api_client.post(f"/api/archive/-home-u-my-proj/{_SID}/resume", headers=_h())
    assert r.status_code == 200
    create.assert_called_once_with("my-proj-2", "/home/u/my-proj", resume_session_id=_SID, engine=None)


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


# ---------------------------------------------------------------------------
# Loop runner (harness bloco A — task 6)
# ---------------------------------------------------------------------------
from app import loop as loop_mod


@pytest.fixture
def loop_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(loop_mod.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def _info(name="cc", cwd="/repo", jsonl="/repo/t.jsonl"):
    return SessionInfo(name=name, cwd=cwd, jsonl=jsonl)


def test_loop_create_404_when_session_missing(api_client, loop_dir):
    with patch("app.api.registry.list", return_value=[]):
        r = api_client.post("/api/sessions/cc/loop", json={"goal": "g"}, headers=_h())
    assert r.status_code == 404


def test_loop_create_409_on_codex_session(api_client, loop_dir):
    # Codex nao e tmux: loop ficaria running mudo pra sempre (sem hook/tick). Recusa cedo.
    info = SessionInfo(name="cx", cwd="/repo", jsonl="/repo/t.jsonl", provider="codex")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.api.automations_enabled", return_value=True):
        r = api_client.post("/api/sessions/cx/loop", json={"goal": "g"}, headers=_h())
    assert r.status_code == 409


def test_loop_create_422_when_max_iters_too_high(api_client, loop_dir):
    with patch("app.api.registry.list", return_value=[_info()]), \
         patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.branch_of", return_value="TICKET-0000"):
        r = api_client.post("/api/sessions/cc/loop",
                            json={"goal": "g", "max_iters": 500}, headers=_h())
    assert r.status_code == 422


def test_loop_create_409_on_main_branch(api_client, loop_dir):
    with patch("app.api.registry.list", return_value=[_info()]), \
         patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.branch_of", return_value="main"):
        r = api_client.post("/api/sessions/cc/loop",
                            json={"goal": "g", "require_branch": True}, headers=_h())
    assert r.status_code == 409


def test_loop_create_ok(api_client, loop_dir):
    calls = []
    with patch("app.api.registry.list", return_value=[_info()]), \
         patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.branch_of", return_value="TICKET-0000"), \
         patch("app.api.drain", side_effect=lambda n, j, p="claude": calls.append((n, j)) or 1):
        r = api_client.post("/api/sessions/cc/loop",
                            json={"goal": "criar ok.txt", "check_cmd": "test -f ok.txt"}, headers=_h())
    assert r.status_code == 200
    d = r.json()["loop"]
    assert d["status"] == "running" and d["goal_entry_id"]
    assert calls  # drain chamado


def test_loop_create_409_when_already_running(api_client, loop_dir):
    loop_mod.LoopLink("cc").set(loop_mod.new_loop("g", None, 10, True))
    with patch("app.api.registry.list", return_value=[_info()]), \
         patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.branch_of", return_value="TICKET-0000"):
        r = api_client.post("/api/sessions/cc/loop", json={"goal": "g"}, headers=_h())
    assert r.status_code == 409


def test_loop_get_none_returns_suggestions(api_client, loop_dir):
    with patch("app.api.registry.list", return_value=[_info(cwd=str(loop_dir))]):
        r = api_client.get("/api/sessions/cc/loop", headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["loop"] is None and isinstance(body["suggestions"], list)


def test_loop_delete_stops(api_client, loop_dir):
    loop_mod.LoopLink("cc").set(loop_mod.new_loop("g", None, 10, True))
    with patch("app.api.registry.list", return_value=[_info()]):
        r = api_client.delete("/api/sessions/cc/loop", headers=_h())
    assert r.status_code == 200
    assert r.json()["loop"]["status"] == "stopped"


def test_loop_resolve_accept(api_client, loop_dir):
    d = loop_mod.new_loop("g", None, 10, True)
    d["status"] = "done_claimed"
    loop_mod.LoopLink("cc").set(d)
    with patch("app.api.registry.list", return_value=[_info()]):
        r = api_client.post("/api/sessions/cc/loop/resolve", json={"accept": True}, headers=_h())
    assert r.status_code == 200 and r.json()["loop"]["status"] == "done"


def test_loop_resolve_409_when_not_done_claimed(api_client, loop_dir):
    loop_mod.LoopLink("cc").set(loop_mod.new_loop("g", None, 10, True))  # running
    with patch("app.api.registry.list", return_value=[_info()]):
        r = api_client.post("/api/sessions/cc/loop/resolve", json={"accept": True}, headers=_h())
    assert r.status_code == 409


def test_loop_resolve_reject_reprompts(api_client, loop_dir):
    d = loop_mod.new_loop("g", None, 10, True)
    d["status"] = "done_claimed"
    d["goal_delivered_ts"] = 1.0
    loop_mod.LoopLink("cc").set(d)
    with patch("app.api.registry.list", return_value=[_info()]), \
         patch("app.api.drain", return_value=1):
        r = api_client.post("/api/sessions/cc/loop/resolve", json={"accept": False}, headers=_h())
    assert r.status_code == 200
    out = r.json()["loop"]
    assert out["status"] == "running" and out["iter"] == 1


# --- Loop goal refiner (claude -p efemero) -----------------------------------
import subprocess as _subprocess


def _completed(returncode=0, stdout="", stderr=""):
    return _subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr=stderr)


def test_loop_refine_ok(api_client):
    with patch("app.api.automations_enabled", return_value=True), \
         patch("app.loop.subprocess.run",
               return_value=_completed(0, stdout="Migre date.ts pra date-fns e mostre o check verde")) as run:
        r = api_client.post("/api/sessions/cc/loop/refine",
                            json={"goal": "arruma as datas", "check_cmd": "npm run check"}, headers=_h())
    assert r.status_code == 200
    assert "date-fns" in r.json()["goal"]
    # argv sem shell, modelo sonnet, tools de efeito colateral negadas (prompt injection headless)
    args = run.call_args[0][0]
    assert args[:4] == ["claude", "-p", "--model", "sonnet"]
    assert "--disallowedTools" in args
    for t in ("Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch"):
        assert t in args
    # prompt vai por STDIN (nunca no argv — --disallowedTools e variadico e engoliria o positional)
    assert "arruma as datas" in run.call_args.kwargs["input"]
    assert all("arruma as datas" not in a for a in args)


def test_loop_refine_409_when_automations_off(api_client):
    with patch("app.api.automations_enabled", return_value=False):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 409


def test_loop_refine_stderr_in_detail(api_client):
    with patch("app.api.automations_enabled", return_value=True), \
         patch("app.loop.subprocess.run",
               return_value=_completed(2, stderr="erro-especifico-do-cli")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 502
    assert "erro-especifico-do-cli" in r.json()["detail"]


def test_loop_refine_timeout_502(api_client):
    with patch("app.loop.subprocess.run",
               side_effect=_subprocess.TimeoutExpired(cmd="claude", timeout=60)):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 502


def test_loop_refine_enoent_502(api_client):
    with patch("app.loop.subprocess.run", side_effect=FileNotFoundError("claude")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 502


def test_loop_refine_nonzero_exit_502(api_client):
    with patch("app.loop.subprocess.run", return_value=_completed(1, stderr="boom")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 502


def test_loop_refine_empty_output_502(api_client):
    with patch("app.loop.subprocess.run", return_value=_completed(0, stdout="   ")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x"}, headers=_h())
    assert r.status_code == 502


def test_loop_refine_422_when_goal_too_long(api_client):
    r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "x" * 2001}, headers=_h())
    assert r.status_code == 422


def test_loop_refine_refusal_is_502(api_client):
    # haiku devolveu pergunta/recusa em vez do objetivo -> guard 502 (nao repassa meta-resposta).
    with patch("app.loop.subprocess.run",
               return_value=_completed(0, stdout="Não consigo reescrever sem saber qual arquivo você quer?")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "arruma o haiku"}, headers=_h())
    assert r.status_code == 502


def test_loop_refine_question_ending_is_502(api_client):
    with patch("app.loop.subprocess.run",
               return_value=_completed(0, stdout="Qual haiku você quer melhorar?")):
        r = api_client.post("/api/sessions/cc/loop/refine", json={"goal": "melhora"}, headers=_h())
    assert r.status_code == 502


# --- ask-history (RAG lexical) -----------------------------------------------
from app.search import SearchHit as _SearchHit


def _hit(name="minha-sessao"):
    return _SearchHit(project="p", session_id="s1", session_name=name, cwd="/c",
                      line="falei de deploy aqui", mtime=1.0, live=True)


def test_ask_history_no_hits_200_empty(api_client):
    with patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.registry.list", return_value=[]), \
         patch("app.api.search_terms", return_value=[]), \
         patch("app.loop.subprocess.run") as run:
        r = api_client.post("/api/ask-history", json={"question": "onde falei de X"}, headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["hits"] == [] and "não achei" in body["answer"]
    run.assert_not_called()  # sem trecho -> nao chama o CLI


def test_ask_history_ok(api_client):
    with patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.registry.list", return_value=[]), \
         patch("app.api.search_terms", return_value=[_hit()]), \
         patch("app.loop.subprocess.run",
               return_value=_completed(0, stdout="Apareceu na sessão minha-sessao, sobre deploy.")):
        r = api_client.post("/api/ask-history", json={"question": "onde falei de deploy"}, headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert "minha-sessao" in body["answer"]
    assert len(body["hits"]) == 1 and body["hits"][0]["session_name"] == "minha-sessao"


def test_ask_history_cli_fail_502(api_client):
    with patch("app.api.automations_enabled", return_value=True), \
         patch("app.api.registry.list", return_value=[]), \
         patch("app.api.search_terms", return_value=[_hit()]), \
         patch("app.loop.subprocess.run", return_value=_completed(1, stderr="boom-ask")):
        r = api_client.post("/api/ask-history", json={"question": "q"}, headers=_h())
    assert r.status_code == 502
    assert "boom-ask" in r.json()["detail"]


def test_ask_history_409_when_automations_off(api_client):
    with patch("app.api.automations_enabled", return_value=False):
        r = api_client.post("/api/ask-history", json={"question": "q"}, headers=_h())
    assert r.status_code == 409


def test_ask_history_422_when_question_too_long(api_client):
    r = api_client.post("/api/ask-history", json={"question": "x" * 501}, headers=_h())
    assert r.status_code == 422


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
    # broadcast e trafego cp-send entre sessoes = nucleo -> tem que ir pelo pool DEDICADO (_send_thread),
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


def test_group_message_uses_send_thread(api_client, monkeypatch):
    from app import api as api_mod
    calls = []

    async def spy(fn, *a):
        calls.append(fn.__name__)
        if fn is api_mod._session_exists:
            return True
        return {"ok": True, "delivered": True}

    monkeypatch.setattr(api_mod, "_send_thread", spy)
    monkeypatch.setattr(api_mod, "_provider_of", lambda n: "claude")
    monkeypatch.setattr(api_mod.PairLink, "get", lambda self: {"peers": ["peer1"]})
    r = api_client.post("/api/sessions/lider/group-message", json={"text": "marco"}, headers=_h())
    assert r.status_code == 200
    assert "_send_one" in calls


# ---------------------------------------------------------------------------
# Modelo + nivel de raciocinio do Pi — GET/POST /pi/model(s)
# ---------------------------------------------------------------------------

_PI_CAT = {
    "current": {"provider": "kimi-coding", "id": "k3", "name": "Kimi K3"},
    "thinking": "low",
    "levels": ["low", "high", "max"],
    "models": [{"provider": "kimi-coding", "id": "k3", "name": "Kimi K3", "reasoning": True}],
}


def _pi_info():
    return SessionInfo(name="pp", cwd="/p", jsonl="/p/ts_uuid.jsonl", provider="pi")


def test_pi_models_returns_catalog(api_client):
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=_PI_CAT), \
         patch("app.api._session_config_dir", return_value=None):
        r = api_client.get("/api/sessions/pp/pi/models", headers=_h())
    assert r.status_code == 200
    assert r.json()["levels"] == ["low", "high", "max"]
    assert r.json()["current"]["id"] == "k3"


def test_pi_models_claude_session_rejected_with_400(api_client):
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))):
        r = api_client.get("/api/sessions/cc/pi/models", headers=_h())
    assert r.status_code == 400


def test_pi_models_missing_sidecar_is_409_not_empty_list(api_client):
    # Extensao ausente/velha: falha ALTA com instrucao — nunca um catalogo vazio que parece "sem modelos".
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=None), \
         patch("app.api._session_config_dir", return_value=None):
        r = api_client.get("/api/sessions/pp/pi/models", headers=_h())
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_catalogo_pi_indisponivel"


def test_pi_model_set_sends_both_commands_and_reports_readback(api_client):
    # `xhigh` FORA dos levels do modelo novo: e o caso real do clamp (o Pi aterrissa em high).
    after = {**_PI_CAT, "current": {"provider": "clinepass", "id": "cline-pass/glm-5.2"},
             "thinking": "high", "levels": ["off", "low", "medium", "high"], "ts": 2.0}
    cat = {**_PI_CAT, "ts": 1.0, "models": _PI_CAT["models"] + [
        {"provider": "clinepass", "id": "cline-pass/glm-5.2", "name": "GLM", "reasoning": True}]}
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", side_effect=[cat, after]), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.terminal.send_pi_commands") as send:
        r = api_client.post("/api/sessions/pp/pi/model", headers=_h(),
                            json={"provider": "clinepass", "model": "cline-pass/glm-5.2",
                                  "effort": "xhigh"})
    assert r.status_code == 200
    send.assert_called_once_with(
        "pp", ["/cp-model clinepass cline-pass/glm-5.2", "/cp-think xhigh"])
    # O Pi clampa: pedimos xhigh, o readback e quem manda no que a UI mostra.
    assert r.json()["thinking"] == "high"


def test_pi_model_set_recusado_pelo_pi_vira_409(api_client):
    # `/cp-model` sem chave pro provedor: setModel devolve false, o Pi notifica DENTRO do TUI e
    # republica o catalogo com o modelo VELHO. Antes disto o app fechava a folha dizendo ok=True.
    cat = {**_PI_CAT, "ts": 1.0, "models": _PI_CAT["models"] + [
        {"provider": "clinepass", "id": "cline-pass/glm-5.2", "name": "GLM", "reasoning": True}]}
    after = {**cat, "ts": 2.0}                     # ts novo (processou), modelo o mesmo (recusou)
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=cat), \
         patch("app.api.pi_models.read_back", return_value=after), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.terminal.send_pi_commands"):
        r = api_client.post("/api/sessions/pp/pi/model", headers=_h(),
                            json={"provider": "clinepass", "model": "cline-pass/glm-5.2"})
    assert r.status_code == 409
    d = r.json()["detail"]
    assert d["code"] == "erro_pi_recusou_troca"
    assert d["params"]["provider"] == "kimi-coding" and d["params"]["id"] == "k3"  # diz onde a sessao FICOU


def test_pi_model_set_sem_republicacao_diz_que_nao_confirmou(api_client):
    # Sidecar parado no MESMO ts: o comando pode nem ter chegado — nao da pra chamar de recusa.
    cat = {**_PI_CAT, "ts": 1.0, "models": _PI_CAT["models"] + [
        {"provider": "clinepass", "id": "cline-pass/glm-5.2", "name": "GLM", "reasoning": True}]}
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=cat), \
         patch("app.api.pi_models.read_back", return_value=cat), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.terminal.send_pi_commands"):
        r = api_client.post("/api/sessions/pp/pi/model", headers=_h(),
                            json={"provider": "clinepass", "model": "cline-pass/glm-5.2"})
    assert r.status_code == 409
    d = r.json()["detail"]
    assert d["code"] == "erro_sem_confirmacao_troca"


def test_pi_model_set_rejects_model_outside_catalog(api_client):
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=_PI_CAT), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.terminal.send_pi_commands") as send:
        r = api_client.post("/api/sessions/pp/pi/model", headers=_h(),
                            json={"provider": "kimi-coding", "model": "k9"})
    assert r.status_code == 422
    send.assert_not_called()


def test_pi_model_set_requires_something_to_change(api_client):
    with patch("app.api._cached_info", AsyncMock(return_value=_pi_info())), \
         patch("app.api.pi_models.read_catalog", return_value=_PI_CAT), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.terminal.send_pi_commands") as send:
        r = api_client.post("/api/sessions/pp/pi/model", headers=_h(), json={})
    assert r.status_code == 422
    send.assert_not_called()


def test_plan_pin_rejeita_stem_com_travessia(api_client, tmp_path):
    # Sem a guarda de separador, o proprio os.path.isfile do endpoint responde se existe um .md
    # FORA da pasta de planos (404 vs 200) — e o stem com barra ainda ia parar no arquivo de pin,
    # onde o read_pin depois descarta calado.
    fora = tmp_path / "segredo.md"
    fora.write_text("x", encoding="utf-8")
    planos = tmp_path / "docs" / "superpowers" / "plans"
    planos.mkdir(parents=True)
    with patch("app.api._session_cwd", return_value=str(tmp_path)), \
         patch("app.api._plans_dir", return_value=str(planos)), \
         patch("app.api.write_pin") as wp:
        r = api_client.post("/api/sessions/cc/plan-pin", headers=_h(),
                            json={"stem": "../../../segredo"})
    assert r.status_code == 400
    wp.assert_not_called()


def test_plan_pin_aceita_plano_da_propria_raiz(api_client, tmp_path):
    planos = tmp_path / "docs" / "superpowers" / "plans"
    planos.mkdir(parents=True)
    (planos / "2026-07-30-x.md").write_text("# x", encoding="utf-8")
    with patch("app.api._session_cwd", return_value=str(tmp_path)), \
         patch("app.api._plans_dir", return_value=str(planos)), \
         patch("app.api.write_pin") as wp:
        r = api_client.post("/api/sessions/cc/plan-pin", headers=_h(),
                            json={"stem": "2026-07-30-x"})
    assert r.status_code == 200
    wp.assert_called_once_with(str(planos), "2026-07-30-x")


# ---------------------------------------------------------------------------
# Catalogo de modelos da sessao Claude — GET /model/options, POST /engine/model
# ---------------------------------------------------------------------------

from app import model_picker as mp

_ENGINE_CAT = [
    {"id": "k3", "context_length": 1048576, "vision": True},
    {"id": "kimi-for-coding", "context_length": 262144, "vision": True},
]


def _engine_info():
    return SessionInfo(name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude", engine="kimi")


@pytest.fixture
def api_client_limpo(api_client):
    # O cache do catalogo e por processo e vive 5 min: sem limpar, um teste leria o do anterior.
    from app import api as api_mod
    api_mod._engine_models_cache.clear()
    api_mod._claude_models_cache.clear()
    yield api_client
    api_mod._engine_models_cache.clear()
    api_mod._claude_models_cache.clear()


def test_model_options_engine_vem_do_provedor_nao_do_picker(api_client_limpo):
    # Numa sessao de motor o picker do CC so lista os 4 aliases (todos o mesmo ANTHROPIC_MODEL):
    # a lista util e a do /v1/models. O terminal NAO pode ser tocado nesse caminho.
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT) as probe, \
         patch("app.api.terminal.list_model_options") as picker:
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["kind"] == "engine" and body["engine"] == "kimi"
    assert [m["id"] for m in body["models"]] == ["k3", "kimi-for-coding"]
    picker.assert_not_called()
    probe.assert_called_once()


def test_model_options_engine_ignora_painel_aberto(api_client_limpo, monkeypatch):
    # O ramo de motor fala com o provedor por HTTP (_engine_models) — nao le o pane, entao o painel
    # anexado nao pode derrubar esta listagem (achado da revisao da Task 3).
    from app import termsock
    monkeypatch.setitem(termsock._ativos, "cc", object())
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT):
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    assert r.status_code == 200
    assert r.json()["kind"] == "engine"


def test_model_options_conta_anthropic_recusa_com_painel_aberto(api_client_limpo, monkeypatch):
    # O ramo da conta Anthropic dirige o picker do /model contando linha do pane -- este SIM tem
    # que recusar com o painel aberto.
    from app import termsock
    monkeypatch.setitem(termsock._ativos, "cc", object())
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))), \
         patch("app.api.terminal.list_model_options") as picker:
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    assert r.status_code == 409
    picker.assert_not_called()


def test_model_options_conta_anthropic_le_o_picker_ao_vivo(api_client_limpo):
    # A lista muda com a conta e com a versao do CC (o Fable entrou e a lista chumbada no front nao
    # soube): ela vem das linhas lidas, nunca de constante.
    lido = {"effort": "high", "models": [
        {"keyword": "default", "id": "default", "name": "Default", "desc": "Opus 5 …", "active": False},
        {"keyword": "fable", "id": "fable", "name": "Fable", "desc": "Fable 5 …", "active": True},
    ]}
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))), \
         patch("app.api.terminal.list_model_options", return_value=lido):
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    assert r.status_code == 200
    body = r.json()
    assert body["kind"] == "claude" and body["effort"] == "high"
    assert [m["id"] for m in body["models"]] == ["default", "fable"]
    assert body["models"][1]["active"] is True


def test_model_options_nao_repete_id_com_as_duas_linhas_opus(api_client_limpo):
    """O picker desta máquina tem DUAS linhas `opus` (a normal e a de 1M). Enquanto a rota mandava a
    keyword como id, os dois ids vinham iguais: a lista da tela morria em `each_key_duplicate` e
    escolher a de 1M aplicava o Opus normal."""
    lido = {"effort": "high", "models": [
        {"keyword": "opus", "id": "opus", "name": "Opus",
         "desc": "Opus 5 · Best for everyday", "active": False},
        {"keyword": "opus", "id": "opus[1m]", "name": "Opus (1M context)",
         "desc": "Opus 5 with 1M context", "active": True},
    ]}
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))), \
         patch("app.api.terminal.list_model_options", return_value=lido):
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    ids = [m["id"] for m in r.json()["models"]]
    assert ids == ["opus", "opus[1m]"]
    assert len(set(ids)) == len(ids)


def test_model_options_sessao_ocupada_propaga_409(api_client_limpo):
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))), \
         patch("app.api.terminal.list_model_options",
               side_effect=mp.PickerError(409, "a sessao esta trabalhando — espere ela terminar")):
        r = api_client_limpo.get("/api/sessions/cc/model/options", headers=_h())
    assert r.status_code == 409
    assert "trabalhando" in r.json()["detail"]


def test_engine_model_set_restaura_o_default_global(api_client_limpo):
    # O ponto da rota: `/model <id>` grava o id como default GLOBAL. O valor anterior volta pro
    # settings.json depois — a troca vale na sessao e em lugar nenhum mais.
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.default_model.snapshot", return_value="claude-opus-5"), \
         patch("app.api.default_model.restore_quando_aterrissar") as restore, \
         patch("app.api.terminal.set_engine_model",
               return_value={"ok": True, "result": "Set model to kimi-for-coding"}) as drv:
        r = api_client_limpo.post("/api/sessions/cc/engine/model", headers=_h(),
                            json={"model": "kimi-for-coding"})
    assert r.status_code == 200
    drv.assert_called_once_with("cc", "kimi-for-coding")
    restore.assert_called_once_with(None, "claude-opus-5")


def test_engine_model_set_restaura_mesmo_quando_o_driver_falha(api_client_limpo):
    # O comando pode ter sido digitado ANTES da falha de leitura: deixar o default global vazado
    # por causa de um erro seria a pior combinacao.
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.default_model.snapshot", return_value="claude-opus-5"), \
         patch("app.api.default_model.restore_quando_aterrissar") as restore, \
         patch("app.api.terminal.set_engine_model", side_effect=mp.PickerError(409, "sem confirmacao")):
        r = api_client_limpo.post("/api/sessions/cc/engine/model", headers=_h(),
                            json={"model": "kimi-for-coding"})
    assert r.status_code == 409
    restore.assert_called_once_with(None, "claude-opus-5")


def test_engine_model_set_recusa_modelo_fora_do_catalogo(api_client_limpo):
    # Digitar o id assim mesmo faria a sessao mandar request pra um modelo inexistente e a falha
    # apareceria so no proximo turno.
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT), \
         patch("app.api.terminal.set_engine_model") as drv:
        r = api_client_limpo.post("/api/sessions/cc/engine/model", headers=_h(),
                            json={"model": "gpt-5.4"})
    assert r.status_code == 422
    drv.assert_not_called()


def test_engine_model_set_recusa_sessao_sem_motor(api_client):
    with patch("app.api._cached_info", AsyncMock(return_value=SessionInfo(
            name="cc", cwd="/p", jsonl="/p/a.jsonl", provider="claude"))):
        r = api_client.post("/api/sessions/cc/engine/model", headers=_h(), json={"model": "k3"})
    assert r.status_code == 400


def test_engine_model_set_recusado_antes_de_digitar_nao_espera_a_escrita(api_client_limpo):
    # Sessao ocupada: o terminal ficou intocado, entao esperar ~3.6s pela escrita do settings.json
    # so faria o erro demorar a aparecer na tela.
    from app.terminal_input import TerminalInput
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT), \
         patch("app.api._session_config_dir", return_value=None), \
         patch("app.api.default_model.snapshot", return_value="claude-opus-5"), \
         patch("app.api.default_model.restore_quando_aterrissar") as restore, \
         patch("app.api.terminal.set_engine_model",
               side_effect=TerminalInput.NaoDigitou(409, "a sessao esta trabalhando")):
        r = api_client_limpo.post("/api/sessions/cc/engine/model", headers=_h(),
                                  json={"model": "k3"})
    assert r.status_code == 409
    restore.assert_not_called()


def test_engine_model_set_valida_contra_catalogo_FRESCO(api_client_limpo):
    # O cache de 5 min existe pra folha abrir rapido. Se a validacao da troca usasse ele, um modelo
    # tirado do plano passaria e a falha apareceria so no proximo turno — exatamente o que o check
    # promete evitar.
    with patch("app.api._cached_info", AsyncMock(return_value=_engine_info())), \
         patch("app.api.engines.listar", return_value={"kimi": {"base_url": "https://x", "api_key": "k"}}), \
         patch("app.api.engine_probe.listar_modelos", return_value=_ENGINE_CAT) as probe:
        # 1) esquenta o cache pela rota de listagem
        assert api_client_limpo.get("/api/sessions/cc/model/options", headers=_h()).status_code == 200
        assert probe.call_count == 1
        # 2) o provedor tirou o k3 do plano
        probe.return_value = [m for m in _ENGINE_CAT if m["id"] != "k3"]
        with patch("app.api.terminal.set_engine_model") as drv:
            r = api_client_limpo.post("/api/sessions/cc/engine/model", headers=_h(),
                                      json={"model": "k3"})
    assert probe.call_count == 2          # nao reusou o cache
    assert r.status_code == 422
    drv.assert_not_called()


# 13/08/2026 — o "idle" mentiroso do Kimi no caminho de ACAO
# ---------------------------------------------------------------------------
# Um turno Kimi que comeca a partir de um prompt enfileirado na TUI nao dispara evento nenhum, entao
# o marcador fica congelado no idle do turno ANTERIOR enquanto o novo roda. As leituras (lista e
# chat) ja corrigem isso; aqui esta o caminho que MEXE: sem a checagem, o loop re-prompta uma sessao
# ocupada e o vinculo `then` e CONSUMIDO — e como o Stop real grava idle SOBRE idle, o
# hook_state._apply nao avisa ninguem e o fim de turno de verdade nunca dispararia o `then` de novo.
_AGORA = time.time()   # ts reais: o mtime do wire e comparado com o ts do marcador do hook


def _sessao_kimi(tmp_path, nome="k1", mtime=None):
    import os
    from types import SimpleNamespace
    j = tmp_path / "wire.jsonl"
    j.write_text("{}\n", encoding="utf-8")
    if mtime is not None:
        os.utime(j, (mtime, mtime))
    return SimpleNamespace(name=nome, jsonl=str(j), provider="kimi")


def _rodar_transicao_idle(monkeypatch, tmp_path, mtime_do_wire, marcador_ts, limpar=True):
    """Dispara _on_hook_transition(sid, 'idle') com o wire escrito em `mtime_do_wire` e o marcador
    gravado em `marcador_ts`. Devolve (chamou_chain, chamou_tick, reagendou)."""
    from types import SimpleNamespace
    info = _sessao_kimi(tmp_path, mtime=mtime_do_wire)
    monkeypatch.setattr(api_mod.registry, "list", lambda: [info])
    monkeypatch.setattr(api_mod, "session_key", lambda p: "sid-k")
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", marcador_ts))
    monkeypatch.setattr(api_mod, "drain", lambda *a, **k: 0)
    monkeypatch.setattr(api_mod.settings, "notify_finished", False)
    chain, tick, timers = [], [], []
    monkeypatch.setattr(api_mod, "_maybe_chain", lambda n: chain.append(n))
    monkeypatch.setattr(api_mod.loop_mod, "schedule_tick", lambda n, ctx: tick.append(n))
    monkeypatch.setattr(api_mod.loop_mod, "LoopLink",
                        lambda n: SimpleNamespace(get=lambda: {"status": "running"}))
    monkeypatch.setattr(api_mod.threading, "Timer",
                        lambda *a, **k: (timers.append(a), SimpleNamespace(start=lambda: None))[1])
    # _work roda numa thread; aqui executa o alvo direto pra o teste ser deterministico.
    monkeypatch.setattr(api_mod.threading, "Thread",
                        lambda target, daemon=None: SimpleNamespace(start=target))
    if limpar:
        api_mod._recheca_armada.clear()
    api_mod._on_hook_transition("sid-k", "idle")
    return chain, tick, timers


def test_idle_falso_do_kimi_nao_dispara_automacao(monkeypatch, tmp_path):
    # wire escrito 90s DEPOIS do marcador ocioso = turno andando.
    chain, tick, timers = _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA + 90, _AGORA)
    assert chain == []                        # `then` NAO e consumido no meio do turno
    assert tick == []                         # loop NAO re-prompta sessao ocupada
    # reagenda a reavaliacao: sem isto o fim de turno real (idle sobre idle) nao gera transicao
    # nenhuma e a fila nunca seria drenada.
    assert any(a[1] is api_mod._recheca_kimi for a in timers)


def test_idle_de_verdade_do_kimi_segue_disparando(monkeypatch, tmp_path):
    # O contrario, pra a correcao nao matar a feature: wire e marcador no mesmo instante = parada.
    chain, tick, timers = _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA, _AGORA)
    assert tick == ["k1"]                     # loop ativo + sent == 0 -> tica
    assert chain == []                        # com loop ativo o chain fica suprimido (regra antiga)
    assert not any(a[1] is api_mod._recheca_kimi for a in timers)   # nada a reavaliar


def test_recheca_do_kimi_e_uma_cadeia_por_sessao(monkeypatch, tmp_path):
    # Cada transicao espuria abria a SUA corrente de Timers, e duas correntes em paralelo dobram o
    # `registry.list()` (que toca tmux) a cada 5s, pra sempre e sem ninguem notar.
    _, _, t1 = _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA + 90, _AGORA)
    assert sum(1 for a in t1 if a[1] is api_mod._recheca_kimi) == 1
    # 2a transicao com a cadeia JA armada (o _rodar_ limpa o set, entao arma na mao aqui)
    api_mod._armar_recheca("sid-k")
    _, _, t2 = _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA + 90, _AGORA, limpar=False)
    assert not any(a[1] is api_mod._recheca_kimi for a in t2)   # nao abre a segunda corrente


def test_push_terminou_nao_sai_no_idle_falso_do_kimi(monkeypatch, tmp_path):
    # O push de "terminou" era o unico dos quatro que ficava fora do `_work` — avisava "terminou"
    # com a sessao escrevendo codigo, e ainda consumia o inicio do turno, entao o fim REAL vinha sem
    # saber ha quanto tempo ela trabalhava e o debounce de turno longo nunca mais disparava.
    monkeypatch.setattr(api_mod.runtime_config, "get",
                        lambda k: True if k == "notify_finished" else 1)
    avisos = []
    monkeypatch.setattr(api_mod, "_notify_async", lambda sid, fn: avisos.append(sid))
    api_mod._working_started["sid-k"] = _AGORA - 300      # turno longo em andamento
    _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA + 90, _AGORA)
    assert avisos == []                                   # nada de "terminou" no meio do turno
    assert "sid-k" in api_mod._working_started            # o inicio do turno NAO foi consumido

    # E o fim de verdade (transcript parado) avisa, com a duracao inteira preservada.
    _rodar_transicao_idle(monkeypatch, tmp_path, _AGORA, _AGORA)
    assert avisos == ["sid-k"]
    assert "sid-k" not in api_mod._working_started


def test_push_terminou_nao_consome_o_inicio_de_outro_turno(monkeypatch):
    # Corrida real: entre o inicio do `_work` e o push rodam registry.list() e drain(), e o proprio
    # drain pode largar um prompt novo — a sessao volta a "working" e o caminho sincrono grava o
    # inicio do turno NOVO. Um `pop` cego levaria embora esse valor: o push deste turno sairia com
    # duracao errada e o turno seguinte, ao acabar de verdade, acharia `started is None` e nunca
    # avisaria.
    monkeypatch.setattr(api_mod.runtime_config, "get",
                        lambda k: True if k == "notify_finished" else 1)
    avisos = []
    monkeypatch.setattr(api_mod, "_notify_async", lambda sid, fn: avisos.append(sid))
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("idle", _AGORA))

    api_mod._working_started["s9"] = _AGORA - 300          # turno NOVO ja registrado
    api_mod._push_terminou("s9", _AGORA - 999)             # push do turno VELHO chega atrasado
    assert avisos == []                                    # nao e dele pra consumir
    assert api_mod._working_started["s9"] == _AGORA - 300   # o do turno novo continua intacto

    api_mod._push_terminou("s9", _AGORA - 300)             # agora sim, o dono certo
    assert avisos == ["s9"]
    assert "s9" not in api_mod._working_started


def test_working_started_e_protegido_dos_dois_lados(monkeypatch):
    # O compare-and-delete de `_push_terminou` so vale se os ESCRITORES pegarem o mesmo lock: quem
    # grava o inicio do turno roda no laco do hook_state.watch e quem consome roda numa thread de
    # `_work`. Protegido de um lado so, o produtor passa entre o get e o del — e o turno seguinte
    # termina com `started is None`, sem aviso nenhum e sem rastro.
    import threading as _th
    monkeypatch.setattr(api_mod.runtime_config, "get",
                        lambda k: True if k == "notify_finished" else 1)
    monkeypatch.setattr(api_mod, "_notify_async", lambda sid, fn: None)
    monkeypatch.setattr(api_mod.hook_state, "get_state", lambda sid: ("working", _AGORA))

    api_mod._working_started.clear()
    erros = []

    def escrever():           # o produtor: hook "working" chegando em rajada
        try:
            for i in range(300):
                api_mod._on_hook_transition("s-lock", "working")
        except Exception as e:      # noqa: BLE001 — o teste quer QUALQUER falha da corrida
            erros.append(e)

    def consumir():           # o consumidor: fim de turno tentando fechar a conta
        try:
            for i in range(300):
                api_mod._push_terminou("s-lock", api_mod._working_started.get("s-lock"))
        except Exception as e:      # noqa: BLE001
            erros.append(e)

    ts = [_th.Thread(target=escrever), _th.Thread(target=consumir)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert erros == []        # sem KeyError/RuntimeError de mutacao concorrente


def test_recheca_desiste_depois_de_falhas_seguidas(monkeypatch, tmp_path):
    # Falha PERMANENTE (jsonl corrompido, erro reproduzivel no registry) reerguia a mesma excecao a
    # cada 5s pra sempre, pagando registry.list() (tmux) toda volta. Com teto, a cadeia para e diz
    # isso no log — sessao presa e ruim, laco eterno tocando tmux e pior.
    from types import SimpleNamespace
    timers = []
    def _explode():
        raise RuntimeError("tmux fora")
    monkeypatch.setattr(api_mod.registry, "list", _explode)
    monkeypatch.setattr(api_mod.threading, "Timer",
                        lambda *a, **k: (timers.append(a), SimpleNamespace(start=lambda: None))[1])
    monkeypatch.setattr(api_mod.threading, "Thread",
                        lambda target, daemon=None: SimpleNamespace(start=target))
    api_mod._recheca_armada.clear()
    api_mod._falhas_seguidas.pop("s-falha", None)

    for _ in range(api_mod._RETRY_FALHA + 3):
        api_mod._recheca_armada.clear()
        api_mod._on_hook_transition("s-falha", "idle")
    agendados = [a for a in timers if a[1] is api_mod._recheca_kimi]
    assert len(agendados) == api_mod._RETRY_FALHA        # para no teto, nao gira pra sempre
    # o contador NAO zera ao desistir: zerando, o proximo evento recomeca a contagem e o laco volta
    # a girar — teto que reinicia nao e teto. Quem zera e a volta que completa.
    assert api_mod._falhas_seguidas["s-falha"] > api_mod._RETRY_FALHA


def test_espera_picker_fechar(monkeypatch):
    # O Escape e a digitacao saiam juntos e o texto era ENGOLIDO pela TUI que ainda fechava o
    # picker: a resposta do usuario sumia e a bolha ficava presa no fim do chat pra sempre. O gate
    # normal nao pega isso no Pi/Kimi — os marcadores de "TUI pronta" la sao pedacos de moldura
    # (`─ ╰ │`) e o proprio picker desenha moldura, entao a primeira leitura ja diz "pronto".
    from app import tmux as tmux_mod
    # O rodape LONGE do fim do pane de proposito: pergunta longa empurra o "to navigate" pra fora
    # das 8 ultimas linhas, e era ali que o `is_overlay` dizia "fechou" com o picker aberto.
    alto = "  ↑↓ to navigate · Enter to select\n" + ("\n".join("  opcao %d" % i for i in range(12)))
    quadros = iter([alto, alto, "╭───────────╮\n│ >         │\n╰───────────╯"])
    monkeypatch.setattr(tmux_mod, "capture_pane", lambda n: next(quadros))
    monkeypatch.setattr(api_mod.time, "sleep", lambda s: None)
    assert api_mod._espera_picker_fechar("s", timeout=5.0) is True


def test_espera_picker_fechar_estoura_e_envia_assim_mesmo(monkeypatch):
    # Prazo estourado NAO bloqueia o envio: devolve False e quem chama manda mesmo assim (nao piora
    # o caso de hoje) — mesma politica do _wait_input_ready.
    from app import tmux as tmux_mod
    monkeypatch.setattr(tmux_mod, "capture_pane",
                        lambda n: "  ❯ 1. presa\n  ↑↓ to navigate · Enter to select")
    monkeypatch.setattr(api_mod.time, "sleep", lambda s: None)
    assert api_mod._espera_picker_fechar("s", timeout=0.3) is False


def test_texto_do_fallback_nao_diz_que_falhou():
    # No Kimi o texto e o caminho NORMAL (nao ha drive de teclas), entao "o seletor de opcoes
    # falhou" era mentira na cara do usuario — e o agente lia a mesma frase e respondia ao fantasma.
    t = api_mod._pi_answer_fallback_text({"kind": "option", "labels": ["Rota nova (Recomendado)"]})
    assert "falhou" not in t.lower()
    assert "Rota nova (Recomendado)" in t
    assert api_mod._pi_answer_fallback_text({"kind": "option", "labels": []}) == ""


# ---------------------------------------------------------------------------
# Round 2 do parecer task 11: warnings estruturados, unpair sem 500, envio dinamico envelopado
# ---------------------------------------------------------------------------

def test_pair_warning_parcial_carrega_avisos_estruturados(api_client):
    # B1: falha parcial do aviso -> warning com params.avisos = [{sessao, erro}] (o front monta a
    # lista traduzida a partir dela; sem o param o template renderizaria "aviso falhou em: " vazio).
    async def fake_deliver(name, text):
        if name == "voce":
            return {"code": "erro_fila_nao_digitada", "params": {"erro": "No space"},
                    "msg": "fila indisponivel e prompt nao foi digitado: No space"}
        return None
    with patch("app.api.registry.list",
              return_value=[SessionInfo(name="me", cwd="/p"), SessionInfo(name="voce", cwd="/p")]), \
         patch("app.api.pair.join_group", return_value=(["me", "voce"], "snap")), \
         patch("app.api.PairLink.get", return_value={"peers": ["voce"], "task": "", "gid": "g1"}), \
         patch("app.api._deliver", side_effect=fake_deliver):
        r = api_client.post("/api/sessions/me/pair", headers=_h(),
                            json={"peers": ["voce"], "task": "t"})
    assert r.status_code == 200
    w = r.json()["warning"]
    assert w["code"] == "erro_pareamento_aviso_parcial"
    assert w["params"]["avisos"] == [{
        "sessao": "voce",
        "erro": {"code": "erro_fila_nao_digitada", "params": {"erro": "No space"},
                 "msg": "fila indisponivel e prompt nao foi digitado: No space"},
    }]


def test_pair_warning_none_quando_todos_avisados(api_client):
    # B1: sem falha -> warning None (array vazio nunca chega ao formatador do front).
    with patch("app.api.registry.list",
              return_value=[SessionInfo(name="me", cwd="/p"), SessionInfo(name="voce", cwd="/p")]), \
         patch("app.api.pair.join_group", return_value=(["me", "voce"], "snap")), \
         patch("app.api.PairLink.get", return_value={"peers": ["voce"], "task": "", "gid": "g1"}), \
         patch("app.api._deliver", return_value=None):
        r = api_client.post("/api/sessions/me/pair", headers=_h(),
                            json={"peers": ["voce"], "task": "t"})
    assert r.status_code == 200
    assert r.json()["warning"] is None


def test_group_message_warning_carrega_avisos(api_client):
    # B1: group-message com falha em TODOS os peers -> warning com a lista estruturada.
    from app import api as api_mod
    async def fake_thread(*a):
        if a[0] is api_mod._session_exists:
            return True
        return {"ok": False, "error": {"code": "erro_envio_falhou", "params": {"erro": "rede"},
                                       "msg": "falha ao enviar: rede"}}
    with patch("app.api.PairLink.get", return_value={"peers": ["b", "c"], "task": "", "gid": "g1"}), \
         patch("app.api._send_thread", side_effect=fake_thread):
        r = api_client.post("/api/sessions/a/group-message", json={"text": "oi"}, headers=_h())
    assert r.status_code == 200
    w = r.json()["warning"]
    assert w["code"] == "erro_pareamento_grupo_falha"
    assert w["params"]["avisos"] == [
        {"sessao": "b", "erro": {"code": "erro_envio_falhou", "params": {"erro": "rede"},
                                 "msg": "falha ao enviar: rede"}},
        {"sessao": "c", "erro": {"code": "erro_envio_falhou", "params": {"erro": "rede"},
                                 "msg": "falha ao enviar: rede"}},
    ]


def test_unpair_warning_estruturado_sem_server_id(api_client, monkeypatch):
    # B2: peer remoto com CP_SERVER_ID vazio -> item {sessao, erro} (antes era string solta e o
    # x['sessao'] seguinte virava TypeError -> 500). O sair do grupo continua 200 com warning.
    from app import api as api_mod
    monkeypatch.setattr(api_mod.settings, "server_id", "")
    with patch("app.api.pair.leave", return_value=["srv-a::x"]), \
         patch("app.api.peers.is_remote", return_value=True), \
         patch("app.api._deliver", return_value=None):
        r = api_client.delete("/api/sessions/me/pair", headers=_h())
    assert r.status_code == 200
    w = r.json()["warning"]
    assert w["code"] == "erro_pareamento_saida_falhou"
    # O item vira envelope aninhado: o front traduz pelo code (a string crua em pt misturaria
    # idiomas na linha em ingles — parecer c0fc8a84).
    item = w["params"]["avisos"][0]
    assert item["sessao"] == "srv-a::x"
    assert item["erro"]["code"] == "erro_pareamento_server_id_ausente"
    assert "CP_SERVER_ID" in item["erro"]["msg"]


def test_unpair_peer_error_serializa_causa_textual(api_client, monkeypatch):
    # B2: PeerError entra como str(ex) no aviso — o objeto serializaria {"transport": ...} e a
    # UI perderia o motivo. Os dois transportes: a causa textual tem que sobreviver.
    from app import api as api_mod
    from app import peers as peers_mod
    monkeypatch.setattr(api_mod.settings, "server_id", "srv-a")
    for transport in (True, False):
        with patch("app.api.pair.leave", return_value=["srv-b::x"]), \
             patch("app.api.peers.is_remote", return_value=True), \
             patch("app.api.peers.split_addr", return_value=("srv-b", "x")), \
             patch("app.api.peers.call",
                   side_effect=peers_mod.PeerError("rede caiu", transport=transport)), \
             patch("app.api._deliver", return_value=None):
            r = api_client.delete("/api/sessions/me/pair", headers=_h())
        assert r.status_code == 200, transport
        w = r.json()["warning"]
        assert w["params"]["avisos"][0]["sessao"] == "srv-b::x"
        assert "rede caiu" in w["params"]["avisos"][0]["erro"], transport
        assert isinstance(w["params"]["avisos"][0]["erro"], str), transport


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
