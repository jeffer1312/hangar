"""Contrato da ponte: streaming, fechamento e ausência de fallback em pending."""
import http.client
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from fastapi import HTTPException
from starlette.requests import Request
from app import account_bridge, runtime_coordinator, upload_bridge


@pytest.fixture
def transport(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    monkeypatch.setattr(account_bridge, "_preparation_transport", ("127.0.0.1:9", "synthetic"))


def test_pending_without_transport_fails_without_python_fallback(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="pending"))
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    with pytest.raises(HTTPException) as error:
        upload_bridge.request_json("test", "list")
    assert error.value.status_code == 503
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    assert upload_bridge.request_json("test", "list") is None


async def test_forward_streams_both_directions_and_closes(transport, monkeypatch):
    connection = Mock()
    incoming = Mock()
    incoming.status = 206
    incoming.getheaders.return_value = [
        ("content-range", "bytes 2-4/10"), ("content-type", "application/octet-stream"),
        ("transfer-encoding", "chunked")]
    incoming.read.side_effect = [b"234", b""]
    connection.getresponse.return_value = incoming
    monkeypatch.setattr(http.client, "HTTPConnection", lambda *args, **kwargs: connection)
    parts = iter([{"type": "http.request", "body": b"first", "more_body": True},
                  {"type": "http.request", "body": b"second", "more_body": False}])

    async def receive():
        return next(parts)

    request = Request({"type": "http", "headers": [], "query_string": b"name=video.webm"}, receive)
    response = await upload_bridge.forward("name/with slash", "save", request, audio_only=True)
    assert b"".join([item async for item in response.body_iterator]) == b"234"
    assert response.status_code == 206
    assert response.headers["content-range"] == "bytes 2-4/10"
    assert "transfer-encoding" not in response.headers
    target = connection.putrequest.call_args.args[1]
    assert "/name%2Fwith%20slash?" in target
    assert "audio_only=true" in target
    assert [call.args[0] for call in connection.send.call_args_list] == [
        b"5\r\nfirst\r\n", b"6\r\nsecond\r\n", b"0\r\n\r\n"]
    incoming.close.assert_called_once()
    connection.close.assert_called_once()


async def test_forward_closes_failed_request_without_retry(transport, monkeypatch):
    connection = Mock()
    connection.endheaders.side_effect = OSError("synthetic")
    monkeypatch.setattr(http.client, "HTTPConnection", lambda *args, **kwargs: connection)
    request = Request({"type": "http", "headers": [], "query_string": b""})
    with pytest.raises(HTTPException) as error:
        await upload_bridge.forward("test", "download", request, filename="1-abcd.bin")
    assert error.value.status_code == 503
    connection.close.assert_called_once()
    connection.putrequest.assert_called_once()
