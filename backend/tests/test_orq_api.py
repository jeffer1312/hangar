import json

import pytest
from fastapi.testclient import TestClient

from app import api as api_mod
from app import orq
from app.config import settings

_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def api_client(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "secret")
    return TestClient(api_mod.app)


def _semeia(tmp_path):
    d = tmp_path / "2026-08-22-paridade"
    d.mkdir(parents=True)
    (d / "eventos.jsonl").write_text("\n".join(json.dumps(x) for x in [
        {"ts": "2026-08-22T09:00:00-03:00", "tipo": "execucao_inicio",
         "plano": "p.md", "branch": "b", "gid": "g"},
        {"ts": "2026-08-22T09:05:00-03:00", "tipo": "task_inicio", "task": 1,
         "titulo": "T", "executor": "e", "par": "pi · x"},
        {"ts": "2026-08-22T11:30:00-03:00", "tipo": "veredito", "task": 1,
         "rodada": 1, "resultado": "aprova", "sessao": "r"},
    ]), encoding="utf-8")


def test_lista_e_detalhe(api_client, tmp_path, monkeypatch):
    monkeypatch.setattr(orq, "raiz_padrao", lambda: tmp_path)
    _semeia(tmp_path)
    r = api_client.get("/api/orq", headers=_H)
    assert r.status_code == 200
    corpo = r.json()
    assert corpo["execucoes"][0]["id"] == "2026-08-22-paridade"
    assert "eventos" not in corpo["execucoes"][0]["tasks"][0]
    assert "eventos_execucao" not in corpo["execucoes"][0]
    r2 = api_client.get("/api/orq/2026-08-22-paridade", headers=_H)
    assert r2.status_code == 200
    assert r2.json()["tasks"][0]["eventos"][0]["tipo"] == "task_inicio"
    r3 = api_client.get("/api/orq/nao-existe", headers=_H)
    assert r3.status_code == 404
    assert r3.json()["detail"]["code"] == "erro_nao_encontrado"


def test_sem_token_e_401(api_client):
    assert api_client.get("/api/orq").status_code == 401
