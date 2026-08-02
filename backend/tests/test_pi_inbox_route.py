import pytest

from app.config import settings
from app.pi_inbox import INBOX


def _client():
    """`client=` explícito: o TestClient conecta como host 'testclient' por padrão
    (starlette/testclient.py:392) e cairia na recusa de não-loopback da rota. Mesmo contorno de
    tests/test_auth_backoff.py:21."""
    settings.auth_token = "secret"
    from app.api import app
    from fastapi.testclient import TestClient
    return TestClient(app, client=("127.0.0.1", 12345))


def test_conexao_sem_token_e_recusada():
    client = _client()
    with pytest.raises(Exception):
        with client.websocket_connect("/api/pi/inbox"):
            pass


def test_registra_e_desregistra_pelo_pane():
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%33"})
        ws.send_json({"pong": True})   # round-trip: garante que o registro já aconteceu
        assert INBOX.tem_linha("%33") is True
    assert INBOX.tem_linha("%33") is False, "fechou o socket, a linha tem que sair"


def test_primeira_mensagem_sem_pane_fecha():
    """Sem identidade não dá pra registrar — e registrar com chave errada é pior que recusar."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"nada": 1})
        with pytest.raises(Exception):
            ws.receive_json()


def test_confirmacao_de_id_desconhecido_nao_derruba():
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%9"})
        ws.send_json({"id": "inexistente", "ok": True})
        ws.send_json({"pong": True})
        assert INBOX.tem_linha("%9") is True


def test_mensagem_gigante_fecha_a_linha():
    """O middleware de body-size ignora WebSocket de propósito (api.py:83), então o teto é aqui."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%7"})
        ws.send_text("x" * (256 * 1024 + 10))
        with pytest.raises(Exception):
            ws.receive_json()
