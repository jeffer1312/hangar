# backend/tests/test_internal_api.py
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api, pqueue
from app.models import SessionInfo

SECRET = "ab" * 32
ROUTE = "/internal/sessions/s1/info"


@pytest.fixture(autouse=True)
def _env(monkeypatch, tmp_path):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    yield
    internal_api.set_secret(None)


def _client(ip="127.0.0.1"):
    return TestClient(api_mod.app, client=(ip, 50000))


def _info(**kw):
    return SessionInfo(**{"name": "s1", "cwd": "/p", "jsonl": "/p/abc-123.jsonl", "provider": "claude", **kw})


def _get(client, headers=None, info=None):
    with patch("app.api._cached_info", AsyncMock(return_value=info or _info())):
        return client.get(ROUTE, headers=headers if headers is not None else {"X-Hangar-Internal": SECRET})


def test_info_has_what_hangar_server_needs(tmp_path):
    with patch("app.adapters.chave_de", lambda name, provider: "claude-headless"):
        r = _get(_client())
    assert r.status_code == 200
    assert r.json() == {"provider": "claude-headless", "jsonl": "/p/abc-123.jsonl", "session_key": "abc-123",
                        "history": {"queue": str(tmp_path / "s1.jsonl")}}


def test_codex_session_key_is_the_rollout_id():
    info = _info(provider="codex",
                 jsonl="/c/rollout-2026-09-02T14-00-00-019f0000-0000-7000-8000-000000000001.jsonl")
    r = _get(_client(), info=info)
    assert r.json()["provider"] == "codex"
    assert r.json()["session_key"] == "019f0000-0000-7000-8000-000000000001"


def test_session_without_transcript_yet():
    r = _get(_client(), info=_info(jsonl=None))
    assert r.status_code == 200
    assert (r.json()["jsonl"], r.json()["session_key"]) == (None, "")


def test_unknown_session_404():
    with patch("app.api._cached_info", AsyncMock(return_value=None)):
        r = _client().get(ROUTE, headers={"X-Hangar-Internal": SECRET})
    assert r.status_code == 404


@pytest.mark.parametrize("headers", [{}, {"X-Hangar-Internal": "errado"}, {"X-Hangar-Internal": ""}])
def test_wrong_secret_404(headers):
    assert _get(_client(), headers=headers).status_code == 404


def test_no_secret_404():
    internal_api.set_secret(None)
    assert _get(_client(), headers={"X-Hangar-Internal": ""}).status_code == 404


@pytest.fixture
def events(monkeypatch):
    got = []
    # Só o evento desta rota: o middleware também registra cada 404 como `api.servidor`.
    monkeypatch.setattr(internal_api.diag, "registrar",
                        lambda evento, nivel="ok", **campos: evento == "internal.recusado"
                        and got.append((evento, nivel, campos)))
    return got


def test_refused_hangar_server_goes_to_the_diary_without_the_secret(events):
    assert _get(_client(), headers={"X-Hangar-Internal": "errado"}).status_code == 404
    internal_api.set_secret(None)
    assert _get(_client(), headers={"X-Hangar-Internal": SECRET}).status_code == 404
    assert events == [("internal.recusado", "aviso", {"codigo": "segredo_errado"}),
                      ("internal.recusado", "aviso", {"codigo": "sem_segredo"})]
    assert SECRET not in repr(events)


def test_requests_that_are_not_the_hangar_server_stay_out_of_the_diary(events):
    assert _get(_client(), headers={}).status_code == 404              # sem cabeçalho
    assert _get(_client("10.0.0.5"), headers={"X-Hangar-Internal": "x"}).status_code == 404   # de fora
    assert _get(_client()).status_code == 200                          # aceito
    assert events == []


def test_secret_never_goes_to_environ(monkeypatch):
    monkeypatch.delenv("HANGAR_INTERNAL_SECRET", raising=False)
    internal_api.set_secret(SECRET)
    import os
    assert "HANGAR_INTERNAL_SECRET" not in os.environ
    assert _get(_client()).status_code == 200


def test_info_payload_is_what_the_route_returns(tmp_path):
    with patch("app.adapters.chave_de", lambda name, provider: "claude-headless"):
        assert internal_api.info_payload("s1", "claude", "/p/abc-123.jsonl") == _get(_client()).json()


def test_outside_loopback_404_even_with_secret():
    assert _get(_client("10.0.0.7")).status_code == 404


def test_left_out_of_the_api_schema():
    paths = api_mod.app.openapi()["paths"]
    assert "/api/sessions/{name}/history" in paths
    assert not any(p.startswith("/internal") for p in paths)
