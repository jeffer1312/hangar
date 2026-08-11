"""A borda HTTP das contas.

O que esta suíte trava: conta recém-criada aparece na lista ANTES do /login (senão o usuário não
tem onde abrir a sessão pra rodar o /login — impasse); com CP_CLAUDE_CONFIG_DIRS setado o POST
recusa em vez de devolver 200 pra uma conta que nunca vai aparecer no seletor; e apagar só aceita
pasta carimbada.
"""
import json

import pytest
from fastapi.testclient import TestClient

from app import contas
from app.api import app
from app.config import list_config_dirs, settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-contas"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    compartilhado = tmp_path / ".claude"
    (compartilhado / "projects").mkdir(parents=True)
    (compartilhado / "skills").mkdir()
    (compartilhado / ".credentials.json").write_text("{}", encoding="utf-8")
    (tmp_path / ".claude.json").write_text(json.dumps({"oauthAccount": {}}), encoding="utf-8")
    return tmp_path


def test_conta_sem_credencial_ainda_aparece_na_lista(casa):
    """Impasse que isto evita: sem credencial a pasta não passava no filtro, então a conta sumia
    justamente entre criar e logar — e o /login só pode ser rodado DENTRO de uma sessão dela."""
    contas.criar("conta2")
    assert str(casa / ".claude-conta2") in {c.path for c in list_config_dirs()}


def test_pasta_parecida_sem_marcador_e_sem_credencial_nao_entra(casa):
    (casa / ".claude-backup").mkdir()
    assert str(casa / ".claude-backup") not in {c.path for c in list_config_dirs()}


def test_criar_conta_pela_api(casa):
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["path"] == str(casa / ".claude-conta2")
    assert r.json()["label"] == "conta2"


def test_criar_conta_repetida_devolve_409(casa):
    cli = TestClient(app)
    cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    r = cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409


def test_nome_invalido_devolve_400(casa):
    r = TestClient(app).post("/api/claude-configs", json={"nome": "Conta 2"}, headers=AUTH)
    assert r.status_code == 400


def test_com_lista_fixa_de_config_dirs_o_post_recusa(casa, monkeypatch):
    """Com CP_CLAUDE_CONFIG_DIRS setado, list_config_dirs ignora o auto-scan: a conta seria criada,
    nunca apareceria no seletor, e mandar o path mesmo assim daria 400 na criação de sessão.
    Recusar aqui, com o motivo, é a única saída que não mente."""
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", f"padrao:{casa / '.claude'}")
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409
    assert "CP_CLAUDE_CONFIG_DIRS" in r.json()["detail"]


def test_apagar_conta(casa):
    cli = TestClient(app)
    cli.post("/api/claude-configs", json={"nome": "cotna2"}, headers=AUTH)
    assert cli.delete("/api/claude-configs/cotna2", headers=AUTH).status_code == 200
    assert not (casa / ".claude-cotna2").exists()


def test_apagar_pasta_nao_carimbada_devolve_404(casa):
    (casa / ".claude-backup").mkdir()
    assert TestClient(app).delete("/api/claude-configs/backup", headers=AUTH).status_code == 404
    assert (casa / ".claude-backup").is_dir()
