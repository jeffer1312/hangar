from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings
import app.api as api_mod
from app.models import SessionInfo

_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def api_client(monkeypatch):
    """Espelha tests/test_api.py:57 — mesma fixture real (client generico + monkeypatch de
    _session_exists), nao a `client` descartavel do topo do arquivo (so tem /ping)."""
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    from app.api import app
    return TestClient(app)


PLAN = "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: verificação manual**\n"


def test_plan_404_sem_plano(api_client):
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd="/tmp")]), \
         patch("app.api.plan_progress", return_value=None):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404
    # so status_code==404 tambem passa pra rota inexistente (404 generico do FastAPI) - o corpo
    # e o que prova que foi ESTA rota, com plan_progress==None, que respondeu.
    assert r.json()["detail"]["code"] == "erro_sem_plano_ativo"


def test_plan_404_sem_sessao(api_client):
    with patch("app.api.registry.list", return_value=[]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404
    assert r.json()["detail"]["code"] == "erro_sessao_inexistente"


def test_plan_devolve_detalhe_e_markdown(api_client, tmp_path):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True)
    (tmp_path / ".git").mkdir()
    (d / "2026-07-29-plano.md").write_text(PLAN, encoding="utf-8")
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 200
    j = r.json()
    assert j["name"] == "plano"
    assert (j["done"], j["total"]) == (1, 2)
    assert j["complete"] is False
    assert j["tasks"][0]["steps"][0]["done"] is True
    assert j["tasks"][0]["steps"][1]["manual"] is True
    # markdown cru viaja na resposta: o GET /file so serve path citado no transcript (api.py:2196),
    # e um plano descoberto por glob nunca aparece la.
    assert "### Task 1" in j["markdown"]
    # stem e idx: as duas chaves que o cliente devolve pra marcar step e arquivar. O `name`, com a
    # data cortada, nao reabre o arquivo.
    assert j["stem"] == "2026-07-29-plano"
    assert [s["idx"] for s in j["tasks"][0]["steps"]] == [0, 1]


def _repo(tmp_path):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True)
    (tmp_path / ".git").mkdir()
    (d / "2026-07-29-plano.md").write_text(PLAN, encoding="utf-8")
    return d


def test_plan_step_marca_e_devolve_o_progresso_novo(api_client, tmp_path):
    d = _repo(tmp_path)
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.post("/api/sessions/s/plan-step", headers=_H,
                            json={"stem": "2026-07-29-plano", "idx": 1, "done": True})
    assert r.status_code == 200
    assert r.json() == {"done": 2, "total": 2, "complete": True}
    assert "- [x] **Step 2:" in (d / "2026-07-29-plano.md").read_text(encoding="utf-8")


def test_plan_step_fora_da_faixa_e_409_com_texto(api_client, tmp_path):
    _repo(tmp_path)
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.post("/api/sessions/s/plan-step", headers=_H,
                            json={"stem": "2026-07-29-plano", "idx": 99, "done": True})
    # 409 e nao 500: e "o plano nao serve", nao bug do servidor — e a UI precisa mostrar o motivo.
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_marcar_step"


def test_plan_step_recusa_stem_com_traversal(api_client, tmp_path):
    _repo(tmp_path)
    (tmp_path / "fora.md").write_text(PLAN, encoding="utf-8")
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.post("/api/sessions/s/plan-step", headers=_H,
                            json={"stem": "../../../fora", "idx": 0, "done": False})
    assert r.status_code == 409
    assert "- [x] **Step 1:" in (tmp_path / "fora.md").read_text(encoding="utf-8")


def test_plan_archive_move_pra_feitos(api_client, tmp_path):
    d = _repo(tmp_path)
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.post("/api/sessions/s/plan-archive", headers=_H,
                            json={"stem": "2026-07-29-plano"})
    assert r.status_code == 200
    assert r.json()["moved"] == ["2026-07-29-plano.md"]
    assert (d / "feitos" / "2026-07-29-plano.md").is_file()
    assert not (d / "2026-07-29-plano.md").exists()


def test_plan_archive_plano_inexistente_e_409(api_client, tmp_path):
    _repo(tmp_path)
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.post("/api/sessions/s/plan-archive", headers=_H, json={"stem": "nada"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_arquivar_plano"
