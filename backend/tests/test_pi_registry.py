import os
import pytest
from app import registry
from app import procinfo


def _pane(monkeypatch, cmd: str):
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: cmd)
    # _argv junto: provider_of_pane le o argv SEPARADO (procinfo._argv), nao a string.
    monkeypatch.setattr(registry, "_argv", lambda p: cmd.split())


def test_detects_pi_from_the_process_command(monkeypatch):
    # Deteccao pelo executavel REAL. Casar por nome de sessao ou por cwd marcaria como Pi
    # qualquer sessao aberta na mesma pasta.
    _pane(monkeypatch, "pi --session-id abc")
    assert registry.provider_of_pane(99) == "pi"
    _pane(monkeypatch, "/usr/bin/pi -e x.ts --session-id abc")
    assert registry.provider_of_pane(99) == "pi"


def test_detects_pi_with_the_argv_rewritten(monkeypatch):
    # Como o pane REAL aparece (Task 0, fato 7): o pi sobrescreve o proprio argv e o /proc devolve
    # "pi" seguido de NUL (que _cmdline vira espaco). Sem o strip/split, argv0 nunca casa.
    _pane(monkeypatch, "pi" + " " * 120)
    assert registry.provider_of_pane(99) == "pi"


def test_does_not_match_a_command_merely_containing_pi(monkeypatch):
    # `pip install`, `pipx`, `mpirun` e um caminho com /pi/ dentro nao sao o agente Pi.
    for cmd in ("pip install requests", "pipx run x", "python -m pip list",
                "mpirun -n 2 a.out", "/opt/pi/bin/node server.js"):
        _pane(monkeypatch, cmd)
        assert registry.provider_of_pane(99) != "pi", cmd


def test_provider_of_pane_reads_proc_not_a_pane_field(monkeypatch):
    # list_panes_active() NAO devolve o comando (tmux.py:106-122): ele vem de /proc, como
    # _repl_sid ja faz. Este teste trava esse contrato — se alguem "simplificar" pra ler um campo
    # inexistente do pane, todo pane vira "claude" calado.
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11, 12])
    _cmds = {11: "fish", 12: "pi --session-id x"}
    monkeypatch.setattr(registry, "_cmdline", lambda p: _cmds.get(p, ""))
    monkeypatch.setattr(registry, "_argv", lambda p: _cmds.get(p, "").split())
    assert registry.provider_of_pane(99) == "pi"

    _cmds2 = {11: "fish", 12: "claude --session-id x"}
    monkeypatch.setattr(registry, "_cmdline", lambda p: _cmds2.get(p, ""))
    monkeypatch.setattr(registry, "_argv", lambda p: _cmds2.get(p, "").split())
    assert registry.provider_of_pane(99) == "claude"


def test_detects_kimi_pelas_duas_grafias_do_argv0(monkeypatch):
    # A 0.36.x reescreve o argv0 como `kimi`; a 0.37.2 como `kimi-code` (medido: a sessao nova
    # virava "claude" na re-descoberta e herdava o transcript do Claude do mesmo cwd).
    _pane(monkeypatch, "kimi ")
    assert registry.provider_of_pane(99) == "kimi"
    _pane(monkeypatch, "kimi-code" + " " * 120)
    assert registry.provider_of_pane(99) == "kimi"
    _pane(monkeypatch, "/usr/bin/kimi-code --model apikey/k3")
    assert registry.provider_of_pane(99) == "kimi"


def test_provider_of_pane_defaults_to_claude_when_nothing_matches(monkeypatch):
    # Default preserva o comportamento de hoje: qualquer pane nao reconhecido segue tratado como
    # Claude, como antes desta task existir.
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: "vim")
    monkeypatch.setattr(registry, "_argv", lambda p: ["vim"])
    assert registry.provider_of_pane(99) == "claude"


def _fake_proc_start(monkeypatch, tmp_path, nasceu: float):
    """Faz o processo do pane 'nascer' em `nasceu`, na fonte que a plataforma de fato le."""
    if not procinfo._TEM_PROC:
        # Sem /proc quem responde e o psutil, e nao ha arquivo pra falsificar: o duble e a propria
        # `_start_time_psutil`. O que estes casos provam continua sendo a logica do
        # `pi_session_file`; o parser do /proc tem caso proprio, marcado como POSIX.
        monkeypatch.setattr(procinfo, "_start_time_psutil", lambda pid: nasceu)
        return
    # /proc/<pid>/stat de mentira com o processo nascendo em `nasceu` (epoch). O comm leva espaco E
    # parentese de proposito: e o caso que quebra qualquer contagem de campo feita da esquerda.
    with open("/proc/stat") as fh:
        btime = next(float(l.split()[1]) for l in fh if l.startswith("btime "))
    ticks = (nasceu - btime) * os.sysconf("SC_CLK_TCK")
    stat = tmp_path / "stat"
    stat.write_text(f"7 (pi (fork) x) S " + "0 " * 18 + f"{ticks:.0f} 0 0\n")
    monkeypatch.setattr(procinfo, "_proc_stat_path", lambda pid: str(stat))


@pytest.mark.skipif(not procinfo._TEM_PROC, reason="exercita o parser de /proc/<pid>/stat")
def test_proc_start_time_survives_a_comm_with_spaces_and_parens(monkeypatch, tmp_path):
    # Sem o rindex(")") o campo 22 sai errado por alguns tokens e o frescor do bilhete vira ruido.
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)
    assert abs(registry._proc_start_time(7) - 1_700_000_000.0) < 1
    monkeypatch.setattr(procinfo, "_proc_stat_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry._proc_start_time(7) is None      # degrada como os vizinhos de /proc
