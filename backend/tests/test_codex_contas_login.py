"""Reserva por conta Codex na criação de sessão, ambiente da conta e o cliente nativo.

Login, logout, preparo e leitura de autenticação são do Rust: sem ele a leitura fica
indisponível (ver `test_accounts_need_rust.py`).
"""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from app import codex_contas as accounts
from app.codex_contas_login import CodexContasLogin
from app.codex_importador import CodexNativo


@pytest.fixture
def contas(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    accounts.default_home().mkdir()
    work = accounts.create_account("work")
    return accounts.Account("default", accounts.default_home(), True), work


@pytest.fixture
def service():
    return CodexContasLogin()


async def test_criacao_em_andamento_recusa_outra_e_libera_depois(contas, service):
    _, work = contas
    lease = service.reserve_creation(work)
    with pytest.raises(accounts.AccountError) as error:
        service.reserve_creation(work)
    assert (error.value.status, error.value.code) == (409, "codex_account_creation_in_progress")
    lease.release()
    service.reserve_creation(work).release()


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


async def test_duas_criacoes_podem_usar_a_mesma_conta(contas, service):
    _, work = contas
    first = service.reserve_creation(work)
    first.mark_live("one")

    second = service.reserve_creation(work)

    second.release()
    first.release()


async def test_dispatch_considera_method_antes_de_id():
    native = CodexNativo(Path("/tmp"), Path("/tmp"))
    native._pending[7] = asyncio.get_running_loop().create_future()
    listener = native.subscribe("account/login/completed")

    native._dispatch({"id": 7, "method": "account/login/completed", "params": {"success": True}})

    assert not native._pending[7].done()
    assert (await listener.get())["method"] == "account/login/completed"
