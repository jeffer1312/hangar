"""A abertura de Claude com terminal é do Rust: só o que entrou direto no Python (Connect, MCP) vai à
ponte dele, e as rotas internas guardam aqui só o que é daqui (registro em espera, `then`); o dono da sessão do convidado o Rust grava."""
import asyncio
import json
from types import SimpleNamespace
from unittest.mock import patch

import pytest
from fastapi import HTTPException
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api, pqueue

SECRET = "cd" * 32
INFO = {"name": "s1", "cwd": "/p", "jsonl": "/p/x.jsonl", "provider": "claude", "headless": False,
        "config_dir": None, "account_source": None}


@pytest.fixture(autouse=True)
def _env(monkeypatch, tmp_path):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    yield
    internal_api.set_secret(None)


def _python_routes(path):
    from starlette.routing import Match, Mount
    scope = {"type": "http", "path": path, "method": "POST", "root_path": ""}
    return [r for r in api_mod.app.router.routes if not isinstance(r, Mount) and r.matches(scope)[0] != Match.NONE]


def test_release_and_birth_are_all_rust():
    assert _python_routes("/api/sessions"), "o casador acha uma rota que o Python atende"
    assert _python_routes("/internal/sessions/s1/release") == [], "soltar o nome é todo do Rust"
    assert _python_routes("/internal/sessions/s1/born") == [], "o nascimento é todo do Rust"


def test_the_guest_claim_is_written_by_the_rust_not_here():
    assert _python_routes("/internal/guests/claim") == []


def _body(**kw):
    return json.dumps({"name": "s1", "cwd": "/p", **kw}).encode()


def test_only_what_entered_python_directly_goes_to_the_rust_open(monkeypatch):
    from app import runtime_coordinator
    from app.connect_port import CONNECT_PORT
    sent = []

    def bridge(name, route, method, body, timeout):
        sent.append((name, route, method, json.loads(body)))
        payload = json.loads(body)
        return (200, json.dumps({"info": INFO, "undo": None}).encode()) if payload["request"].get("provider") == "claude" else None
    monkeypatch.setattr(runtime_coordinator, "rust_claude", bridge)
    connect = SimpleNamespace(scope={"server": ("127.0.0.1", CONNECT_PORT)})
    info = asyncio.run(api_mod._open_in_rust(_body(provider="claude", headless=False), connect))
    assert (info.name, info.provider, info.owner) == ("s1", "claude", None)
    # Ausente continua ausente: o Rust decide o provedor padrão.
    assert sent[0] == ("_", "open", "POST", {"request": {"name": "s1", "cwd": "/p", "provider": "claude", "headless": False}, "guest": None})
    assert asyncio.run(api_mod._open_in_rust(_body(provider="codex"), connect)) is None
    upstream = SimpleNamespace(scope={"server": ("127.0.0.1", 49152)})
    assert asyncio.run(api_mod._open_in_rust(_body(provider="claude"), upstream)) is None
    # Sem pedido HTTP ninguém cria por dentro do Python: o MCP abre no Rust.
    assert asyncio.run(api_mod._open_in_rust(_body(provider="claude"), None)) is None
    assert len(sent) == 2, "o que entrou pela 8765 o Rust já decidiu: não volta a ele"
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args: (
        409, json.dumps({"detail": {"code": "erro_nome_em_uso", "params": {}, "msg": "ja existe uma sessao com esse nome"}}).encode()))
    with pytest.raises(HTTPException) as refused:
        asyncio.run(api_mod._open_in_rust(_body(provider="claude"), connect))
    assert (refused.value.status_code, refused.value.detail["code"]) == (409, "erro_nome_em_uso")


def test_the_connect_guest_goes_to_the_rust_by_id_and_is_not_claimed_here(monkeypatch):
    """O Rust grava o dono no `guests.json`: daqui só vai o id do convidado autenticado."""
    from app import guest_users, runtime_coordinator
    from app.connect_port import CONNECT_PORT
    sent = []
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args: (
        sent.append(json.loads(args[3])), (200, json.dumps({"info": {**INFO, "owner": "ana"}}).encode()))[1])
    monkeypatch.setattr(guest_users, "claim", lambda *a: pytest.fail("o dono é gravado pelo Rust"))
    marker = guest_users.current.set(SimpleNamespace(id="g1", name="ana", root="/casa/ana"))
    try:
        info = asyncio.run(api_mod._open_in_rust(_body(provider="claude"), SimpleNamespace(scope={"server": ("127.0.0.1", CONNECT_PORT)})))
    finally:
        guest_users.current.reset(marker)
    assert sent[0]["guest"] == "g1" and "guest_root" not in sent[0]
    assert info.owner == "ana"


def test_the_connect_request_reaches_the_rust_as_sent_inherit_included(monkeypatch):
    """O Python não valida o pedido (D-012/D-016): o que o Connect mandou chega ao Rust tal e qual."""
    from app import runtime_coordinator
    from app.config import settings
    from app.connect_port import CONNECT_PORT
    sent = []
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args: (
        sent.append(json.loads(args[3])), (200, json.dumps({"info": INFO}).encode()))[1])
    monkeypatch.setattr(settings, "auth_token", "secret")
    request = {"name": "s1", "cwd": "/p", "inherit": False, "creator": "mae", "campo_novo": 1}
    answer = TestClient(api_mod.app, base_url=f"http://127.0.0.1:{CONNECT_PORT}").post(
        "/api/sessions", json=request, headers={"Authorization": "Bearer secret"})
    assert answer.status_code == 200, answer.text
    assert answer.json()["name"] == "s1"
    assert sent == [{"request": request, "guest": None}]
