import pytest
from fastapi import Depends, FastAPI, Request
from fastapi.testclient import TestClient

from app import auth
from app.config import settings
from app.connect_port import CONNECT_PEER, CONNECT_PORT, ConnectPortGate


@pytest.fixture(autouse=True)
def _limpo():
    settings.auth_token = "secret"
    auth.reset_backoff()
    yield
    auth.reset_backoff()


def _app():
    a = FastAPI()

    @a.get("/quem")
    def quem(request: Request):
        return {"ip": request.client.host}

    @a.get("/local", dependencies=[Depends(auth.require_auth), Depends(auth.require_loopback)])
    def local():
        return {"ok": True}

    a.add_middleware(ConnectPortGate)
    return a


def _connect():
    return TestClient(_app(), base_url=f"http://127.0.0.1:{CONNECT_PORT}", client=("127.0.0.1", 5))


def test_porta_do_connect_nunca_e_local():
    c = _connect()
    assert c.get("/quem").json()["ip"] == CONNECT_PEER
    assert c.get("/local", headers={"Authorization": "Bearer secret"}).status_code == 403


def test_acerto_do_dono_nao_zera_o_contador_do_connect():
    c = _connect()
    for _ in range(7):
        assert c.get("/local", headers={"Authorization": "Bearer errado"}).status_code == 401
    assert c.get("/local", headers={"Authorization": "Bearer secret"}).status_code == 403
    assert c.get("/local", headers={"Authorization": "Bearer errado"}).status_code == 401
    assert c.get("/local", headers={"Authorization": "Bearer secret"}).status_code == 429


def test_pedido_sem_senha_nao_trava_o_dono():
    c = _connect()
    for _ in range(20):
        assert c.get("/local").status_code == 401
    assert c.get("/local", headers={"Authorization": "Bearer secret"}).status_code == 403


def test_esquema_sai_da_porta():
    a = FastAPI()

    @a.get("/esquema")
    def esquema(request: Request):
        return {"s": request.url.scheme}

    a.add_middleware(ConnectPortGate)
    c = TestClient(a, base_url=f"http://127.0.0.1:{CONNECT_PORT}")
    assert c.get("/esquema").json()["s"] == "https"


def test_porta_principal_mantem_o_cliente():
    c = TestClient(_app(), base_url="http://127.0.0.1:8765", client=("127.0.0.1", 5))
    assert c.get("/quem").json()["ip"] == "127.0.0.1"


def test_erros_pelo_connect_bloqueiam():
    c = _connect()
    for _ in range(8):
        assert c.get("/local", headers={"Authorization": "Bearer errado"}).status_code == 401
    assert c.get("/local", headers={"Authorization": "Bearer secret"}).status_code == 429
