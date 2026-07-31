import re
from unittest.mock import patch

from fastapi.testclient import TestClient

from app import tts, runtime_config
from app.config import settings


HEX64 = re.compile(r"^[0-9a-f]{64}$")
_HASH_FALSO = "a" * 64
_AUTH = {"Authorization": "Bearer secret"}


def _client():
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app)


def test_hash_invalido_nao_chega_no_disco():
    # O guard vive no endpoint; aqui garantimos que a forma esperada e mesmo hex de 64.
    assert HEX64.match(tts.hash_de("x", "v", "elevenlabs"))
    assert not HEX64.match("../../etc/passwd")
    assert not HEX64.match("voices")


def test_texto_vazio_depois_do_preparo():
    from app.tts_text import preparar
    assert preparar("✅ →") == ""


def test_limite_de_caracteres_e_do_servidor(monkeypatch):
    # O limite da tela evita o susto; o do servidor e o que impede um cliente autenticado de mandar
    # megabytes numa requisicao. Aqui so travamos o default.
    monkeypatch.setattr(runtime_config, "get", lambda campo: 0)
    from app.api import _tts_limite
    assert _tts_limite() == 5000


def test_409_confirma_e_passa_pra_200(monkeypatch):
    # Limite de aviso baixo pra nao precisar de texto gigante no teste.
    monkeypatch.setattr(runtime_config, "get", lambda campo: 10)
    client = _client()
    texto = "a" * 20  # acima do limite (10), abaixo do teto duro (40000)
    with patch("app.tts.sintetizar") as fake_sintetizar:
        r1 = client.post("/api/tts", json={"text": texto}, headers=_AUTH)
        assert r1.status_code == 409
        fake_sintetizar.assert_not_called()

        fake_sintetizar.return_value = (_HASH_FALSO, False)
        r2 = client.post("/api/tts", json={"text": texto, "confirm": True}, headers=_AUTH)
    assert r2.status_code == 200
    assert r2.json() == {"url": f"/api/tts/audio/{_HASH_FALSO}", "chars": 20, "cached": False}


def test_413_recusa_mesmo_com_confirm(monkeypatch):
    # Teto duro: nenhuma confirmacao passa por ele, diferente do 409 acima.
    monkeypatch.setattr(runtime_config, "get", lambda campo: 10)
    client = _client()
    texto = "a" * 40_001
    with patch("app.tts.sintetizar") as fake_sintetizar:
        r = client.post("/api/tts", json={"text": texto, "confirm": True}, headers=_AUTH)
        fake_sintetizar.assert_not_called()
    assert r.status_code == 413


def test_400_texto_que_nao_sobra_fala():
    client = _client()
    r = client.post("/api/tts", json={"text": "✅ →"}, headers=_AUTH)
    assert r.status_code == 400


def test_400_hash_invalido_na_rota_sem_tocar_disco():
    client = _client()
    with patch("app.tts.caminho_do_cache") as fake_caminho:
        r = client.get("/api/tts/audio/nao-e-hex-64-chars", headers=_AUTH)
        fake_caminho.assert_not_called()
    assert r.status_code == 400


def test_404_hash_bem_formado_sem_cache():
    client = _client()
    r = client.get(f"/api/tts/audio/{_HASH_FALSO}", headers=_AUTH)
    assert r.status_code == 404


def test_401_sem_token():
    client = _client()
    r = client.post("/api/tts", json={"text": "oi"})
    assert r.status_code == 401
