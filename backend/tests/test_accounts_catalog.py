"""Leitores Python de contas: conta pendente fora da lista e ambiente igual ao golden do Rust."""
from __future__ import annotations

import json
from pathlib import Path

FIXTURES = Path(__file__).parent / "fixtures" / "accounts_contract"


def test_python_readers_do_not_select_pending_accounts(tmp_path, monkeypatch):
    from app import contas, codex_contas, config
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    for provider, marker in [("claude", ".hangar-conta"), ("codex", ".hangar-codex-conta")]:
        target = tmp_path / f".{provider}-partial"
        target.mkdir()
        (target / marker).write_text(json.dumps({"version": 1, "id": "partial"}), encoding="utf-8")
        (target / ".hangar-account-pending").touch()
        (target / ".credentials.json").write_text("{}", encoding="utf-8")
        (target / "projects").mkdir()
    assert contas.listar() == []
    assert [account.id for account in codex_contas.list_accounts()] == ["default"]
    assert not config._is_config_dir(tmp_path / ".claude-partial")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", "Parcial:" + str(tmp_path / ".claude-partial"))
    assert config.list_config_dirs() == []


def test_environment_matches_shared_golden(tmp_path, monkeypatch):
    from app.codex_contas import Account, environment
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    fixture = json.loads((FIXTURES / "environment.json").read_text(encoding="utf-8"))
    def materialize(name):
        return {key: str(tmp_path / value.removeprefix("<HOME>").lstrip("/"))
                if value.startswith("<HOME>") else value for key, value in fixture[name].items()}
    for account_id, is_default, golden in [("default", True, "default"), ("extra", False, "additional")]:
        home = tmp_path / ("captured" if is_default else ".codex-extra")
        assert environment(Account(account_id, home, is_default), base=materialize("base")) == materialize(golden)
