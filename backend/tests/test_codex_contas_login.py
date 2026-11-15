"""Reserva por conta Codex na criação de sessão e ambiente da conta.

Login, logout, preparo e leitura de autenticação são do Rust.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from app import codex_contas as accounts
import codex_contas_apoio


@pytest.fixture
def contas(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    accounts.default_home().mkdir()
    work = codex_contas_apoio.create_account("work")
    return accounts.Account("default", accounts.default_home(), True), work


def test_ambiente_da_secundaria_remove_identidade_externa_e_padrao_preserva(contas):
    default, work = contas
    base = {"OPENAI_API_KEY": "secret", "OPENAI_BASE_URL": "https://external.invalid",
            "TOOL_ENDPOINT": "keep", "PATH": "/bin"}

    secondary = accounts.environment(work, home=Path("/tmp/hangar-home"), base=base)
    primary = accounts.environment(default, home=Path("/tmp/hangar-home"), base=base)

    assert "OPENAI_API_KEY" not in secondary
    assert "OPENAI_BASE_URL" not in secondary
    assert secondary["TOOL_ENDPOINT"] == "keep"
    assert primary["OPENAI_API_KEY"] == "secret"
    assert secondary["CODEX_HOME"] == str(work.home)
    assert "CODEX_CONFIG_HOME" not in secondary
    assert "CODEX_SQLITE_HOME" not in secondary
