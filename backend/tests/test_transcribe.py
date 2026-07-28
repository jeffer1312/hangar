import pytest

from app import runtime_config
from app.config import settings
from app.transcribe import build_multipart, transcribe, TranscribeError


def _sem_chave(monkeypatch):
    """Chave ausente de verdade: a `transcribe()` lê do runtime_config (arquivo editável pela UI),
    não mais de `settings` — mockar só o settings deixava a chave REAL do usuário valendo, e o
    teste do caminho "sem chave" ia bater na Groq de verdade (voltava 502 da API, não 503)."""
    monkeypatch.setattr(settings, "groq_api_key", "")
    monkeypatch.setattr(runtime_config, "get", lambda campo: "" if campo == "groq_api_key" else getattr(settings, campo, None))


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
    _sem_chave(monkeypatch)
    with pytest.raises(TranscribeError) as ei:
        transcribe(b"audio", "a.webm")
    assert ei.value.status == 503


def test_transcribe_ignora_filename_do_cliente(monkeypatch):
    # Filename malicioso (aspas + CRLF tentando injetar um campo 'model') NAO pode vazar pro multipart:
    # o nome enviado a Groq e fixo no servidor (audio.<ext sanitizada>).
    monkeypatch.setattr(settings, "groq_api_key", "k")
    monkeypatch.setattr(runtime_config, "get", lambda campo: "k" if campo == "groq_api_key" else getattr(settings, campo, None))
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self): return b"ok"

    monkeypatch.setattr(
        "app.transcribe.urllib.request.urlopen",
        lambda req, timeout=None: captured.setdefault("body", req.data) and None or FakeResp(),
    )
    evil = 'x".webm\r\nContent-Disposition: form-data; name="model"\r\n\r\nhacked\r\n'
    transcribe(b"audio", evil)
    body = captured["body"]
    assert body.count(b'name="model"') == 1      # so o campo model legitimo, nada injetado
    assert b"hacked" not in body
    assert b'filename="audio.' in body           # nome fixo do servidor
