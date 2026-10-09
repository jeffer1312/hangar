"""Cadastro e resolução isolados das contas Codex."""

from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import stat

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


def test_delete_keeps_rollouts_in_the_default_home(isolated_home):
    work = accounts.create_account("work")
    rollouts = ["sessions/2026/10/09/rollout-1.jsonl", "archived_sessions/rollout-0.jsonl"]
    for name in rollouts:
        (work.home / name).parent.mkdir(parents=True)
        (work.home / name).write_text(name)
    (work.home / "auth.json").write_text("segredo")

    assert accounts.delete_account(work) == {"merged": 2, "skipped": 0, "renamed": 0}

    assert not work.home.exists()
    for name in rollouts:
        assert (isolated_home / ".codex" / name).read_text() == name
    assert not (isolated_home / ".codex" / "auth.json").exists()


def test_delete_without_keep_copies_nothing(isolated_home):
    work = accounts.create_account("work")
    (work.home / "sessions").mkdir()
    (work.home / "sessions" / "rollout-1.jsonl").write_text("x")

    assert accounts.delete_account(work, keep_transcripts=False) is None

    assert not work.home.exists()
    assert not (isolated_home / ".codex").exists()


def test_delete_removes_read_only_git_pack(isolated_home):
    # O Codex clona marketplaces em .tmp/ e o git grava os packs somente-leitura.
    work = accounts.create_account("work")
    pack = work.home / ".tmp" / "marketplaces" / "clone" / ".git" / "objects" / "pack" / "pack-1.idx"
    pack.parent.mkdir(parents=True)
    pack.write_bytes(b"x")
    pack.chmod(stat.S_IREAD)

    accounts.delete_account(work)

    assert not work.home.exists()


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
