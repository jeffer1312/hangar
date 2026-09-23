"""Atalhos configuráveis: validação do campo `shortcuts` e endpoint shell dispara-e-esquece.

O que esta suíte trava: config quebrada é recusada na GRAVAÇÃO com o item apontado (o resolve do
front é tolerante e cairia no conjunto nativo, calado), e o endpoint shell roda no cwd da sessão
sem esperar o comando terminar.
"""
import json
import time

import pytest
from fastapi.testclient import TestClient

from app import runtime_config as rc
from app.api import app
from app.config import settings


@pytest.fixture(autouse=True)
def _isolate(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


@pytest.fixture
def client():
    """Mesmo arranjo de test_api.py: sem armar o token, toda rota devolve 401."""
    previous = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(app)
    settings.auth_token = previous


def _as_json(*items) -> str:
    return json.dumps(list(items))


# --- validação na gravação -------------------------------------------------------------------

def test_valid_list_is_accepted_and_empty_restores_default():
    value = _as_json(
        {"id": "terminal", "type": "internal", "action": "terminal"},
        {"id": "a1", "type": "send_text", "label": "Relatório", "text": "/relatorio-pm",
         "icon": "emoji:📋", "send_direct": True},
        {"id": "a2", "type": "shell", "label": "Editor", "command": "code .", "confirm": True},
    )
    rc.aplicar({"shortcuts": value})
    assert rc.get("shortcuts") == value
    rc.aplicar({}, remover={"shortcuts"})
    assert rc.get("shortcuts") == ""


def test_broken_json_is_rejected():
    with pytest.raises(ValueError, match="JSON invalido"):
        rc.aplicar({"shortcuts": "{nao é json"})


def test_unknown_type_is_rejected_naming_the_item():
    with pytest.raises(ValueError, match="item 2"):
        rc.aplicar({"shortcuts": _as_json(
            {"id": "terminal", "type": "internal", "action": "terminal"},
            {"id": "x", "type": "foguete"},
        )})


def test_internal_with_unknown_action_is_rejected():
    with pytest.raises(ValueError, match="action desconhecida"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "internal", "action": "jetpack"})})


def test_item_missing_required_field_is_rejected():
    with pytest.raises(ValueError, match="sem texto"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "send_text", "label": "Oi"})})
    with pytest.raises(ValueError, match="sem comando"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "label": "Oi", "command": " "})})
    with pytest.raises(ValueError, match="sem rotulo"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "command": "true"})})
    with pytest.raises(ValueError, match="sem id"):
        rc.aplicar({"shortcuts": _as_json({"type": "internal", "action": "rodar"})})


def test_value_must_be_a_list():
    with pytest.raises(ValueError, match="esperado uma lista"):
        rc.aplicar({"shortcuts": json.dumps({"id": "x"})})


# --- endpoint shell --------------------------------------------------------------------------

def test_shell_requires_auth(client):
    assert client.post("/api/sessions/s/shortcut-shell",
                       json={"command": "true"}).status_code == 401


def test_shell_unknown_session_returns_404(client, monkeypatch):
    from app import api
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: None)
    r = client.post("/api/sessions/nada/shortcut-shell", json={"command": "true"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 404


def test_shell_runs_in_session_cwd_without_waiting(client, monkeypatch, tmp_path):
    from app import api
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    r = client.post("/api/sessions/s/shortcut-shell", json={"command": "pwd > prova.txt"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 202 and r.json() == {"ok": True}
    # dispara-e-esquece: a resposta volta antes do fim; espera-se o arquivo aparecer
    proof = tmp_path / "prova.txt"
    for _ in range(50):
        if proof.exists() and proof.read_text().strip():
            break
        time.sleep(0.1)
    assert proof.read_text().strip() == str(tmp_path)


def test_shell_empty_command_returns_400(client, monkeypatch, tmp_path):
    from app import api
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    r = client.post("/api/sessions/s/shortcut-shell", json={"command": "   "},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_shortcut_vazio"


def test_shell_oversized_command_is_rejected_before_running(client, monkeypatch, tmp_path):
    from app import api
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    r = client.post("/api/sessions/s/shortcut-shell", json={"command": "x" * 200_001},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 422
