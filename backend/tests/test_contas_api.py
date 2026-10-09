"""Contas Claude do lado Python: o catálogo e a criação de sessão numa conta.

Cadastro, exclusão e saída de conta são do Rust (ver `test_accounts_need_rust.py`). Aqui fica o
que o Python ainda faz: conta recém-criada aparece no catálogo ANTES do /login (senão não há onde
abrir a sessão que roda o /login), e a criação de sessão reconcilia a conta sob a trava do ciclo.
"""
import json

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import contas
from app.api import app
from app.config import list_config_dirs, settings
from app.models import SessionInfo

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


def test_pasta_conta_por_symlink_nao_entra_na_lista(casa):
    """~/.claude-evil -> /tmp/fora com marcador do lado de lá não é conta: a reconciliação e o
    apagar remexeriam — e destruiriam — um diretório externo."""
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
    assert str(casa / ".claude-evil") not in {c.path for c in list_config_dirs()}


def test_marcador_por_symlink_nao_entra_na_lista(casa):
    conta = casa / ".claude-conta2"
    conta.mkdir()
    alvo = casa / "marcador-fora"
    alvo.write_text("", encoding="utf-8")
    (conta / contas.MARCADOR).symlink_to(alvo)
    assert str(conta) not in {c.path for c in list_config_dirs()}


def test_falha_na_reconciliacao_devolve_erro_e_nao_cria_sessao(casa, monkeypatch):
    """ContaError do reconciliar (ex: Windows sem Modo Desenvolvedor) e OSError de filesystem
    viram HTTPException com o motivo, e a sessão NÃO é criada — abertura abortada, não 500 com
    traceback."""
    contas.criar("conta2")

    def boom_conta(self, projeto=None):
        raise contas.ContaError(500, "não consegui criar o atalho")

    def boom_os(self, projeto=None):
        raise OSError("permissão negada")

    criados = []
    monkeypatch.setattr(api_mod.registry, "create",
                        lambda *a, **k: criados.append(a) or object())
    cli = TestClient(app)
    # ContaError: o detail passa como STRING (e.detail) — o texto do motivo chega direto.
    monkeypatch.setattr(contas._Ciclo, "reconciliar", boom_conta)
    r = cli.post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude"}, headers=AUTH)
    assert r.status_code == 500
    assert "não consegui criar o atalho" in r.json()["detail"]
    # OSError: vira o envelope erro_conta_reconciliacao_falhou — o contrato code/params tem que
    # ser afirmado (B4 do parecer task 11): um envelope com o MESMO msg mas code errado reprova.
    monkeypatch.setattr(contas._Ciclo, "reconciliar", boom_os)
    r = cli.post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude"}, headers=AUTH)
    assert r.status_code == 500
    d = r.json()["detail"]
    assert d["code"] == "erro_conta_reconciliacao_falhou"
    assert d["params"]["nome_conta"] == "conta2"
    assert d["params"]["erro"] == "permissão negada"
    assert "permissão negada" in d["msg"]
    assert criados == []


def test_conta_nomeada_repassa_as_mesmas_escolhas_da_padrao(casa, monkeypatch):
    contas.criar("conta2")
    monkeypatch.setattr(contas._Ciclo, "reconciliar", lambda self, projeto=None: [])
    monkeypatch.setattr(api_mod.cliproxy, "supports_fast", lambda engine, model=None: True)
    kwargs = []
    monkeypatch.setattr(api_mod.registry, "create",
                        lambda *a, **k: kwargs.append(k) or SessionInfo(name="s1", provider="claude"))
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude", "service_tier": "priority", "initial_prompt": "oi"}, headers=AUTH)
    assert r.status_code == 200, r.text
    assert kwargs[0]["service_tier"] == "priority"
    assert kwargs[0]["initial_prompt"] == "oi"


def test_codex_com_config_dir_nao_reconcilia(casa, monkeypatch):
    """Provider codex não consome config dir (ele tem conta própria, do CLI do Codex): a
    reconciliação — efeito no disco — não pode rodar num pedido que vai criar uma sessão codex."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas._Ciclo, "reconciliar",
                        lambda self, projeto=None: reconciliou.append(1) or [])
    monkeypatch.setattr(api_mod.registry, "create",
                        lambda *a, **k: SessionInfo(name="s1", provider="codex"))
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "codex"}, headers=AUTH)
    assert reconciliou == []


def test_engine_invalido_rejeita_antes_de_reconciliar(casa, monkeypatch):
    """Validação de engine vem ANTES do toque no disco: pedido que vai ser rejeitado não pode
    ter movido deriva nem criado memória na conta."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas._Ciclo, "reconciliar",
                        lambda self, projeto=None: reconciliou.append(1) or [])
    monkeypatch.setattr(api_mod.engines, "listar", lambda: {})
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude", "engine": "naoexiste"}, headers=AUTH)
    assert r.status_code == 400
    assert reconciliou == []


def test_conta_de_quem_pede_sessao_nova_recusa_pane_sem_processo(monkeypatch):
    """Pane sem processo: para o DELETE libera ("ninguém usando"), para CRIAR recusa.

    Confiar aqui criaria a sessão nova na conta padrão sem ninguém escolher — a cobrança errada e
    calada que a tool `new_session` existe pra não repetir."""
    from app import tmux

    monkeypatch.setattr(api_mod.headless_sessions, "exists", lambda name: False)
    monkeypatch.setattr(tmux, "pane_pid", lambda name: None)
    assert api_mod._session_config_dir_strict("morta") == (None, True)
    assert api_mod._caller_config_dir("morta") == (None, False)

    monkeypatch.setattr(tmux, "pane_pid", lambda name: (_ for _ in ()).throw(RuntimeError("tmux fora")))
    assert api_mod._caller_config_dir("morta") == (None, False)
