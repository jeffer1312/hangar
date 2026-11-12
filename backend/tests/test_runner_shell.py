"""O comando do runner é uma LINHA DE SHELL, e o shell do POSIX não existe no Windows.

O comando é escrito pelo usuário e pode ter sintaxe de shell (`cd x && npm run dev`), então
precisa de um shell de verdade. `exec {SHELL} -lc <cmd>` no pane do psmux não executa NADA: nem
citando o SHELL (o bash do Git, com espaço no caminho), e `exec` não existe no PowerShell, que é o
default-shell do psmux. `cmd /c <linha>` executa com `&&` e tudo.

Os casos forcam `os.name` nos dois valores, entao valem — e falham contra o codigo velho — nos
dois sistemas.
"""
import shlex

import pytest

from app import runner


@pytest.fixture
def win(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "nt")
    monkeypatch.setenv("COMSPEC", r"C:\WINDOWS\system32\cmd.exe")


@pytest.fixture
def posix(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "posix")
    monkeypatch.setenv("SHELL", "/bin/zsh")


CMD = "cd sub && npm run dev"


def test_windows_nao_usa_shell_posix_no_pane(win):
    linha = runner._linha_de_shell_no_pane(CMD)
    assert "/bin/sh" not in linha and " -lc " not in linha and not linha.startswith("exec ")
    assert linha == r"C:\WINDOWS\system32\cmd.exe /c cd sub && npm run dev"
    # Sem citacao POSIX: o `cmd /c` toma o RESTO da linha, entao citar viraria argumento literal.
    assert shlex.quote(CMD) not in linha


def test_posix_fica_byte_identico(posix):
    """O ramo POSIX e a string de antes, caractere por caractere — inclusive o `exec` e o quote."""
    assert runner._linha_de_shell_no_pane(CMD) == f"exec /bin/zsh -lc {shlex.quote(CMD)}"


def test_posix_sem_SHELL_cai_no_bin_sh(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "posix")
    monkeypatch.delenv("SHELL", raising=False)
    assert runner._linha_de_shell_no_pane("x") == f"exec /bin/sh -lc {shlex.quote('x')}"


def test_windows_sem_COMSPEC_cai_no_cmd_exe(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "nt")
    monkeypatch.delenv("COMSPEC", raising=False)
    assert runner._linha_de_shell_no_pane("x") == "cmd.exe /c x"
