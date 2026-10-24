import base64
import json

import pytest

from app import connect
from app.connect_port import CONNECT_PORT


def _codigo(**over):
    d = {"v": 1, "server": "connect.hangar.dev.br", "token": "a" * 64,
         "host": "notebook.jeffersonfelizardo.hangar.dev.br", **over}
    return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip("=")


def test_codigo_valido():
    c = connect.parse_code(_codigo())
    assert (c.server, c.host) == ("connect.hangar.dev.br", "notebook.jeffersonfelizardo.hangar.dev.br")


@pytest.mark.parametrize("over", [
    {"v": 2}, {"host": "dev.br"}, {"host": 'x".y.z'}, {"host": "a.b.c\n[x]"}, {"host": "a.b.c\n"},
    {"token": 'abc"def' + "a" * 20}, {"token": "curto"}, {"server": "connect hangar"},
])
def test_codigo_recusado(over):
    with pytest.raises(connect.ConnectError):
        connect.parse_code(_codigo(**over))


def test_codigo_ilegivel():
    with pytest.raises(connect.ConnectError):
        connect.parse_code("@@@não é base64@@@")


def test_frpc():
    toml = connect.render_frpc(connect.parse_code(_codigo()))
    assert 'serverAddr = "connect.hangar.dev.br"' in toml
    assert 'transport.protocol = "wss"' in toml
    assert f"localPort = {connect.CADDY_PORT}" in toml
    assert 'customDomains = ["notebook.jeffersonfelizardo.hangar.dev.br"]' in toml


def test_caddyfile(tmp_path):
    texto = connect.render_caddyfile(connect.parse_code(_codigo()), tmp_path / "com espaço")
    assert f'storage file_system "{(tmp_path / "com espaço").as_posix()}"' in texto
    assert "notebook.jeffersonfelizardo.hangar.dev.br {" in texto
    assert f"reverse_proxy 127.0.0.1:{CONNECT_PORT}" in texto
    assert "bind 127.0.0.1" in texto


def test_estado_ida_e_volta(tmp_path, monkeypatch):
    monkeypatch.setattr(connect, "folder", lambda: tmp_path)
    assert connect.read_state() == {}
    connect.write_state({"code": "x", "enabled": True})
    assert connect.read_state() == {"code": "x", "enabled": True}


import hashlib
import io
import tarfile


def _tar(membro: str, conteudo: bytes) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        info = tarfile.TarInfo(membro)
        info.size = len(conteudo)
        t.addfile(info, io.BytesIO(conteudo))
    return buf.getvalue()


@pytest.fixture
def _plataforma(tmp_path, monkeypatch):
    monkeypatch.setattr(connect, "folder", lambda: tmp_path)
    monkeypatch.setattr(connect, "_platform", lambda: ("linux", "amd64"))
    pacotes = {
        "frp": _tar("frp_0.71.0_linux_amd64/frpc", b"FRPC"),
        "caddy": _tar("caddy", b"CADDY"),
    }
    monkeypatch.setitem(connect._FRP_SHA256, ("linux", "amd64"), hashlib.sha256(pacotes["frp"]).hexdigest())
    monkeypatch.setitem(connect._CADDY_SHA512, ("linux", "amd64"), hashlib.sha512(pacotes["caddy"]).hexdigest())
    baixados = []

    def baixar(url):
        baixados.append(url)
        return pacotes["frp" if "fatedier" in url else "caddy"]

    monkeypatch.setattr(connect, "_download", baixar)
    return baixados


def test_binarios_baixa_uma_vez(_plataforma):
    bins = connect.binaries()
    assert bins["frpc"].read_bytes() == b"FRPC" and bins["caddy"].read_bytes() == b"CADDY"
    connect.binaries()
    assert len(_plataforma) == 2


@pytest.fixture
def _api(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api
    from app.config import settings
    settings.auth_token = "secret"
    monkeypatch.setattr(connect, "folder", lambda: tmp_path)
    chamadas = []

    async def start():
        chamadas.append("start")

    async def stop():
        chamadas.append("stop")

    monkeypatch.setattr(connect, "start", start)
    monkeypatch.setattr(connect, "stop", stop)
    c = TestClient(api.app, client=("10.0.0.7", 1))
    c.headers["Authorization"] = "Bearer secret"
    return c, chamadas


def test_rotas(_api):
    c, chamadas = _api
    assert c.get("/api/connect").json()["configured"] is False
    r = c.put("/api/connect", json={"code": _codigo()})
    assert r.status_code == 200 and r.json()["url"] == "https://notebook.jeffersonfelizardo.hangar.dev.br"
    assert "token" not in json.dumps(r.json())
    assert c.put("/api/connect", json={"code": "lixo"}).status_code == 400
    assert c.delete("/api/connect").json()["configured"] is False
    # O Connect fica inativo até a reescrita no Rust: gravar o código não liga o frpc nem o Caddy.
    assert chamadas == []


def test_sem_porta_do_connect_nao_liga(tmp_path, monkeypatch):
    import asyncio
    monkeypatch.setattr(connect, "folder", lambda: tmp_path)
    monkeypatch.setattr(connect, "_error", None)
    monkeypatch.setattr(connect.connect_port, "unavailable", "porta do Connect 8768 indisponível")
    connect.write_state({"code": _codigo(), "enabled": True})
    asyncio.run(connect.start())
    assert connect.status()["error"] == "porta do Connect 8768 indisponível"
    assert connect.status()["processes"] == {}


def test_hash_errado_nao_grava(_plataforma, monkeypatch):
    monkeypatch.setitem(connect._FRP_SHA256, ("linux", "amd64"), "0" * 64)
    with pytest.raises(connect.ConnectError):
        connect.binaries()
    assert not list((connect.folder() / "bin").rglob("frpc"))


def test_codigo_com_chave_vai_para_o_frpc():
    c = connect.parse_code(_codigo(key="k" * 43))
    assert c.key == "k" * 43
    assert 'metadatas.key = "' + "k" * 43 + '"' in connect.render_frpc(c)
    assert "transport.heartbeatInterval = 30" in connect.render_frpc(c)


def test_codigo_sem_chave_continua_valendo():
    c = connect.parse_code(_codigo())
    assert c.key is None and "metadatas.key" not in connect.render_frpc(c)


@pytest.mark.parametrize("key", ['abc"' + "k" * 20, "k" * 20 + "\n", "curta"])
def test_chave_ruim_recusada(key):
    with pytest.raises(connect.ConnectError):
        connect.parse_code(_codigo(key=key))
