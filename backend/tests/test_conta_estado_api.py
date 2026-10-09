"""Borda HTTP do estado de conta — a lista de contas do servidor (aba Contas).

Dono: Task 4 (recorte 16/08). A Task 7 (Lote B) acrescenta aqui quando ligar o botão Entrar.

Isolamento por monkeypatch no módulo conta_estado (nunca disco/CLI real): a lista de contas
vem de `list_config_dirs` fake, o login de `_auth_status` fake e o limite de `_limite` fake.
O login remoto (Task 7) é testado contra `login_conta` fake: a janela escondida do tmux e a
CLI do claude nunca são chamadas de verdade aqui.
"""
import shutil

import pytest
from fastapi.testclient import TestClient

from app import conta_estado, contas, login_conta
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


def _conta_carimbada(tmp_path, nome: str, active: bool = False):
    """Aba lista só pasta de conta de verdade (Task 4): o catálogo é fake, mas `e_conta`
    roda de verdade — o path precisa ser uma pasta real com marcador real."""
    d = tmp_path / f".claude-{nome}"
    d.mkdir()
    (d / contas.MARCADOR).write_text("", encoding="utf-8")
    return _cfg(str(d), nome, active)


def test_listagem_traz_estado_por_conta(cli, monkeypatch, tmp_path):
    monkeypatch.setattr(conta_estado, "list_config_dirs", lambda: [
        _conta_carimbada(tmp_path, "a", active=True),
        _conta_carimbada(tmp_path, "b"),
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
    assert a["path"] == str(tmp_path / ".claude-a")
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


def test_conta_deslogada_continua_na_lista(cli, monkeypatch, tmp_path):
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_conta_carimbada(tmp_path, "testes")])
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


def test_cli_indisponivel_nao_derruba_lista(cli, monkeypatch, tmp_path):
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_conta_carimbada(tmp_path, "x")])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: None)
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    contas = r.json()
    assert len(contas) == 1
    assert contas[0]["login"]["estado"] == "indisponivel"
    assert contas[0]["login"]["motivo"] == "cli-indisponivel"


def test_sem_leitura_e_explicito_nao_zero(cli, monkeypatch, tmp_path):
    # Régua: conta sem leitura de limite devolve o estado nomeado, nunca 0 nem ausente.
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_conta_carimbada(tmp_path, "x")])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: {"loggedIn": True})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    limite = r.json()[0]["limite"]
    assert limite["estado"] == "sem_leitura"
    assert "linha" not in limite or limite["linha"] is None


def test_login_em_cache_ate_a_credencial_mudar(monkeypatch, tmp_path):
    from types import SimpleNamespace
    chamadas = []
    monkeypatch.setattr(conta_estado, "_auth_status",
                        lambda p: chamadas.append(p) or {"loggedIn": True})
    monkeypatch.setattr(conta_estado.renova_token, "refresh_expires_at", lambda p: None)
    cfg = SimpleNamespace(path=str(tmp_path))
    cred = tmp_path / ".credentials.json"
    cred.write_text("{}")
    conta_estado._login_de(cfg)
    monkeypatch.setattr(conta_estado, "_LOGIN_TTL", 0)  # só o arquivo segura o cache
    conta_estado._login_de(cfg)
    assert len(chamadas) == 1
    cred.write_text('{"novo": 1}')
    conta_estado._login_de(cfg)
    assert len(chamadas) == 2


def test_401_sem_credencial(cli):
    # Régua do árbitro 16/08: esta rota serve e-mail e plano de conta — sem o caso, alguém
    # remove o require_auth um dia e nada acusa.
    r = cli.get("/api/conta-estado")
    assert r.status_code == 401


def test_backup_sem_marcador_nao_aparece_na_aba(cli, monkeypatch, tmp_path):
    """Task 4: a pasta de backup (login + projects de verdade, SEM o marcador .hangar-conta)
    sai da lista de contas — era a linha que a própria tela não conseguia apagar (o DELETE
    exige conta carimbada e devolve 404). O catálogo continua vendo ela (o GET
    /api/claude-configs é outro consumidor), a ABA não."""
    backup = tmp_path / ".claude-work.bak-2026-08-12-1538"
    backup.mkdir()
    (backup / ".credentials.json").write_text("{}", encoding="utf-8")
    (backup / "projects" / "ws").mkdir(parents=True)
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: {"loggedIn": True})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))
    monkeypatch.setattr(conta_estado, "list_config_dirs", lambda: [
        _conta_carimbada(tmp_path, "valida"),
        _cfg(str(backup), "work.bak-2026-08-12-1538", False),
    ])

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert r.status_code == 200
    labels = [c["label"] for c in r.json()]
    assert labels == ["valida"]
    assert "work.bak-2026-08-12-1538" not in labels


