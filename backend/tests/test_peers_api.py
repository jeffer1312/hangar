"""Borda HTTP dos peers — máquinas que este servidor alcança (aba Servidores).

A Task 1 só abre o espaço: a rota de listagem existe e devolve lista vazia, em vez de 404.
Quem preenche o conteúdo é a Task 5 (e a 8, no Lote B), dentro deste módulo. O módulo de ROTA é
`peers_api.py`: `app/peers.py` já existe e é a lógica de pareamento cross-server, não a rota.
"""
import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-peers"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def test_listagem_vazia(cli):
    r = cli.get("/api/peers", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == []
