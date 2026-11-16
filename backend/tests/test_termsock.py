import subprocess

import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

from tmux_teste import matar_sessao
from app.config import settings

SESS = "cp-test-termsock"


def _client(host="127.0.0.1"):
    """Sem `with`: nenhuma rota daqui precisa do lifespan. `client=` explícito porque o TestClient
    conecta como host 'testclient' por padrão."""
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app, client=(host, 12345))


@pytest.fixture
def sessao():
    matar_sessao(SESS)
    subprocess.run(["tmux", "new-session", "-d", "-s", SESS, "-x", "200", "-y", "50"], check=True)
    yield SESS
    matar_sessao(SESS)


def test_token_errado_fecha_sem_abrir_pty(sessao):
    c = _client()
    with pytest.raises(Exception):
        with c.websocket_connect(f"/api/sessions/{sessao}/term?token=errado&cols=80&rows=24"):
            pass


def test_token_do_dono_abre_de_qualquer_origem():
    # Token na query não é credencial ambiente: passa da Origin e só para na sessão inexistente.
    with pytest.raises(WebSocketDisconnect) as e:
        with _client().websocket_connect("/api/sessions/nao-existe/term?token=secret&cols=80&rows=24",
                                         headers={"origin": "https://evil.com"}):
            pass
    assert (e.value.code, e.value.reason) == (1008, "sessao nao existe")


def test_sessao_desconhecida_recusa(sessao):
    c = _client()
    with pytest.raises(Exception):
        with c.websocket_connect("/api/sessions/nao-existe/term?token=secret&cols=80&rows=24"):
            pass
