"""A abertura de Claude com terminal é do Rust: só o que entrou direto no Python (Connect, MCP) vai à
ponte dele, e as rotas internas guardam aqui só o que é daqui (registro em espera, `then`); o dono da sessão do convidado o Rust grava."""
import pytest

import app.api as api_mod
from app import internal_api

SECRET = "cd" * 32


@pytest.fixture(autouse=True)
def _env():
    internal_api.set_secret(SECRET)
    yield
    internal_api.set_secret(None)


def _python_routes(path):
    from starlette.routing import Match, Mount
    scope = {"type": "http", "path": path, "method": "POST", "root_path": ""}
    return [r for r in api_mod.app.router.routes if not isinstance(r, Mount) and r.matches(scope)[0] != Match.NONE]



def test_the_guest_claim_is_written_by_the_rust_not_here():
    assert _python_routes("/internal/guests/claim") == []


