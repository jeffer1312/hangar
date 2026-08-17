"""Borda HTTP do estado de conta — a lista de contas do servidor (aba Contas).

Dono: Task 4 (recorte 16/08). A Task 7 (Lote B) acrescenta aqui quando ligar o botão Entrar.

Isolamento por monkeypatch no módulo conta_estado (nunca disco/CLI real): a lista de contas
vem de `list_config_dirs` fake, o login de `_auth_status` fake e o limite de `_limite` fake.
"""
import pytest
from fastapi.testclient import TestClient

from app import conta_estado
from app.api import app
from app.config import settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-conta-estado"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    # Cache curto do login é estado de módulo: limpar entre testes pra um path fake não
    # vazar o login de outro teste com o mesmo caminho.
    monkeypatch.setattr(conta_estado, "_login_cache", {})
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def _cfg(path: str, label: str, active: bool):
    return type("Cfg", (), {"path": path, "label": label, "active": active})()


def test_listagem_traz_estado_por_conta(cli, monkeypatch):
    monkeypatch.setattr(conta_estado, "list_config_dirs", lambda: [
        _cfg("/home/u/.claude-a", "a", True),
        _cfg("/home/u/.claude-b", "b", False),
    ])
    monkeypatch.setattr(conta_estado, "_auth_status",
                        lambda p: {"loggedIn": True, "email": "a@example.com",
                                   "subscriptionType": "max"})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(
                            estado="lido", linha="⚡5h:3% 📅7d:1%", ts=100.0, idade_s=25.0))

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    contas = r.json()
    assert len(contas) == 2
    a = contas[0]
    assert a["path"] == "/home/u/.claude-a"
    assert a["label"] == "a"
    assert a["active"] is True
    assert a["login"]["estado"] == "ok"
    assert a["login"]["email"] == "a@example.com"
    assert a["login"]["plano"] == "max"
    assert a["limite"]["estado"] == "lido"
    assert a["limite"]["linha"] == "⚡5h:3% 📅7d:1%"
    assert a["limite"]["idade_s"] == 25.0
    b = contas[1]
    assert b["active"] is False


def test_conta_deslogada_continua_na_lista(cli, monkeypatch):
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_cfg("/home/u/.claude-testes", "testes", False)])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: {"loggedIn": False})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    contas = r.json()
    assert len(contas) == 1
    assert contas[0]["label"] == "testes"
    assert contas[0]["login"]["estado"] == "ok"
    assert contas[0]["login"]["loggedIn"] is False
    assert contas[0]["login"]["email"] is None
    assert contas[0]["limite"]["estado"] == "sem_leitura"


def test_cli_indisponivel_nao_derruba_lista(cli, monkeypatch):
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_cfg("/home/u/.claude-x", "x", False)])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: None)
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    contas = r.json()
    assert len(contas) == 1
    assert contas[0]["login"]["estado"] == "indisponivel"
    assert contas[0]["login"]["motivo"] == "cli-indisponivel"


def test_sem_leitura_e_explicito_nao_zero(cli, monkeypatch):
    # Régua: conta sem leitura de limite devolve o estado nomeado, nunca 0 nem ausente.
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_cfg("/home/u/.claude-x", "x", False)])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: {"loggedIn": True})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    limite = r.json()[0]["limite"]
    assert limite["estado"] == "sem_leitura"
    assert "linha" not in limite or limite["linha"] is None


def test_401_sem_credencial(cli):
    # Régua do árbitro 16/08: esta rota serve e-mail e plano de conta — sem o caso, alguém
    # remove o require_auth um dia e nada acusa.
    r = cli.get("/api/conta-estado")
    assert r.status_code == 401