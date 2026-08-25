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


def test_exec_id_hostil_e_404_e_nao_500(api_client, tmp_path, monkeypatch):
    # Null byte e nome de drive do Windows: os dois tem que sair como "nao encontrei", nunca como
    # erro interno — 500 aqui e o endpoint admitindo que tentou abrir o caminho.
    monkeypatch.setattr(orq, "raiz_padrao", lambda: tmp_path)
    _semeia(tmp_path)
    for hostil in ("x%00y", "D:foo", ".."):
        r = api_client.get(f"/api/orq/{hostil}", headers=_H)
        assert r.status_code == 404, f"{hostil} devolveu {r.status_code}"
        # `..` nem chega ao endpoint: o roteador normaliza a URL e responde o 404 dele, com
        # `detail` string. Os outros dois passam pelo guard e trazem o code do app.
        detalhe = r.json()["detail"]
        if isinstance(detalhe, dict):
            assert detalhe["code"] == "erro_nao_encontrado"


def test_execucao_corrompida_nao_derruba_a_listagem(api_client, tmp_path, monkeypatch):
    monkeypatch.setattr(orq, "raiz_padrao", lambda: tmp_path)
    _semeia(tmp_path)
    podre = tmp_path / "2026-08-23-truncada"
    podre.mkdir()
    (podre / "eventos.jsonl").write_bytes(b'{"ts": "t", "tipo": "task_inicio", "titulo": "caf\xc3')
    r = api_client.get("/api/orq", headers=_H)
    assert r.status_code == 200
    assert [e["id"] for e in r.json()["execucoes"]] == ["2026-08-22-paridade"]
