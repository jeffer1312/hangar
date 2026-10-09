"""Dono da transcrição: Rust ativo nunca recorre ao serviço Python depois da falha."""
import urllib.request
from types import SimpleNamespace

import pytest

from app import runtime_config, runtime_coordinator, transcribe
from app import transcription_bridge as bridge


@pytest.fixture(autouse=True)
def isolated(monkeypatch):
    bridge.configure(None, None)
    monkeypatch.setattr(bridge._ready, "wait", lambda timeout: False)
    monkeypatch.setattr(runtime_config, "get", {
        "transcription_providers": [{"id": "p", "kind": "openai", "api_key": "fixture", "base_url": "http://127.0.0.1:9999/v1"}],
    }.get)
    monkeypatch.setattr(urllib.request, "urlopen", lambda *args, **kwargs: pytest.fail("o Python executou uma segunda chamada de transcrição"))


def test_rust_owner_receives_audio_and_profile(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    received = []

    def rust(content, filename, profile, provider_id=None):
        received.append((content, filename, profile))
        return {"text": "Texto pelo Rust.", "provider": "Local", "aviso": None}

    monkeypatch.setattr(bridge, "transcribe", rust)
    result = transcribe.transcribe_with_provider(b"audio", "fala.webm", transcribe.FILE_LIMITS)
    assert result.text == "Texto pelo Rust."
    assert received == [(b"audio", "fala.webm", "file")]


@pytest.mark.parametrize("mode", ["rust", "pending"])
def test_rust_failure_does_not_charge_python_provider(monkeypatch, mode):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode=mode))
    with pytest.raises(transcribe.TranscribeError, match="Rust") as error:
        transcribe.transcribe_with_provider(b"audio", "fala.wav")
    assert error.value.status == 503


def test_python_mode_has_no_second_transcription_engine(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    with pytest.raises(transcribe.TranscribeError, match="Rust") as error:
        transcribe.transcribe_with_provider(b"audio", "fala.wav")
    assert error.value.status == 503
