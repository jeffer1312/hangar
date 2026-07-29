import pytest
from fastapi import Depends, FastAPI
from fastapi.testclient import TestClient

from app import auth
from app.config import settings


@pytest.fixture
def app():
    settings.auth_token = "secret"
    a = FastAPI()

    @a.get("/ping", dependencies=[Depends(auth.require_auth)])
    def ping():
        return {"ok": True}

    return a


def cli(app, ip="10.0.0.5"):
    # TestClient usa host "testclient" por padrao; um IP explicito e o que faz cada teste ser uma
    # ORIGEM distinta (e nao cair na isencao de loopback).
    return TestClient(app, client=(ip, 12345))


def hit(c, token="wrong"):
    return c.get("/ping", headers={"Authorization": f"Bearer {token}"}).status_code


def test_token_certo_passa(app):
    assert hit(cli(app), "secret") == 200


def test_token_errado_401(app):
    assert hit(cli(app)) == 401


def test_backoff_barra_apos_max_fails(app):
    c = cli(app)
    assert [hit(c) for _ in range(auth._MAX_FAILS)] == [401] * auth._MAX_FAILS
    assert hit(c) == 429
    r = c.get("/ping", headers={"Authorization": "Bearer wrong"})
    assert r.headers["Retry-After"] == str(int(auth._WINDOW))
    # Bloqueado, o token nem e avaliado — nem o CERTO passa (senao 200-vs-429 entregaria o palpite).
    assert hit(c, "secret") == 429


def test_acerto_no_meio_limpa_o_estado(app):
    c = cli(app)
    for _ in range(auth._MAX_FAILS - 1):
        assert hit(c) == 401
    assert hit(c, "secret") == 200
    assert auth._fails == {}
    # Sem a limpeza, estas somariam com as anteriores e a ultima daria 429.
    for _ in range(auth._MAX_FAILS - 1):
        assert hit(c) == 401


def test_origem_diferente_nao_e_afetada(app):
    ruim = cli(app, "10.0.0.9")
    for _ in range(auth._MAX_FAILS):
        hit(ruim)
    assert hit(ruim) == 429
    outra = cli(app, "10.0.0.10")
    assert hit(outra) == 401       # erra por conta propria, nao herda o bloqueio
    assert hit(outra, "secret") == 200


def test_loopback_nunca_bloqueia(app):
    # cp-send/cp-panel batem no backend por 127.0.0.1 o tempo todo; quem esta na maquina ja le o .env.
    c = cli(app, "127.0.0.1")
    for _ in range(auth._MAX_FAILS * 3):
        assert hit(c) == 401
    assert auth._fails == {}
    assert hit(c, "secret") == 200


def test_estado_nao_cresce_sem_limite(app):
    for n in range(auth._MAX_ORIGINS + 200):
        hit(cli(app, f"10.1.{n // 256}.{n % 256}"))
    assert len(auth._fails) <= auth._MAX_ORIGINS


def test_janela_expira(app, monkeypatch):
    c = cli(app)
    for _ in range(auth._MAX_FAILS):
        hit(c)
    assert hit(c) == 429
    real = auth.time.time
    monkeypatch.setattr(auth.time, "time", lambda: real() + auth._WINDOW + 1)
    assert hit(c) == 401           # passou a janela: volta a avaliar o token
    assert hit(c, "secret") == 200


def test_token_nao_ascii_nao_explode(app):
    # compare_digest com str rejeita nao-ASCII (TypeError -> 500). Em bytes, e so um 401. Vai pela
    # query porque header nao carrega nao-ASCII — e a query e justamente o caminho do link/QR.
    r = cli(app).get("/ping", params={"token": "sênha-çom-acento"})
    assert r.status_code == 401
