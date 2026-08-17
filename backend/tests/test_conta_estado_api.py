"""Borda HTTP do estado de conta — a lista de contas do servidor (aba Contas).

Dono: Task 4 (recorte 16/08). A Task 7 (Lote B) acrescenta aqui quando ligar o botão Entrar.

Isolamento por monkeypatch no módulo conta_estado (nunca disco/CLI real): a lista de contas
vem de `list_config_dirs` fake, o login de `_auth_status` fake e o limite de `_limite` fake.
O login remoto (Task 7) é testado contra `login_conta` fake: a janela escondida do tmux e a
CLI do claude nunca são chamadas de verdade aqui.
"""
import pytest
from fastapi.testclient import TestClient

from app import conta_estado, login_conta
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
    # Estado de módulo do login remoto: a janela da tentativa em voo não pode vazar.
    monkeypatch.setattr(login_conta, "_tentativas", {}, raising=False)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


@pytest.fixture
def login_fake(monkeypatch):
    """Trocáveis do login remoto (tmux/CLI reais nunca tocados) + espelho das chamadas."""
    espelho = {"iniciar": [], "passo": [], "confirmar": [], "cancelar": []}

    def _iniciar(conta, cwd):
        espelho["iniciar"].append((conta, cwd))
        return {"ok": True}

    def _passo(conta):
        espelho["passo"].append(conta)
        return {"etapa": "aguardando", "url": "https://claude.com/cai/oauth/authorize"}

    def _confirmar(conta, codigo):
        espelho["confirmar"].append((conta, codigo))
        return {"ok": True, "email": "u@example.com", "plano": "max"}

    def _cancelar(conta):
        espelho["cancelar"].append(conta)
        return {"ok": True}

    monkeypatch.setattr(login_conta, "iniciar", _iniciar)
    monkeypatch.setattr(login_conta, "passo", _passo)
    monkeypatch.setattr(login_conta, "confirmar", _confirmar)
    monkeypatch.setattr(login_conta, "cancelar", _cancelar)
    return espelho


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


# ------------------------------------------------------------------ login remoto (Task 7)


def test_iniciar_login_abre_janela_escondida(cli, login_fake, monkeypatch):
    # A conta existe: a rota consulta list_config_dirs e abre a janela com o cwd da conta.
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_cfg("/home/u/.claude-testes", "testes", False)])
    r = cli.post("/api/conta-estado/testes/login", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == {"ok": True}
    assert login_fake["iniciar"] == [("testes", "/home/u/.claude-testes")]


def test_iniciar_login_sem_conta_devolve_404(cli, login_fake, monkeypatch):
    # Sem a conta na lista, a rota recusa ANTES de tocar no login (janela nunca abre).
    monkeypatch.setattr(conta_estado, "list_config_dirs", lambda: [])
    r = cli.post("/api/conta-estado/nao-existe/login", headers=AUTH)
    assert r.status_code == 404
    assert "não existe" in r.json()["detail"]["msg"]
    assert login_fake["iniciar"] == []


def test_iniciar_login_401_sem_credencial(cli):
    # Régua do árbitro 16/08: rota que serve e-mail e plano de conta — sem o caso, alguém
    # remove o require_auth um dia e nada acusa.
    r = cli.post("/api/conta-estado/testes/login")
    assert r.status_code == 401


def test_passar_codigo_confirma_e_devolve_email_e_plano(cli, login_fake):
    r = cli.post("/api/conta-estado/testes/login/codigo",
                 json={"codigo": "CODE-123"}, headers=AUTH)
    assert r.status_code == 200
    corpo = r.json()
    assert corpo["ok"] is True
    assert corpo["email"] == "u@example.com"
    assert corpo["plano"] == "max"
    assert login_fake["confirmar"] == [("testes", "CODE-123")]


def test_codigo_sem_tentativa_devolve_erro(cli, login_fake, monkeypatch):
    monkeypatch.setattr(login_conta, "confirmar",
                        lambda conta, codigo: (_ for _ in ()).throw(RuntimeError("sem tentativa")))
    r = cli.post("/api/conta-estado/testes/login/codigo",
                 json={"codigo": "CODE-123"}, headers=AUTH)
    assert r.status_code == 409
    assert "sem tentativa" in r.json()["detail"]["msg"]


def test_codigo_401_sem_credencial(cli):
    r = cli.post("/api/conta-estado/testes/login/codigo", json={"codigo": "CODE-123"})
    assert r.status_code == 401


def test_consultar_passo_do_login(cli, login_fake):
    r = cli.get("/api/conta-estado/testes/login/passo", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["etapa"] == "aguardando"
    assert "https://claude.com/cai/oauth/authorize" in r.json()["url"]
    assert login_fake["passo"] == ["testes"]


def test_passo_401_sem_credencial(cli):
    r = cli.get("/api/conta-estado/testes/login/passo")
    assert r.status_code == 401


def test_cancelar_login_mata_a_janela(cli, login_fake):
    r = cli.post("/api/conta-estado/testes/login/cancelar", headers=AUTH)
    assert r.status_code == 200
    assert login_fake["cancelar"] == ["testes"]


def test_cancelar_401_sem_credencial(cli):
    r = cli.post("/api/conta-estado/testes/login/cancelar")
    assert r.status_code == 401