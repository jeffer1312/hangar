"""Endpoint da paleta: so responde pra quem esta NA MAQUINA do backend."""
import pytest
from fastapi.testclient import TestClient

from app import desktop_palette
from app.api import app
from app.config import settings


@pytest.fixture(autouse=True)
def _token():
    settings.auth_token = "secret"


def _cli(ip: str) -> TestClient:
    # `TestClient(app)` manda `client.host == "testclient"`, que NAO e loopback e daria 403 em
    # tudo. O jeito certo neste repo e forjar o par (ip, porta) — ver tests/test_auth_backoff.py.
    return TestClient(app, client=(ip, 12345))


AUTH = {"Authorization": "Bearer secret"}


def test_loopback_recebe_a_paleta(tmp_path, monkeypatch, paleta_azul):
    f = tmp_path / "material_colors.scss"
    f.write_text(paleta_azul, encoding="utf-8")
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    r = _cli("127.0.0.1").get("/api/desktop/palette", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["cores"]["background"] == "#111318"
    assert r.json()["escuro"] is True


def test_de_fora_da_maquina_e_403(tmp_path, monkeypatch, paleta_azul):
    f = tmp_path / "material_colors.scss"
    f.write_text(paleta_azul, encoding="utf-8")
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    r = _cli("192.168.15.28").get("/api/desktop/palette", headers=AUTH)
    assert r.status_code == 403


def test_sem_arquivo_e_404_e_nao_500(tmp_path, monkeypatch):
    # O front usa o 404 pra ESCONDER a opcao. Um 500 aqui viraria uma opcao que aparece e nao
    # funciona, que e o pior dos dois mundos.
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: tmp_path / "nada.scss")
    r = _cli("127.0.0.1").get("/api/desktop/palette", headers=AUTH)
    assert r.status_code == 404


def test_porteiro_sem_client_rejeita():
    """Sem peer identificavel a resposta e NAO: deixar passar transformaria o unico portao num `if`
    que qualquer transporte estranho contorna."""
    from fastapi import HTTPException
    from app.auth import require_loopback

    class _Req:
        client = None

    with pytest.raises(HTTPException) as e:
        require_loopback(_Req())
    assert e.value.status_code == 403
