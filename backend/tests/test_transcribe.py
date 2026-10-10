"""A borda HTTP mantém a organização separada do reconhecimento no Rust."""
import pytest
from app.config import settings


@pytest.mark.parametrize("query,cleaned", [("", True), ("?limpar=0", False), ("?limpar=1", True)])
def test_before_session_audio_can_skip_cleanup(monkeypatch, query, cleaned):
    from fastapi.testclient import TestClient
    from unittest.mock import Mock
    from app import api
    from app.transcribe import Transcription, DICTATION_LIMITS
    monkeypatch.setattr(settings, "auth_token", "test-audio")
    speech = Mock(return_value=Transcription("texto original", "p"))
    cleanup = Mock(return_value={"text": "texto limpo"})
    monkeypatch.setattr(api, "transcribe_with_provider", speech)
    monkeypatch.setattr(api.dictation_bridge, "organize", cleanup)
    response = TestClient(api.app).post("/api/dictation/transcribe" + query,
        headers={"Authorization": "Bearer test-audio", "Content-Type": "audio/mp4", "X-Filename": "nota.m4a"},
        content=b"audio-m4a")
    assert response.status_code == 200
    speech.assert_called_once_with(b"audio-m4a", "nota.m4a", DICTATION_LIMITS)
    assert response.json() == {"text": "texto limpo" if cleaned else "texto original", "provider": "p"}
    if cleaned:
        cleanup.assert_called_once_with("texto original", None, mode=None, model=None, include_recent_messages=None, harness_allowed=True)
    else:
        cleanup.assert_not_called()
