import pytest

from app.config import settings
from app.transcribe import build_multipart, transcribe, TranscribeError


def test_build_multipart_has_model_format_and_file():
    body, boundary = build_multipart("nota.webm", b"\x00\x01audio")
    # boundary aparece no corpo e todos os campos estao presentes e bem formados
    assert boundary.encode() in body
    assert b'name="model"' in body
    assert b"whisper-large-v3-turbo" in body
    assert b'name="response_format"' in body
    assert b'name="file"; filename="nota.webm"' in body
    assert b"\x00\x01audio" in body                 # bytes crus preservados
    assert body.rstrip().endswith(b"--" + boundary.encode() + b"--")


def test_transcribe_sem_chave_levanta_503(monkeypatch):
    monkeypatch.setattr(settings, "groq_api_key", "")
    with pytest.raises(TranscribeError) as ei:
        transcribe(b"audio", "a.webm")
    assert ei.value.status == 503