def test_default_ativo_continua_na_aba_sem_marcador(cli, monkeypatch, tmp_path):
    """Task 4: o `~/.claude` default (a base do app) não é carimbado, mas é `active` e aparece
    na aba até hoje (com "em uso"; o DELETE dela recusa com 409 e motivo). Filtrá-lo fora seria
    "some da tela sem explicação" — o critério é e_conta OU active."""
    monkeypatch.setattr(conta_estado, "list_config_dirs", lambda: [
        _cfg(str(tmp_path / ".claude"), "default", True),
    ])
    monkeypatch.setattr(conta_estado, "_auth_status", lambda p: {"loggedIn": True})
    monkeypatch.setattr(conta_estado, "_limite",
                        lambda p: conta_estado.EstadoLimite(estado="sem_leitura"))

    r = cli.get("/api/conta-estado", headers=AUTH)
    assert [c["label"] for c in r.json()] == ["default"]


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


@pytest.mark.parametrize("code", ["synthetic\nsecond-command", "synthetic\rsecond-command", "synthetic\x00tail"])
def test_confirmation_rejects_control_characters_before_io(cli, login_fake, code):
    response = cli.post("/api/conta-estado/testes/login/codigo", json={"codigo": code}, headers=AUTH)
    assert response.status_code == 422
    assert code not in response.text
    assert "input" not in response.json()["detail"][0]
    assert login_fake["confirmar"] == []


def test_codigo_sem_tentativa_devolve_erro(cli, login_fake, monkeypatch):
    monkeypatch.setattr(login_conta, "confirmar",
                        lambda conta, codigo: (_ for _ in ()).throw(RuntimeError("sem tentativa")))
    r = cli.post("/api/conta-estado/testes/login/codigo",
                 json={"codigo": "CODE-123"}, headers=AUTH)
    assert r.status_code == 409
    assert "sem tentativa" in r.json()["detail"]["msg"]


