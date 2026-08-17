"""Borda HTTP do estado de conta — a lista de contas do servidor (aba Contas).

A Task 1 só abre o espaço: a rota de listagem existe e devolve lista vazia, em vez de 404.
Quem preenche o conteúdo é a Task 4 (e a 7, no Lote B), dentro deste módulo.
"""
import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-conta-estado"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def test_listagem_vazia(cli):
    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == []
