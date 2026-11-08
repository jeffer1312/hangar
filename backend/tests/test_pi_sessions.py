import os

import pytest

from app.adapters.pi import sessions as pi_sessions

# Os dois casos de slug abaixo espelham diretorios REAIS de `~/.pi/agent/sessions` na maquina
# Linux. O slug e o caminho ABSOLUTO com os separadores virando `-`, entao caminho POSIX literal
# so faz sentido la: no Windows o `os.path.abspath("/home/jefferson")` ganha a letra do drive e o
# esperado passaria a medir o abspath, nao a regra. O gemeo logo abaixo cobre o Windows com o
# valor medido no disco DESTA VM.
so_posix = pytest.mark.skipif(os.name != "posix", reason="slug de caminho POSIX; ver o gemeo do Windows")


@so_posix
def test_cwd_slug_matches_pi_layout():
    # Medido na maquina alvo: /home/jefferson -> --home-jefferson--
    assert pi_sessions.cwd_slug("/home/jefferson") == "--home-jefferson--"
    assert (pi_sessions.cwd_slug("/home/jefferson/Projetos/hangar")
            == "--home-jefferson-Projetos-hangar--")


@so_posix
def test_cwd_slug_keeps_spaces_accents_and_underscores():
    # Diretorios REAIS em ~/.pi/agent/sessions (Pi 0.82.1): so o separador de caminho vira '-'.
    # Trocar todo nao-alfanumerico devolvia `--home-jefferson--rea-de-trabalho-...--`, que nao
    # existe — e ai transcript_path() nao achava nada e o fallback por CP_PI_SESSION ficava morto.
    assert (pi_sessions.cwd_slug("/home/jefferson/Área de trabalho/repos/servicos_api")
            == "--home-jefferson-Área de trabalho-repos-servicos_api--")
    assert (pi_sessions.cwd_slug("/tmp/claude-1000/-home-jefferson-Projetos/scratchpad/piprobe")
            == "--tmp-claude-1000--home-jefferson-Projetos-scratchpad-piprobe--")


@pytest.mark.skipif(os.name != "nt", reason="slug de caminho Windows; ver os gemeos POSIX")
def test_cwd_slug_no_windows_leva_a_letra_do_drive():
    """Medido em 22/08/2026 na VM Windows: com o Pi 0.84.2 rodando em `C:\\cockpit`, o diretorio
    que ele criou em `~/.pi/agent/sessions` chama-se `--C--cockpit--` — a letra do drive entra
    como se fosse o primeiro componente, e as duas barras viram `-`. As duas grafias de entrada
    (barra e contrabarra) tem que cair no MESMO slug, senao o transcript_path erra conforme quem
    chamou."""
    assert pi_sessions.cwd_slug("C:\\cockpit") == "--C--cockpit--"
    assert pi_sessions.cwd_slug("C:/cockpit") == "--C--cockpit--"
    assert pi_sessions.cwd_slug("C:\\cockpit\\backend") == "--C--cockpit-backend--"
