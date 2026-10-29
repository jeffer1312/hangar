"""Ponte dos catálogos: envelope do Rust validado, limites e espera do modo pending."""
import json
from types import SimpleNamespace

import pytest

from app import diag, dictation_bridge, runtime_coordinator
from app import transcription_bridge as transport


class _Response:
    def __init__(self, body: bytes):
        self.body = body

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self, limit: int) -> bytes:
        return self.body[:limit]


@pytest.fixture(autouse=True)
def isolated(monkeypatch):
    transport.configure(None, None)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    yield
    transport.configure(None, None)


def _rust_answers(monkeypatch, body: bytes) -> list:
    calls = []

    def open_(call, timeout):
        calls.append(call)
        return _Response(body)

    transport.configure("127.0.0.1:9", "secret")
    monkeypatch.setattr(transport, "_opener", SimpleNamespace(open=open_))
    return calls


@pytest.mark.parametrize("envelope", [
    {"ok": 1, "result": {}},
    {"ok": "true", "result": {}},
    {"ok": False, "error": {"status": 200, "code": "x", "detail": "y"}},
    {"ok": False, "error": {"status": 600, "code": "x", "detail": "y"}},
    {"ok": False, "error": {"status": "500"}},
    {"ok": True, "result": []},
])
def test_invalid_envelope_is_503(monkeypatch, envelope):
    _rust_answers(monkeypatch, json.dumps(envelope).encode())
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("catalog_models", {"raw": "texto"})
    assert (error.value.status, error.value.code) == (503, "dictation_rust_unavailable")


def test_rust_error_keeps_its_status_and_code(monkeypatch):
    _rust_answers(monkeypatch, json.dumps(
        {"ok": False, "error": {"status": 409, "code": "dictation_cli_missing", "detail": "sem CLI"}}).encode())
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("catalog_models", {})
    assert (error.value.status, error.value.code, error.value.detail) == (409, "dictation_cli_missing", "sem CLI")


def test_response_over_4_mib_is_503(monkeypatch):
    _rust_answers(monkeypatch, b'{"ok":true,"result":{"t":"' + b"a" * (4 * 1024 * 1024) + b'"}}')
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("catalog_models", {})
    assert error.value.status == 503


def test_failure_records_cause_class_without_text(monkeypatch):
    logged = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **campos: logged.append(campos))
    _rust_answers(monkeypatch, b"not json com texto ditado")
    with pytest.raises(transport.BridgeError):
        dictation_bridge.request("catalog_models", {})
    assert logged == [{"codigo": "dictation_rust_unavailable", "etapa": "catalog_models",
                       "causa": "JSONDecodeError", "causa_interna": None}]


def test_payload_over_1_mib_is_413_without_calling_rust(monkeypatch):
    calls = _rust_answers(monkeypatch, b"{}")
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("catalog_models", {"raw": "a" * (1024 * 1024)})
    assert error.value.status == 413
    assert calls == []


def test_pending_waits_for_rust_then_succeeds(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="pending"))
    waited = []

    def ready(timeout):
        waited.append(timeout)
        _rust_answers(monkeypatch, json.dumps({"ok": True, "result": {"models": []}}).encode())
        return True

    monkeypatch.setattr(transport._ready, "wait", ready)
    assert dictation_bridge.request("parse_models", {}) == {"models": []}
    assert waited == [runtime_coordinator.PENDING_WAIT_S]


def test_pending_timeout_is_503(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="pending"))
    monkeypatch.setattr(transport._ready, "wait", lambda timeout: False)
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("parse_models", {})
    assert (error.value.status, error.value.code) == (503, "dictation_rust_unavailable")


def test_python_mode_does_not_wait(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    monkeypatch.setattr(transport._ready, "wait", lambda timeout: pytest.fail("modo python não espera o Rust"))
    with pytest.raises(transport.BridgeError) as error:
        dictation_bridge.request("parse_models", {})
    assert error.value.status == 503