@pytest.mark.parametrize("rota,funcao", [("login", "iniciar"), ("login/codigo", "confirmar")])
@pytest.mark.parametrize("falha", [OSError("leitura falhou"), ValueError("JSON inválido")])
def test_login_informa_erro_de_leitura_da_credencial(cli, monkeypatch, rota, funcao, falha):
    monkeypatch.setattr(conta_estado, "list_config_dirs",
                        lambda: [_cfg("/home/u/.claude-testes", "testes", False)])

    def falhar(*args):
        raise falha

    monkeypatch.setattr(login_conta, funcao, falhar)
    r = cli.post(f"/api/conta-estado/testes/{rota}", json={"codigo": "CODE-123"}, headers=AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_login_credencial_ilegivel"


def test_codigo_401_sem_credencial(cli):
    r = cli.post("/api/conta-estado/testes/login/codigo", json={"codigo": "CODE-123"})
    assert r.status_code == 401


def test_consultar_passo_do_login(cli, login_fake):
    r = cli.get("/api/conta-estado/testes/login/passo", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["etapa"] == "aguardando"
    assert "https://claude.com/cai/oauth/authorize" in r.json()["url"]
    assert login_fake["passo"] == ["testes"]


def test_passo_que_desistiu_devolve_409(cli, monkeypatch):
    def _passo(conta):
        raise RuntimeError("não consegui reler o estado da conta")

    monkeypatch.setattr(login_conta, "passo", _passo)
    r = cli.get("/api/conta-estado/testes/login/passo", headers=AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_login_nao_confirmado"


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

@pytest.mark.skipif(shutil.which("claude") is None,
                    reason="a régua deste caso é a CLI REAL; sem ela (CI) não há o que medir")
def test_auth_status_real_conta_virgem_e_ok_deslogada(monkeypatch, tmp_path):
    # B1 — a régua "fonte REAL": a CLI existe nesta máquina e responde JSON válido com
    # rc=1 numa conta virgem (deslogada de verdade). `_auth_status` NÃO pode jogar isso
    # fora por causa do rc: é exatamente o estado em que o botão Entrar precisa aparecer.
    # O config dir é um diretório vazio de verdade (a conta virgem); o HOME vai pro tmp
    # pra não tocar nas contas reais da máquina.
    monkeypatch.setenv("HOME", str(tmp_path))
    d = tmp_path / ".claude-conta-virgem"
    d.mkdir()
    bruto = conta_estado._auth_status(d)
    assert bruto is not None
    assert bruto.get("loggedIn") is False
    estado = conta_estado._estado_login(bruto)
    assert estado.estado == "ok"
    assert estado.loggedIn is False

def test_rust_login_requires_changed_token_and_authenticated_identity(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    try:
        credential = account / ".credentials.json"
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        reply = account / "auth-reply.json"
        reply.write_text(json.dumps({"loggedIn": True, "email": "fixture@example.test", "subscriptionType": "pro"}), encoding="utf-8")
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        response = server.request("POST", path)
        assert response.status_code == 200, "o Rust deve abrir o login sem delegar ao handler Python bloqueado"
        assert server.request("GET", path + "/passo").json() == {
            "etapa": "aguardando", "url": "https://claude.ai/oauth/authorize?fixture=1", "email": None, "plano": None}
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        reply.write_text('{"loggedIn":false}', encoding="utf-8")
        assert server.request("GET", path + "/passo").json()["etapa"] == "aguardando"
        reply.write_text(json.dumps({"loggedIn": True, "email": "fixture@example.test", "subscriptionType": "pro"}), encoding="utf-8")
        assert server.request("GET", path + "/passo").json() == {
            "etapa": "concluido", "url": None, "email": "fixture@example.test", "plano": "pro"}
        assert json.loads((account / ".claude.json").read_text(encoding="utf-8"))["hasCompletedOnboarding"] is True
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_rust_restart_closes_only_the_abandoned_login_window(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        assert server.request("POST", path).status_code == 200
        old = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert len(old) == 1
        server.close()
        server = RustClaude(reference)
        assert server.request("GET", path + "/passo").json()["etapa"] == "idle"
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
        assert server.request("POST", path).status_code == 200
        new = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert len(new) == 1 and new != old
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
    finally:
        server.close()
        reference.close()

def test_rust_old_confirmation_cannot_clean_or_complete_a_new_attempt(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    from concurrent.futures import ThreadPoolExecutor
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    pool = ThreadPoolExecutor()
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        account = reference.root / ".claude-work"
        credential = account / ".credentials.json"
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        (account / "auth-reply.json").write_text('{"loggedIn":true}', encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        old = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        confirming = pool.submit(server.request, "POST", path + "/codigo", {"codigo": "synthetic-code-not-in-argv"})
        assert reference.request("GET", "/__contract__/wait-claude-code").json()["entered"]
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert server.request("POST", path).status_code == 200
        new = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert new != old and len(new) == 1
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        assert confirming.result(timeout=20).status_code == 409
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == new
        assert server.request("GET", path + "/passo").json()["etapa"] == "concluido"
        assert "synthetic-code-not-in-argv" not in json.dumps(reference.calls())
        assert "synthetic-code-not-in-argv" not in (reference.root / "worker.log").read_text(encoding="utf-8")
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()
        pool.shutdown(wait=True)


@pytest.mark.parametrize("reply", ['{"loggedIn":"true"}', 'invalid-json'])
def test_rust_unreadable_identity_cleans_attempt_without_onboarding(tmp_path, reply):
    from accounts_contract import PythonReference, RustClaude
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        assert server.request("POST", path).status_code == 200
        (account / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        (account / "auth-reply.json").write_text(reply, encoding="utf-8")
        response = server.request("GET", path + "/passo")
        assert response.status_code == 409
        assert not (account / ".claude.json").exists()
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
    finally:
        server.close()
        reference.close()

def test_invalidation_uses_canonical_account_identity(tmp_path):
    from app import cotas
    directory = tmp_path / "Conta"
    directory.mkdir()
    actual = str(directory)
    alias = str(directory / ".." / "Conta")
    state = conta_estado._estado_login({"loggedIn": True})
    with conta_estado._login_lock:
        conta_estado._login_cache[actual] = (0, state, None)
    with cotas._lock:
        cotas._cache["claude:" + actual] = (0, "fixture")
    conta_estado.esquecer_conta(alias)
    assert actual not in conta_estado._login_cache
    assert "claude:" + actual not in cotas._cache



def test_python_consumer_uses_private_rust_auth_for_equivalent_account_paths(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    from urllib.parse import quote
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    try:
        account = reference.root / ".claude-work"
        (account / "auth-reply.json").write_text('{"loggedIn":true,"email":"fixture@example.test"}', encoding="utf-8")
        address = server.request("GET", "/__hangar_server/health").json()["terminal_address"]
        assert reference.request("POST", "/__contract__/claude-owner", {"address": address}).json() == {"ok": True}
        normal = reference.request("GET", "/__contract__/claude-auth?path=" + quote(str(account), safe=""))
        assert normal.status_code == 200 and normal.json()["loggedIn"] is True
        alias = str(account / ".." / ".claude-work")
        reply = reference.request("GET", "/__contract__/claude-auth?path=" + quote(alias, safe=""))
        assert reply.status_code == 200, "o consumidor Python deve resolver a conta pela identidade canônica"
        assert reply.json()["loggedIn"] is True
        assert reply.json()["email"] == "fixture@example.test"
        assert server.request("POST", "/__hangar_server/accounts/claude", {"action": "auth", "path": alias}).status_code == 404
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()


def test_step_does_not_conclude_when_credential_changes_during_identity_probe(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    import json
    import os
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    credential = account / ".credentials.json"
    path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
    try:
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-initial"}}), encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-before-probe"}}), encoding="utf-8")
        source = """const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;
const marker=p.join(d,'probe-raced');
if(!fs.existsSync(marker)){
  fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-after-probe'}}));
  fs.writeFileSync(marker,'1');
  process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));
}else{process.stdout.write(JSON.stringify({loggedIn:false}));}
"""
        fixture = reference.root / "claude-native"
        (fixture / "node_modules/@anthropic-ai/claude-code/cli.js").write_text(source, encoding="utf-8")
        if os.name != "nt":
            (fixture / "claude").write_text("#!/usr/bin/env node\n" + source, encoding="utf-8")
        response = server.request("GET", path + "/passo")
        assert response.status_code == 200
        assert (account / "probe-raced").exists()
        assert json.loads(credential.read_text())["claudeAiOauth"]["accessToken"] == "synthetic-after-probe"
        assert response.json()["etapa"] == "aguardando", "identidade da credencial anterior não comprova autenticação do token atual"
        assert len(reference.request("GET", "/__contract__/claude-windows").json()["windows"]) == 1
        assert not (account / ".claude.json").exists()
    finally:
        server.close()
        reference.close()


def test_confirmation_does_not_accept_identity_read_before_token_replacement(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    from concurrent.futures import ThreadPoolExecutor
    import json
    import os
    import time
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    credential = account / ".credentials.json"
    path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
    pool = ThreadPoolExecutor()
    try:
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        source = """const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;
const marker=p.join(d,'probe-count');
const count=fs.existsSync(marker)?Number(fs.readFileSync(marker,'utf8'))+1:1;
if(count===1){
  fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-new'}}));
  process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));
}else{process.stdout.write(JSON.stringify({loggedIn:false}));}
fs.writeFileSync(marker,String(count));
"""
        fixture = reference.root / "claude-native"
        (fixture / "node_modules/@anthropic-ai/claude-code/cli.js").write_text(source, encoding="utf-8")
        if os.name != "nt":
            (fixture / "claude").write_text("#!/usr/bin/env node\n" + source, encoding="utf-8")
        confirming = pool.submit(server.request, "POST", path + "/codigo", {"codigo": "synthetic-code"})
        marker = account / "probe-count"
        deadline = time.monotonic() + 15
        while not confirming.done():
            if marker.exists() and marker.read_text() not in {"", "1"}:
                break
            assert time.monotonic() < deadline, "a releitura nativa não avançou"
        assert marker.exists()
        assert server.request("POST", path + "/cancelar").status_code == 200
        response = confirming.result(timeout=20)
        assert response.status_code == 409, "confirmação com identidade anterior deve permanecer pendente até cancelar"
        assert not (account / ".claude.json").exists()
    finally:
        server.close()
        reference.close()
        pool.shutdown(wait=True)
