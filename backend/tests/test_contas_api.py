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


def test_conta_sem_credencial_ainda_aparece_na_lista(casa, hangar_server_bin):
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


def test_codex_com_config_dir_nao_reconcilia(casa, monkeypatch, hangar_server_bin):
    """A abertura é do Rust: no modo python o pedido é recusado com código, e a reconciliação
    (efeito no disco) não roda antes da recusa."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas, "reconciliar", lambda nome, projeto=None: reconciliou.append(1) or [])
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "codex"}, headers=AUTH)
    assert r.status_code == 503 and "session_open_needs_rust_server" in r.text
    assert reconciliou == []
