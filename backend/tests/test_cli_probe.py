import os
import stat
import subprocess
import time

import pytest

import app.cli_probe as cli_probe

# Plataforma REAL, lida no import. `test_windows_path_com_drive_e_pathext` falsifica `os.name` pra
# exercitar o ramo Windows da sonda, e qualquer `os.name` consultado DEPOIS disso responde "nt" ate
# no Linux — foi assim que a criacao dos dubles passou a fazer `.cmd` la e quebrou. Quem decide
# COMO criar um executavel e a maquina de verdade, nunca o estado global que o proprio teste
# falsificou.
_NT_REAL = os.name == "nt"


def _make_script(tmp_path, name, content):
    p = tmp_path / name
    p.write_text(content)
    p.chmod(p.stat().st_mode | stat.S_IEXEC)
    return p


def _make_sh(tmp_path, name, exit_code=0, sleep=None):
    """Executavel de mentira que a sonda consiga RODAR nesta plataforma.

    A sonda executa o candidato de verdade (`[caminho, "--version"]`), entao o duble precisa
    ser executavel de verdade — nao basta existir. No POSIX isso e um `#!/bin/sh` com o bit de
    exec. No Windows nao ha bit de exec e um arquivo de TEXTO chamado `claude` (ou ate
    `claude.EXE`) devolve WinError 193 na hora de rodar: o que o sistema executa la e `.cmd`,
    e a sonda ja procura por PATHEXT. Por isso o nome ganha a extensao — sem ela o caso ficaria
    afirmando "nao instalado" e nao provaria nada da sonda.

    O `sleep` vira `ping` porque o `timeout` do Windows recusa stdin redirecionado (e a sonda
    roda com capture_output).
    """
    if _NT_REAL:
        corpo = f"@echo off\nping -n {int(sleep) + 1} 127.0.0.1 > nul\n" if sleep is not None else "@echo off\n"
        return _make_script(tmp_path, name + ".cmd", corpo + f"exit /b {exit_code}\n")
    if sleep is not None:
        content = f"#!/bin/sh\nsleep {sleep}\nexit {exit_code}\n"
    else:
        content = f"#!/bin/sh\nexit {exit_code}\n"
    return _make_script(tmp_path, name, content)


@pytest.fixture(autouse=True)
def _limpa_cache():
    # isola cada teste do cache de PATH e do cache de 60s
    cli_probe._path_cache = None
    cli_probe._cache = None  # type: ignore[attr-defined]
    cli_probe._cache_ts = 0  # type: ignore[attr-defined]
    # garante que o seam de PATH não vaze entre testes
    orig = getattr(cli_probe, "_path_login", None)
    yield
    cli_probe._path_cache = None
    cli_probe._cache = None  # type: ignore[attr-defined]
    cli_probe._cache_ts = 0  # type: ignore[attr-defined]
    # restaura o seam (se o teste monkeypatchou, o monkeypatch já desfaz, mas garante)
    if orig is None and hasattr(cli_probe, "_path_login"):
        # se o teste setou para string, volta para None (valor inicial que indica "usar shell")
        try:
            cli_probe._path_login = None  # type: ignore[attr-defined]
        except Exception:
            pass


def test_binario_ok_e_instalado(tmp_path, monkeypatch):
    _make_sh(tmp_path, "claude", exit_code=0)
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path))
    res = cli_probe.sondar_providers()
    assert res["claude"]["disponivel"] is True
    assert res["claude"]["motivo"] is None


def test_exit_nao_zero_ainda_e_instalado(tmp_path, monkeypatch):
    _make_sh(tmp_path, "codex", exit_code=3)
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path))
    res = cli_probe.sondar_providers()
    assert res["codex"]["disponivel"] is True
    assert res["codex"]["motivo"] is None


def test_timeout_e_instalado(tmp_path, monkeypatch):
    # script que dorme mais que o timeout de 2s da sonda
    _make_sh(tmp_path, "pi", exit_code=0, sleep=5)
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path))
    res = cli_probe.sondar_providers()
    assert res["pi"]["disponivel"] is True
    assert res["pi"]["motivo"] is None


