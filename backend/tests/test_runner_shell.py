"""O comando de projeto e uma LINHA DE SHELL — e o shell do POSIX nao existe no Windows.

`start_command`/`stop_command` sao escritos pelo usuario e podem ter sintaxe de shell
(`cd x && npm run dev`), entao precisam de um shell de verdade. O codigo velho assumia POSIX nos
dois pontos, e medido nesta VM em 22/08/2026 os dois falhavam no Windows:

  - start (`runner`): `exec {SHELL} -lc <cmd>` no pane do psmux nao executa NADA. Nem citando o
    SHELL — aqui ele e o bash do Git, com espaco no caminho —, e `exec` tambem nao existe no
    PowerShell, que e o default-shell do psmux. O mesmo comando SEM envelope roda.
  - stop (`projects`): `/bin/sh` nao existe -> FileNotFoundError [WinError 2], caindo na mensagem
    generica "stop_command falhou" que nao dizia o motivo.

E medido o que FUNCIONA la: `cmd /c <linha>` executa com `&&` e tudo, no pane e por subprocess.

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


def test_windows_nao_usa_bin_sh_no_stop(win):
    argv = runner.argv_de_shell(CMD)
    assert argv[0].endswith("cmd.exe")
    assert argv[1] == "/c"
    assert argv[2] == CMD


def test_posix_fica_byte_identico(posix):
    """O ramo POSIX e a string de antes, caractere por caractere — inclusive o `exec` e o quote."""
    assert runner._linha_de_shell_no_pane(CMD) == f"exec /bin/zsh -lc {shlex.quote(CMD)}"
    assert runner.argv_de_shell(CMD) == ["/bin/sh", "-lc", CMD]


def test_posix_sem_SHELL_cai_no_bin_sh(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "posix")
    monkeypatch.delenv("SHELL", raising=False)
    assert runner._linha_de_shell_no_pane("x") == f"exec /bin/sh -lc {shlex.quote('x')}"


def test_windows_sem_COMSPEC_cai_no_cmd_exe(monkeypatch):
    monkeypatch.setattr(runner.os, "name", "nt")
    monkeypatch.delenv("COMSPEC", raising=False)
    assert runner._linha_de_shell_no_pane("x") == "cmd.exe /c x"
    assert runner.argv_de_shell("x") == ["cmd.exe", "/c", "x"]


def test_o_stop_usa_o_mesmo_lugar_que_o_start():
    """Duas verdades divergindo foi o defeito: o stop tinha `/bin/sh` proprio, chumbado."""
    import pathlib
    fonte = pathlib.Path(runner.__file__).parent.joinpath("projects.py").read_text(encoding="utf-8")
    assert "runner.argv_de_shell(" in fonte
    assert '"/bin/sh"' not in fonte
