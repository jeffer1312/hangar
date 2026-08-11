"""As duas implementacoes do procinfo tem que concordar sobre o MESMO processo.

Isto roda no Linux. O psutil funciona aqui (ele proprio le /proc), entao da pra exercitar o
caminho de Windows/macOS contra processos REAIS desta maquina e comparar com o que o caminho
/proc devolve. Sem isto, o codigo que so roda fora do Linux nunca seria testado por quem
desenvolve no Linux — e a primeira vez que alguem descobriria um erro seria no Windows.
"""
import os
from pathlib import Path

import psutil
import pytest

from app import procinfo


@pytest.fixture
def via_psutil(monkeypatch):
    """Forca o despacho pro lado psutil.

    `procinfo.psutil` nao existe no Linux (o import e condicional), dai `raising=False`.
    """
    monkeypatch.setattr(procinfo, "psutil", psutil, raising=False)
    monkeypatch.setattr(procinfo, "_TEM_PROC", False)


def test_no_linux_o_despacho_escolhe_o_proc():
    # Guarda da promessa central: em maquina com /proc nada muda. Se este teste falhar num
    # Linux, alguem trocou o caminho quente por psutil sem querer.
    assert procinfo._TEM_PROC is True


def test_cmdline_igual_nas_duas_implementacoes(via_psutil):
    eu = os.getpid()
    # O lado /proc troca NUL por espaco e sobra um no fim; o psutil junta com espaco. O que os
    # chamadores usam (_session_id_from_cmdline) e busca de substring, entao comparo normalizado.
    assert procinfo._cmdline(eu).split() == _cmdline_proc_direto(eu).split()


def test_environ_igual_nas_duas_implementacoes(via_psutil, monkeypatch):
    monkeypatch.setenv("CP_ENGINE", "kimi")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/tmp/cfg-de-teste")
    # Nao da pra reler o proprio environ mudado (o /proc/self/environ congela no exec), entao a
    # comparacao aqui e de FORMATO: o psutil devolve dict de str, com as chaves que os
    # chamadores procuram.
    env = procinfo._env_psutil(os.getpid())
    assert isinstance(env, dict)
    assert all(isinstance(k, str) and isinstance(v, str) for k, v in env.items())
    assert "PATH" in env


def test_start_time_bate_com_o_do_proc(via_psutil):
    eu = os.getpid()
    # Mesmo instante de nascimento pelos dois caminhos. 1s de folga: o lado /proc reconstroi de
    # ticks+btime e arredonda; o psutil devolve o valor direto.
    assert abs(procinfo._proc_start_time(eu) - psutil.Process(eu).create_time()) < 1


def test_children_map_acha_este_processo_sob_o_pai(via_psutil):
    mapa = procinfo._proc_children_map()
    assert os.getpid() in mapa.get(os.getppid(), [])


def test_open_jsonl_desiste_de_proposito_fora_do_linux(via_psutil, tmp_path):
    # A UNICA das sete que NAO tem versao portatil: fora do Linux devolve None SEMPRE, mesmo com o
    # transcript aberto bem debaixo do nariz. `psutil.open_files()` no Windows enumera a tabela de
    # handles do sistema inteiro e a propria doc avisa que pode levar SEGUNDOS — isto roda por
    # descendente, por sessao, num poll de 1,5s, e pararia o backend. O sinal perdido e barato: o
    # claude nao segura o fd em idle, e a resolucao autoritativa vem do --session-id do cmdline.
    projects = tmp_path / "projects"
    projects.mkdir()
    alvo = projects / "2026_abc.jsonl"
    alvo.write_text("{}\n")
    with open(alvo):   # aberto de verdade — o lado /proc acharia; o portatil nem procura
        assert procinfo._open_jsonl(os.getpid(), projects) is None


