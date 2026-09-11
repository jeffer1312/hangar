"""Cadastro e resolução isolados das contas Codex."""

from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path

import pytest

from app import codex_contas as accounts


@pytest.fixture
def isolated_home(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    return tmp_path


def test_account_creation_is_isolated(isolated_home):
    account = accounts.create_account("work")

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


def test_delete_removes_managed_account_only(isolated_home):
    work = accounts.create_account("work")
    (work.home / "auth.json").write_text("x")
    alheia = isolated_home / ".codex-alheia"
    alheia.mkdir()

    accounts.delete_account(work)
    assert not work.home.exists()

    with pytest.raises(accounts.AccountError) as padrao:
        accounts.delete_account(accounts.resolve_account("default"))
    assert padrao.value.code == "codex_account_default_protected"

    with pytest.raises(accounts.AccountError) as sem_marcador:
        accounts.delete_account(accounts.Account("alheia", alheia, False))
    assert sem_marcador.value.code == "codex_account_invalid_marker"
    assert alheia.exists()


@pytest.mark.parametrize("name", ["default", "../other", "a/b", "a\n", "", "a" * 33])
def test_invalid_account_names_are_rejected(isolated_home, name):
    with pytest.raises(accounts.AccountError) as error:
        accounts.create_account(name)

    assert error.value.status == 400
    assert error.value.code == "codex_account_invalid_name"
    assert not list(isolated_home.glob(".codex-*"))


def test_existing_unmanaged_directory_is_untouched(isolated_home):
    target = isolated_home / ".codex-work"
    target.mkdir()
    payload = target / "sentinel"
    payload.write_bytes(b"keep")

    with pytest.raises(accounts.AccountError):
        accounts.create_account("work")

    assert payload.read_bytes() == b"keep"
    assert not (target / accounts.MARKER).exists()


def test_existing_marker_symlink_is_untouched(isolated_home):
    target = isolated_home / ".codex-work"
    target.mkdir()
    outside = isolated_home / "outside-marker"
    outside.write_bytes(b"outside")
    marker = target / accounts.MARKER
    marker.symlink_to(outside)

    with pytest.raises(accounts.AccountError):
        accounts.create_account("work")

    assert marker.is_symlink()
    assert outside.read_bytes() == b"outside"


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


def test_concurrent_creation_has_one_owner_and_keeps_its_directory(isolated_home):
    def create():
        try:
            return accounts.create_account("work")
        except accounts.AccountError as error:
            return error

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda _: create(), range(2)))

    assert sum(isinstance(result, accounts.Account) for result in results) == 1
    errors = [result for result in results if isinstance(result, accounts.AccountError)]
    assert len(errors) == 1
    assert errors[0].status == 409
    assert errors[0].code == "codex_account_exists"
    account = accounts.resolve_account("work")
    assert account.home.is_dir()
    assert (account.home / accounts.MARKER).is_file()


def test_account_for_rollout_uses_only_registered_canonical_roots(isolated_home):
    work = accounts.create_account("work")
    live = work.home / "sessions" / "2026" / "09" / "09" / "rollout.jsonl"
    archived = work.home / "archived_sessions" / "rollout-old.jsonl"
    live.parent.mkdir(parents=True)
    archived.parent.mkdir()

    assert accounts.account_for_rollout(live) == work
    assert accounts.account_for_rollout(archived) == work
    assert accounts.account_for_rollout(isolated_home / ".codex-unknown" / "sessions" / "x.jsonl") is None


def test_account_for_rollout_rejects_ambiguous_canonical_roots(isolated_home, monkeypatch):
    work = accounts.create_account("work")
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", work.home)
    rollout = work.home / "sessions" / "rollout.jsonl"

    with pytest.raises(accounts.AccountError) as error:
        accounts.account_for_rollout(rollout)

    assert error.value.status == 409
    assert error.value.code == "codex_account_ambiguous_rollout"
    assert error.value.params["accounts"] == ["default", "work"]


def test_marker_write_failure_removes_only_new_empty_directory(isolated_home, monkeypatch):
    target = isolated_home / ".codex-work"
    original_write = Path.write_text

    def fail_marker(self, *args, **kwargs):
        if self.name == accounts.MARKER:
            raise OSError("marker unavailable")
        return original_write(self, *args, **kwargs)

    monkeypatch.setattr(Path, "write_text", fail_marker)

    with pytest.raises(accounts.AccountError) as error:
        accounts.create_account("work")

    assert error.value.status == 500
    assert not target.exists()
    assert not list(isolated_home.glob(".codex-*"))


def test_default_home_is_always_listed_without_creating_it(isolated_home):
    assert accounts.default_home() == isolated_home / ".codex"
    assert accounts.list_accounts() == [accounts.Account("default", isolated_home / ".codex", True)]
    assert not (isolated_home / ".codex").exists()


def test_rollout_outside_registered_accounts_is_not_adopted(isolated_home):
    external = isolated_home / "external" / "sessions" / "rollout.jsonl"
    external.parent.mkdir(parents=True)
    external.write_text("private")

    assert accounts.account_for_rollout(external) is None
    assert external.read_text() == "private"
