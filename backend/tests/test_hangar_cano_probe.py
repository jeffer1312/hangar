"""Sonda do hangar-cano: binário que não roda nesta máquina volta ao cano.py com uma linha no
diário, e a sonda roda uma vez só por processo."""
import os
from pathlib import Path

import pytest

from app.adapters.claude_headless import adapter as A

pytestmark = pytest.mark.skipif(os.name == "nt", reason="binário falso é script sh")


@pytest.fixture
def fake_cano(tmp_path, monkeypatch):
    diario: list[tuple] = []
    monkeypatch.setattr(A.diag, "registrar", lambda evento, nivel="ok", **kw: diario.append((evento, kw)))
    A._probe_cano_bin.cache_clear()
    contador = tmp_path / "rodou"

    def make(corpo: str) -> Path:
        exe = tmp_path / "hangar-cano"
        exe.write_text(corpo.replace("@CONTA@", f"echo x >> {contador}"), encoding="utf-8")
        exe.chmod(0o755)
        monkeypatch.setattr(A.rust_bins, "find_bin", lambda name, env_var: exe)
        return exe

    def runs() -> int:
        return len(contador.read_text().splitlines()) if contador.exists() else 0
    yield make, diario, runs
    A._probe_cano_bin.cache_clear()


def test_binario_que_roda_e_usado_e_sondado_uma_vez(fake_cano):
    make, diario, runs = fake_cano
    exe = make("#!/bin/sh\n@CONTA@\nexit 2\n")
    assert A._usable_cano_bin() == exe
    assert A._usable_cano_bin() == exe
    assert runs() == 1 and diario == []


def test_retorno_errado_volta_ao_cano_py(fake_cano):
    make, diario, runs = fake_cano
    make("#!/bin/sh\n@CONTA@\nexit 127\n")
    assert A._usable_cano_bin() is None
    assert A._usable_cano_bin() is None
    assert runs() == 1
    assert diario == [("hangar_cano.indisponivel", {"codigo": "retorno", "retorno": 127})]


def test_binario_que_nem_executa_volta_ao_cano_py(fake_cano):
    make, diario, _ = fake_cano
    make("\x7fELF lixo que o kernel recusa")
    assert A._usable_cano_bin() is None
    assert A._usable_cano_bin() is None
    assert len(diario) == 1 and diario[0][0] == "hangar_cano.indisponivel"
    assert diario[0][1]["codigo"] == "exec"


def test_sem_binario_nao_sonda_nem_registra(fake_cano, monkeypatch):
    _, diario, _ = fake_cano
    monkeypatch.setattr(A.rust_bins, "find_bin", lambda name, env_var: None)
    assert A._usable_cano_bin() is None and diario == []
