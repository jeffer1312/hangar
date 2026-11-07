"""A abertura de Claude com terminal é do Rust: só o que entrou direto no Python (Connect, MCP) vai à
ponte dele, e as rotas internas guardam aqui só o que é daqui (registro em espera, `then`); o dono da sessão do convidado o Rust grava."""
import json
from unittest.mock import patch

import pytest

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


