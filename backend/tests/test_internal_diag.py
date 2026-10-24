# backend/tests/test_internal_diag.py
"""Diário interno que o Rust usa (`/internal/diag`)."""
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api

SECRET = "ab" * 32
HEADERS = {"X-Hangar-Internal": SECRET}


@pytest.fixture(autouse=True)
def _env():
    internal_api.set_secret(SECRET)
    yield
    internal_api.set_secret(None)


def _client():
    return TestClient(api_mod.app, client=("127.0.0.1", 50000))


def test_the_rust_diary_accepts_the_select_events():
    for event in ("opcao.nao_convergiu", "opcao.envio_falhou"):
        body = {"evento": event, "sessao": "s1", "codigo": "x", "motivo": "m"}
        with patch("app.diag.registrar") as seen:
            assert _client().post("/internal/diag", headers=HEADERS, json=body).status_code == 200
        assert event in [call.args[0] for call in seen.call_args_list]


def test_the_rust_diary_takes_the_events_named_by_the_rust_and_refuses_the_raw_ones():
    # `diag.rs::wire_event`: o que não é `rust.*` sai como `rust.<evento com _>`.
    for event, status in (("rust.session_limit_check", 200), ("rust.runtime_registration_failed", 200), ("session.limit_check", 400)):
        body = {"evento": event, "sessao": "s1", "codigo": "limit_check_none", "motivo": "m"}
        with patch("app.diag.registrar"):
            assert _client().post("/internal/diag", headers=HEADERS, json=body).status_code == status, event
