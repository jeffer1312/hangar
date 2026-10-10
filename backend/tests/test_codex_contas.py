"""Resolução isolada das contas Codex (o cadastro é do Rust)."""

import json
from pathlib import Path

import pytest

from app import codex_contas as accounts
import codex_contas_apoio


@pytest.fixture
def isolated_home(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    return tmp_path


def test_account_creation_is_isolated(isolated_home):
    account = codex_contas_apoio.create_account("work")

    assert account == accounts.Account("work", isolated_home / ".codex-work", False)
    assert account.home == isolated_home / ".codex-work"
    assert accounts.resolve_account("work") == account
    assert not (account.home / "auth.json").exists()
    assert not (isolated_home / ".codex").exists()
    assert json.loads((account.home / accounts.MARKER).read_text()) == {
        "version": 1,
        "id": "work",
    }
    # Nasce pronta pro login: credencial em arquivo, dentro da pasta da conta.
    assert (account.home / "config.toml").read_text() == 'cli_auth_credentials_store = "file"\n'
    assert [a.id for a in accounts.list_accounts()] == ["default", "work"]


@pytest.mark.parametrize("name", ["../other", "a/b", "a\n", "", "a" * 33])
def test_invalid_account_names_are_rejected(isolated_home, name):
    with pytest.raises(accounts.AccountError) as error:
        accounts.resolve_account(name)

    assert error.value.status == 400
    assert error.value.code == "codex_account_invalid_name"
    assert not list(isolated_home.glob(".codex-*"))


@pytest.mark.parametrize("marker", [
    "{}",
    json.dumps({"version": 2, "id": "work"}),
    json.dumps({"version": 1, "id": "other"}),
])
def test_resolve_rejects_incoherent_marker(isolated_home, marker):
    target = isolated_home / ".codex-work"
    target.mkdir()
    (target / accounts.MARKER).write_text(marker)

    with pytest.raises(accounts.AccountError):
        accounts.resolve_account("work")

    assert accounts.list_accounts() == [accounts.Account("default", isolated_home / ".codex", True)]


def test_resolve_rejects_symlink_account(isolated_home):
    outside = isolated_home / "outside"
    outside.mkdir()
    (outside / accounts.MARKER).write_text(json.dumps({"version": 1, "id": "work"}))
    (isolated_home / ".codex-work").symlink_to(outside, target_is_directory=True)

    with pytest.raises(accounts.AccountError):
        accounts.resolve_account("work")

    assert [a.id for a in accounts.list_accounts()] == ["default"]


def test_account_for_rollout_uses_only_registered_canonical_roots(isolated_home):
    work = codex_contas_apoio.create_account("work")
    live = work.home / "sessions" / "2026" / "09" / "09" / "rollout.jsonl"
    archived = work.home / "archived_sessions" / "rollout-old.jsonl"
    live.parent.mkdir(parents=True)
    archived.parent.mkdir()

    assert accounts.account_for_rollout(live) == work
    assert accounts.account_for_rollout(archived) == work
    assert accounts.account_for_rollout(isolated_home / ".codex-unknown" / "sessions" / "x.jsonl") is None


def test_account_for_rollout_rejects_ambiguous_canonical_roots(isolated_home, monkeypatch):
    work = codex_contas_apoio.create_account("work")
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", work.home)
    rollout = work.home / "sessions" / "rollout.jsonl"

    with pytest.raises(accounts.AccountError) as error:
        accounts.account_for_rollout(rollout)

    assert error.value.status == 409
    assert error.value.code == "codex_account_ambiguous_rollout"
    assert error.value.params["accounts"] == ["default", "work"]


def test_default_home_is_always_listed_without_creating_it(isolated_home):
    assert accounts.default_home() == isolated_home / ".codex"
    assert accounts.list_accounts() == [accounts.Account("default", isolated_home / ".codex", True)]
    assert not (isolated_home / ".codex").exists()


def test_padrao_fantasma_nao_aparece_na_tela(isolated_home, monkeypatch):
    # Maquina sem Codex e sem ~/.codex: a padrao era um cartao inventado, "precisa entrar" numa
    # conta que nunca existiu (medido 12/09/2026). A lista completa continua com ela, pra resolver id.
    monkeypatch.setattr(accounts.shutil, "which", lambda nome: None)
    assert accounts.list_visible_accounts() == []
    assert [a.id for a in accounts.list_accounts()] == ["default"]


def test_padrao_aparece_com_codex_instalado_ou_com_a_pasta(isolated_home, monkeypatch):
    monkeypatch.setattr(accounts.shutil, "which", lambda nome: r"C:\bin\codex.exe")
    assert [a.id for a in accounts.list_visible_accounts()] == ["default"]
    monkeypatch.setattr(accounts.shutil, "which", lambda nome: None)
    (isolated_home / ".codex").mkdir()
    assert [a.id for a in accounts.list_visible_accounts()] == ["default"]


def test_rollout_outside_registered_accounts_is_not_adopted(isolated_home):
    external = isolated_home / "external" / "sessions" / "rollout.jsonl"
    external.parent.mkdir(parents=True)
    external.write_text("private")

    assert accounts.account_for_rollout(external) is None
    assert external.read_text() == "private"
