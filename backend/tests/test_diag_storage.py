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
    monkeypatch.setattr(diag, "_versao_mux", lambda: "tmux teste")
    return tmp_path / "logs"


def rows():
    return [json.loads(line) for line in diag.ler_tudo().splitlines()]


def test_log_directory_is_independent_of_provider_account(tmp_path, monkeypatch):
    monkeypatch.setattr(log_paths, "os", SimpleNamespace(name="nt", environ={"LOCALAPPDATA": str(tmp_path)}))
    assert BASE() == tmp_path / "hangar" / "logs"
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "outra-conta"))
    assert BASE() == tmp_path / "hangar" / "logs"
    monkeypatch.setattr(log_paths, "os", SimpleNamespace(name="posix", environ={}))
    monkeypatch.setattr(log_paths.Path, "home", lambda: tmp_path)
    assert BASE() == tmp_path / ".hangar" / "logs"


def test_legacy_sources_are_preserved_and_copied_without_merging(tmp_path, monkeypatch):
    name = f"uso-{date.today().isoformat()}.jsonl"
    sources = [tmp_path / "conta-a", tmp_path / "conta-b"]
    for index, source in enumerate(sources):
        source.mkdir()
        (source / name).write_text(json.dumps({"evento": f"legado-{index}"}) + "\n")
    monkeypatch.setattr(diag, "_pastas_legadas", lambda: sources)
    diag.registrar("atual")
    assert {r["evento"] for r in rows()} >= {"atual", "legado-0", "legado-1"}
    assert len(diag.arquivos()) == 3
    assert all((source / name).exists() for source in sources)
    assert len([r for r in rows() if r["evento"].startswith("legado-")]) == 2
    with (sources[0] / name).open("a") as f:
        f.write('{"evento":"escritor-antigo-ainda-vivo"}\n')
    assert any(r["evento"] == "escritor-antigo-ainda-vivo" for r in rows())


def test_failed_copy_keeps_previous_snapshot_and_original(tmp_path, monkeypatch):
    source = tmp_path / "antigo"
    source.mkdir()
    file = source / f"uso-{date.today().isoformat()}.jsonl"
    file.write_text('{"evento":"anterior"}\n')
    monkeypatch.setattr(diag, "_pastas_legadas", lambda: [source])
    diag.migrar_legados()
    file.write_text('{"evento":"original-preservado"}\n')

    def fail(*args):
        raise PermissionError(13, "segredo")

    monkeypatch.setattr(diag.atomico, "substituir", fail)
    exported = rows()
    assert any(r["evento"] == "anterior" for r in exported)
    assert any(r["evento"] == "diag.migracao" and r["errno"] == 13 for r in exported)
    assert "original-preservado" in file.read_text()
    assert "segredo" not in json.dumps(exported)


def test_legacy_private_log_never_enters_export(tmp_path, logs, monkeypatch):
    source = tmp_path / "antigo"
    source.mkdir()
    (source / "install.log").write_text("segredo")
    monkeypatch.setattr(diag, "_logs_legados", lambda: {source})
    diag.migrar_legados(privados=True)
    copied = list((logs / "privado").rglob("install.log"))
    assert len(copied) == 1 and copied[0].read_text() == "segredo"
    assert "segredo" not in diag.ler_tudo()


def test_diary_symlink_cannot_export_another_file(tmp_path, monkeypatch):
    source = tmp_path / "antigo"
    source.mkdir()
    secret = tmp_path / "credencial"
    secret.write_text("segredo")
    try:
        (source / f"uso-{date.today().isoformat()}.jsonl").symlink_to(secret)
    except OSError:
        pytest.skip("o Windows deste teste não permite criar symlink")
    monkeypatch.setattr(diag, "_pastas_legadas", lambda: [source])
    exported = rows()
    assert any(r.get("codigo") == "link_ignorado" for r in exported)
    assert "segredo" not in json.dumps(exported)


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
