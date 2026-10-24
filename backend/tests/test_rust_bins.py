# backend/tests/test_rust_bins.py
"""Busca dos binários Rust: a variável vence e não cai para outro lugar; sem ela, o mais novo
entre o build do checkout e o baixado (empate fica com o checkout); arquivo que não executa não conta."""
import os
import stat
from pathlib import Path

import pytest

from app import rust_bins

EXE = "hangar-cano" + (".exe" if os.name == "nt" else "")


def _executavel(p: Path) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("#!/bin/sh\n", encoding="utf-8")
    p.chmod(p.stat().st_mode | stat.S_IXUSR)
    return p


@pytest.fixture
def locais(tmp_path, monkeypatch):
    repo, home = tmp_path / "repo", tmp_path / "home"
    monkeypatch.setattr(rust_bins, "_REPO", repo)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))
    monkeypatch.delenv("CP_RUST_CANO_BIN", raising=False)
    return repo / "crates" / "target" / "release" / EXE, home / ".hangar" / "bin" / EXE


def test_sem_nada_devolve_none(locais):
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None


def test_o_mais_novo_vence_e_empate_fica_com_o_checkout(locais):
    build, baixado = locais
    _executavel(baixado)
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == baixado
    _executavel(build)
    os.utime(build, (1_000, 1_000))
    os.utime(baixado, (1_000, 1_000))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == build
    os.utime(baixado, (2_000, 2_000))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == baixado
    os.utime(build, (3_000, 3_000))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == build


def test_variavel_vence(locais, tmp_path, monkeypatch):
    build, _ = locais
    _executavel(build)
    escolhido = _executavel(tmp_path / "outro" / EXE)
    monkeypatch.setenv("CP_RUST_CANO_BIN", str(escolhido))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == escolhido


def test_variavel_com_caminho_errado_nao_cai_para_outro_binario(locais, tmp_path, monkeypatch, caplog):
    build, _ = locais
    _executavel(build)
    monkeypatch.setenv("CP_RUST_CANO_BIN", str(tmp_path / "nao-existe"))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None
    assert "CP_RUST_CANO_BIN" in caplog.text


@pytest.mark.skipif(os.name == "nt", reason="no Windows todo arquivo conta como executável")
def test_arquivo_sem_permissao_de_execucao_nao_conta(locais):
    build, _ = locais
    build.parent.mkdir(parents=True)
    build.write_text("x", encoding="utf-8")
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None


@pytest.mark.skipif(os.name == "nt", reason="symlink de pasta no Windows pede privilégio; a junction é coberta na VM")
def test_pacote_lanca_pelo_caminho_real_da_versao(locais):
    """Processo longo roda do caminho real: a troca do `current` não o tira de baixo."""
    _, baixado = locais
    home = baixado.parents[2]
    versao = home / ".hangar" / "versions" / "0.1.0.5"
    _executavel(versao / "bin" / EXE)
    (home / ".hangar" / "current").symlink_to(versao, target_is_directory=True)
    achado = rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN")
    assert achado == versao.resolve() / "bin" / EXE
    assert "current" not in achado.parts


@pytest.mark.skipif(os.name == "nt", reason="symlink de pasta no Windows pede privilégio; a junction é coberta na VM")
def test_current_no_meio_da_troca_espera_o_link_novo(locais):
    """No Windows a troca tira o `current` do lugar por um instante (fica o `current.old`)."""
    import threading
    import time
    _, baixado = locais
    hangar = baixado.parents[1]
    velha, nova = hangar / "versions" / "0.1.0.4", hangar / "versions" / "0.1.0.5"
    _executavel(velha / "bin" / EXE)
    _executavel(nova / "bin" / EXE)
    (hangar / "current.old").symlink_to(velha, target_is_directory=True)
    threading.Timer(0.2, lambda: (hangar / "current").symlink_to(nova, target_is_directory=True)).start()
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == nova.resolve() / "bin" / EXE


def test_sem_troca_em_andamento_nao_espera(locais):
    import time
    inicio = time.monotonic()
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None
    assert time.monotonic() - inicio < 0.5
