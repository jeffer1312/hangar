import re
import pytest
from app import tts


HEX64 = re.compile(r"^[0-9a-f]{64}$")


def test_hash_invalido_nao_chega_no_disco():
    # O guard vive no endpoint; aqui garantimos que a forma esperada e mesmo hex de 64.
    assert HEX64.match(tts.hash_de("x", "v", "elevenlabs"))
    assert not HEX64.match("../../etc/passwd")
    assert not HEX64.match("voices")


def test_texto_vazio_depois_do_preparo(monkeypatch):
    from app.tts_text import preparar
    assert preparar("✅ →") == ""


def test_limite_de_caracteres_e_do_servidor(monkeypatch):
    # O limite da tela evita o susto; o do servidor e o que impede um cliente autenticado de mandar
    # megabytes numa requisicao. Aqui so travamos o default.
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: 0)
    from app.api import _tts_limite
    assert _tts_limite() == 5000
