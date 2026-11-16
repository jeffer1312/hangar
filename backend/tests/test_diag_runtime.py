"""O arquivo exportado explica a falha sem carregar conteúdo do terminal."""
import asyncio
import json
import subprocess
from types import SimpleNamespace

import pytest

from app import diag

VERSION_PROBE = diag._versao_mux


@pytest.fixture(autouse=True)
def isolated_diary(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    monkeypatch.setattr(diag, "_versao_mux", lambda: "psmux 3.3.7", raising=False)


def events():
    return [json.loads(line) for line in diag.ler_tudo().splitlines()]


def test_export_identifies_current_environment():
    header = events()[0]
    assert header["mux"] == "psmux 3.3.7"
    assert header["python"] and header["sistema"] and header["arquitetura"]
    assert header["inicio_backend"] and header["pid_backend"] > 0




@pytest.mark.parametrize("output,code,expected", [
    ("psmux 3.3.7\n", 0, "psmux 3.3.7"),
    ("tmux 3.6a\n", 0, "tmux 3.6a"),
    ("segredo\npsmux 3.3.7", 0, "resposta_nao_reconhecida"),
    ("segredo", 1, "retorno:1"),
])
def test_version_probe_never_exports_arbitrary_output(monkeypatch, output, code, expected):
    monkeypatch.setattr(diag.subprocess, "run", lambda *a, **kw: subprocess.CompletedProcess(a, code, output, "segredo"))
    assert VERSION_PROBE() == expected


def test_windows_memory_snapshot_and_failure(monkeypatch):
    import psutil
    monkeypatch.setattr(diag.sys, "platform", "win32")
    monkeypatch.setattr(psutil, "virtual_memory", lambda: SimpleNamespace(total=8 * 1024**3, available=1024**3))
    assert diag.recursos()["memoria_total_mb"] == 8192
    assert diag.recursos()["memoria_disponivel_mb"] == 1024

    def denied():
        raise psutil.AccessDenied()

    monkeypatch.setattr(psutil, "virtual_memory", denied)
    assert diag.recursos()["recursos_erro"] == "AccessDenied"