def test_open_jsonl_acha_o_transcript_aberto_no_proc(tmp_path):
    # Cobertura do caminho /proc, que NAO mudou (sem a fixture: _TEM_PROC real).
    projects = tmp_path / "projects"
    projects.mkdir()
    alvo = projects / "2026_abc.jsonl"
    alvo.write_text("{}\n")
    with open(alvo):   # precisa estar ABERTO: e um fd que se procura, nao um arquivo em disco
        assert procinfo._open_jsonl(os.getpid(), projects) == str(alvo)


def test_open_jsonl_ignora_jsonl_fora_do_projects_dir(tmp_path):
    # Armadilha do lado /proc: dir IRMAO de mesmo prefixo nao pode casar.
    projects = tmp_path / "projects"
    projects.mkdir()
    (tmp_path / "projects-outro").mkdir()
    fora = tmp_path / "projects-outro" / "2026_abc.jsonl"
    fora.write_text("{}\n")
    with open(fora):
        assert procinfo._open_jsonl(os.getpid(), projects) is None


def test_pid_morto_degrada_igual_ao_lado_proc(via_psutil):
    # Contrato de degradacao: processo inexistente devolve vazio, NUNCA excecao. Uma
    # psutil.NoSuchProcess escapando viraria 500 no meio de um poll de listagem so porque uma
    # sessao morreu entre duas leituras.
    morto = 2**22   # acima de /proc/sys/kernel/pid_max em qualquer maquina realista
    assert procinfo._cmdline(morto) == ""
    assert procinfo._env_psutil(morto) == {}
    assert procinfo._proc_start_time(morto) is None
    assert procinfo._config_dir_of(morto) is None
    assert procinfo._engine_of(morto) is None
    assert procinfo._open_jsonl(morto, "/qualquer") is None


def _cmdline_proc_direto(pid: int) -> str:
    with open(f"/proc/{pid}/cmdline", "rb") as fh:
        return fh.read().replace(b"\x00", b" ").decode(errors="replace")


def test_pids_com_config_dir_acha_no_proc_fake(monkeypatch, tmp_path):
    """Varredura de processos por CLAUDE_CONFIG_DIR (a guarda do DELETE de conta contra CLI vivo
    fora do tmux), exercitada contra um /proc de mentira: casa pelo valor do env, ignora quem não
    tem a var e ignora entradas não numéricas."""
    raiz = tmp_path / "proc"
    (raiz / "100").mkdir(parents=True)
    (raiz / "100" / "environ").write_bytes(
        b"PATH=/bin\x00CLAUDE_CONFIG_DIR=/x/.claude-conta2\x00")
    (raiz / "101").mkdir()
    (raiz / "101" / "environ").write_bytes(b"PATH=/bin\x00")
    (raiz / "nao-pid").mkdir()
    (raiz / "nao-pid" / "environ").write_bytes(b"CLAUDE_CONFIG_DIR=/x/.claude-conta2\x00")
    monkeypatch.setattr(procinfo, "_PROC_ROOT", str(raiz))
    assert procinfo._pids_com_config_dir(Path("/x/.claude-conta2")) == ([100], True)
    assert procinfo._pids_com_config_dir(Path("/x/.claude-outra")) == ([], True)


def test_pids_com_config_dir_psutil_casa_pelo_env(via_psutil, monkeypatch):
    """Lado psutil: o pid cujo env fake casa entra, os outros não — mesmo contrato do /proc."""
    eu = os.getpid()

    def env_fake(pid):
        return {"CLAUDE_CONFIG_DIR": "/x/.claude-conta2"} if pid == eu else {}

    monkeypatch.setattr(procinfo, "_env_psutil", env_fake)
    assert procinfo._pids_com_config_dir(Path("/x/.claude-conta2")) == ([eu], True)
    assert procinfo._pids_com_config_dir(Path("/x/.claude-outra")) == ([], True)


def test_pids_com_config_dir_pid_morto_nao_levanta(via_psutil):
    # Mesmo contrato de degradacao dos vizinhos: pid morto/ilegivel nunca vira excecao.
    assert procinfo._pids_com_config_dir(Path("/tmp/.claude-conta-que-nao-existe-xyz")) == ([], True)
