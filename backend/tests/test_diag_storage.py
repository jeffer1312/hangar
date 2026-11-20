"""Centralização, compatibilidade e separação entre diário e logs privados."""
from types import SimpleNamespace

import pytest

from app import log_paths

BASE = log_paths.base


@pytest.fixture(autouse=True)
def logs(tmp_path, monkeypatch):
    monkeypatch.setattr(log_paths, "base", lambda: tmp_path / "logs")
    return tmp_path / "logs"


def test_log_directory_is_independent_of_provider_account(tmp_path, monkeypatch):
    monkeypatch.setattr(log_paths, "os", SimpleNamespace(name="nt", environ={"LOCALAPPDATA": str(tmp_path)}))
    assert BASE() == tmp_path / "hangar" / "logs"
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "outra-conta"))
    assert BASE() == tmp_path / "hangar" / "logs"
    monkeypatch.setattr(log_paths, "os", SimpleNamespace(name="posix", environ={}))
    monkeypatch.setattr(log_paths.Path, "home", lambda: tmp_path)
    assert BASE() == tmp_path / ".hangar" / "logs"
