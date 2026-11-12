import json

import pytest
from fastapi import HTTPException, Request, Response

from app import auth, diag


SECRET = "segredo-que-nao-pode-aparecer@example.invalid"


@pytest.fixture
def diario(tmp_path, monkeypatch):
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "diario")
    token = diag.req_atual.set("req-diagnostico")
    yield lambda: [json.loads(line) for line in diag.caminho_do_dia().read_text().splitlines()]
    diag.req_atual.reset(token)


def _request(headers=(), query=b"", ip="127.0.0.1"):
    return Request({"type": "http", "method": "GET", "scheme": "http", "path": "/",
                    "headers": list(headers), "query_string": query, "client": (ip, 1234),
                    "server": ("localhost", 80)})


@pytest.mark.parametrize("mecanismo", ["bearer", "query", "cookie", "ausente"])
@pytest.mark.asyncio
async def test_auth_registra_transicao_e_recuperacao_sem_credenciais(diario, monkeypatch, mecanismo):
    monkeypatch.setattr(auth.settings, "auth_token", "token-correto")
    headers = {
        "bearer": [(b"authorization", f"Bearer {SECRET}".encode())],
        "cookie": [(b"cookie", f"cp_token={SECRET}".encode())],
    }.get(mecanismo, [])
    query = f"token={SECRET}".encode() if mecanismo == "query" else b""
    for _ in range(3):
        with pytest.raises(HTTPException) as exc:
            await auth.require_auth(_request(headers, query))
        assert exc.value.status_code == 401
    for _ in range(2):
        await auth.require_auth(_request([(b"authorization", b"Bearer token-correto")]))
    rows = diario()
    assert [r["evento"] for r in rows] == ["auth.recusado", "auth.recuperado"]
    assert rows[0]["etapa"] == mecanismo
    assert all(r["req"] == "req-diagnostico" for r in rows)
    assert SECRET not in json.dumps(rows) and "127.0.0.1" not in json.dumps(rows)


@pytest.mark.asyncio
async def test_auth_bloqueio_e_recuperacao_sem_repetir_poll(diario, monkeypatch):
    monkeypatch.setattr(auth.settings, "auth_token", "token-correto")
    clock = [1000.0]
    monkeypatch.setattr(auth.time, "time", lambda: clock[0])
    bad = _request([(b"authorization", SECRET.encode())], ip="192.0.2.15")
    for _ in range(auth._MAX_FAILS + 3):
        with pytest.raises(HTTPException):
            await auth.require_auth(bad)
    clock[0] += auth._WINDOW + 1
    await auth.require_auth(_request([(b"authorization", b"Bearer token-correto")], ip="192.0.2.15"))
    rows = diario()
    assert [r["detalhe"] for r in rows] == ["token_ausente", "bloqueio_temporario", "acesso_restabelecido"]
    assert "192.0.2.15" not in json.dumps(rows)
