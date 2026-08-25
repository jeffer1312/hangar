import pytest

from app import runtime_config
from app.config import settings
from app import transcribe as mod_transcribe
from app.transcribe import build_multipart, transcribe, vocabulario, TranscribeError


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
    assert b'name="language"' in body
    assert b'name="file"; filename="nota.webm"' in body
    assert b"\x00\x01audio" in body                 # bytes crus preservados
    assert body.rstrip().endswith(b"--" + boundary.encode() + b"--")


def test_build_multipart_sem_vocab_nao_manda_prompt():
    # Campo `prompt` vazio nao pode ir: a Whisper trataria a string vazia como vocabulario valido,
    # e o objetivo do parametro opcional e o payload ficar identico ao de antes quando nao ha nada.
    body, _ = build_multipart("nota.webm", b"a")
    assert b'name="prompt"' not in body


def test_build_multipart_com_vocab_manda_prompt():
    body, _ = build_multipart("nota.webm", b"a", "hangar-send, Hangar")
    assert b'name="prompt"' in body
    assert "hangar-send, Hangar".encode() in body


def test_vocabulario_soma_base_e_config(monkeypatch):
    monkeypatch.setattr(mod_transcribe.runtime_config, "get",
                        lambda campo: "Acme, projeto-x" if campo == "ditado_vocabulario" else None)
    v = vocabulario()
    assert "hangar-send" in v          # a base do app continua
    assert "Acme, projeto-x" in v  # e o do usuario entra junto


def test_transcribe_manda_idioma_e_vocabulario_no_corpo_real(monkeypatch):
    """A LIGACAO, nao as pecas. build_multipart e vocabulario ja tem teste cada um, mas nenhum via
    o corpo que transcribe() realmente monta — trocar a chamada de volta pra
    `build_multipart(nome, content)` deixava a suite verde com o vocabulario indo pro lixo. E
    exatamente o tipo de fio solto que fez o mic do card ficar seis meses sem a limpeza: as duas
    pontas certas, o meio nunca testado."""
    monkeypatch.setattr(settings, "groq_api_key", "k")
    monkeypatch.setattr(mod_transcribe.runtime_config, "get",
                        lambda campo: {"groq_api_key": "k",
                                       "ditado_vocabulario": "Acme, projeto-x"}.get(campo))
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self): return b"ok"

    def fake_urlopen(req, timeout=None):
        captured["body"] = req.data
        return FakeResp()

    monkeypatch.setattr("app.transcribe.urllib.request.urlopen", fake_urlopen)
    transcribe(b"audio", "a.webm")
    body = captured["body"]
    assert b'name="language"' in body
    assert mod_transcribe.IDIOMA.encode() in body
    assert b'name="prompt"' in body
    assert b"hangar-send" in body                    # a base do app chegou
    assert "Acme, projeto-x".encode() in body   # e o vocabulario do usuario tambem


def test_vocabulario_trunca_e_GRITA(monkeypatch, caplog):
    # Lista gigante nao pode passar em silencio: a API corta em ~224 tokens e perderia o fim sem
    # avisar. O corte aqui e explicito, testado — e RUIDOSO. Cortar calado seria reimplementar
    # exatamente o defeito da API que este codigo existe pra contornar.
    monkeypatch.setattr(mod_transcribe.runtime_config, "get",
                        lambda campo: "palavra, " * 500 if campo == "ditado_vocabulario" else None)
    with caplog.at_level("WARNING"):
        assert len(vocabulario()) == mod_transcribe._VOCAB_MAX
    assert "cortado" in caplog.text


def test_vocabulario_no_teto_nao_grita(monkeypatch, caplog):
    # O aviso so pode aparecer quando algo foi PERDIDO. Gritar no caso normal treina quem le o log
    # a ignorar o aviso — e ai ele nao serve pra nada no dia em que importa.
    monkeypatch.setattr(mod_transcribe.runtime_config, "get",
                        lambda campo: "x" * mod_transcribe.VOCAB_USUARIO_MAX
                        if campo == "ditado_vocabulario" else None)
    with caplog.at_level("WARNING"):
        assert len(vocabulario()) == mod_transcribe._VOCAB_MAX
    assert caplog.text == ""


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