def test_inexistente_nao_instalado(tmp_path, monkeypatch):
    # tmp_path vazio, sem binário nenhum
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path))
    res = cli_probe.sondar_providers()
    assert res["kimi"]["disponivel"] is False
    assert res["kimi"]["motivo"] == "nao_encontrado"


def test_sem_permissao_pula_pro_proximo_candidato(tmp_path, monkeypatch):
    # 2 dirs no PATH: o primeiro tem o binário sem permissão, o segundo tem um bom
    dir1 = tmp_path / "d1"
    dir2 = tmp_path / "d2"
    dir1.mkdir()
    dir2.mkdir()
    # d1/claude existe mas sem exec
    # No POSIX o candidato ruim e um script SEM o bit de exec (PermissionError). No Windows nao
    # ha bit de exec: o equivalente e um arquivo de texto com nome executavel, que a sonda tenta
    # rodar e recebe WinError 193 (ENOEXEC). Os dois caem no mesmo ramo do `except` e provam a
    # mesma coisa: candidato ruim NAO encerra a busca, a sonda segue pro proximo diretorio.
    p1 = dir1 / ("claude.EXE" if _NT_REAL else "claude")
    p1.write_bytes(b"\x7fELF nao sou executavel deste sistema\n")
    if not _NT_REAL:
        p1.chmod(0o644)  # sem exec
    # d2/claude é bom
    _make_sh(dir2, "claude", exit_code=0)
    monkeypatch.setattr(cli_probe, "_path_login", os.pathsep.join([str(dir1), str(dir2)]))
    res = cli_probe.sondar_providers()
    assert res["claude"]["disponivel"] is True
    assert res["claude"]["motivo"] is None


def test_cache_nao_re_sonda_dentro_do_ttl(tmp_path, monkeypatch):
    _make_sh(tmp_path, "claude", exit_code=0)
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path))
    chamadas = {"n": 0}
    orig_run = subprocess.run

    def fake_run(*a, **kw):
        chamadas["n"] += 1
        return orig_run(*a, **kw)

    monkeypatch.setattr(subprocess, "run", fake_run)
    # primeira sonda
    cli_probe.sondar_providers()
    n1 = chamadas["n"]
    assert n1 >= 1
    # segunda sonda dentro do TTL não deve chamar subprocess de novo para --version
    # (o PATH também é cacheado, mas o ponto é o cache de 60s do resultado)
    cli_probe.sondar_providers()
    n2 = chamadas["n"]
    assert n2 == n1


def test_windows_path_com_drive_e_pathext(tmp_path, monkeypatch):
    # Windows: PATH com ; e drive letter, PATHEXT com .EXE/.CMD, binários com extensão
    monkeypatch.setattr(os, "name", "nt")
    monkeypatch.setattr(os, "pathsep", ";")
    monkeypatch.setenv("PATHEXT", ".COM;.EXE;.BAT;.CMD")
    dir1 = tmp_path / "bin1"
    dir1.mkdir()
    # No Linux o shebang roda com qualquer extensao, entao `.EXE`/`.CMD` sao so nomes. No Windows
    # eles precisam ser mesmo executaveis do sistema — `_make_sh` ja cuida disso e cria `.cmd`,
    # que esta no PATHEXT forjado abaixo.
    if _NT_REAL:
        _make_sh(dir1, "claude")
        _make_sh(dir1, "codex")
    else:
        _make_sh(dir1, "claude.EXE", exit_code=0)
        _make_sh(dir1, "codex.CMD", exit_code=0)
    # PATH com drive letter + ; + dir com binários
    fake_path = f"C:\\Users\\jefferson\\bin;{dir1}"
    monkeypatch.setattr(cli_probe, "_path_login", fake_path)
    res = cli_probe.sondar_providers()
    assert res["claude"]["disponivel"] is True
    assert res["claude"]["motivo"] is None
    assert res["codex"]["disponivel"] is True
    assert res["codex"]["motivo"] is None
    # pi e kimi não existem → nao_encontrado
    assert res["pi"]["disponivel"] is False
    assert res["kimi"]["disponivel"] is False
