"""Centralização, compatibilidade e separação entre diário e logs privados."""
import asyncio
import json
import logging
import os
import subprocess
import sys
from datetime import date
from pathlib import Path
from types import SimpleNamespace

import pytest

from app import diag, log_paths

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


def test_operation_carries_safe_context_across_sync_and_async_steps():
    @diag.rastrear("teste.operacao")
    async def fail(secret):
        diag.registrar("teste.etapa", etapa="persistir")
        try:
            original = PermissionError(13, secret)
            original.winerror = 5
            raise original
        except OSError as exc:
            raise RuntimeError(secret) from exc

    with pytest.raises(RuntimeError):
        asyncio.run(fail("segredo"))
    events = [r for r in rows() if r["evento"].startswith("teste.")]
    assert len({r["operacao"] for r in events}) == 1
    assert events[-1]["codigo"] == "excecao" and events[-1]["winerror"] == 5
    assert events[-1]["causa_tipo"] == "PermissionError"
    assert "segredo" not in diag.ler_tudo()
