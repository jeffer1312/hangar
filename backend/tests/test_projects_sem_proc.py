"""Caminho de deteccao de porta onde NAO ha /proc — Windows e macOS.

Roda no Linux forcando `_TEM_PROC = False`, igual ao test_procinfo: o psutil funciona aqui, entao
da pra exercitar contra sockets REAIS desta maquina.
"""
import socket

import psutil
import pytest

from app import projects
from app.models import RunInfo


@pytest.fixture
def sem_proc(monkeypatch):
    monkeypatch.setattr(projects, "_TEM_PROC", False)


def test_port_info_acha_a_porta_e_o_dono(sem_proc):
    import os
    with socket.socket() as srv:
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        porta = srv.getsockname()[1]
        info = projects._port_info({porta})
    escutando, dono = info[porta]
    assert escutando is True
    # Quem segura o socket e o proprio processo de teste, entao o dono tem que bater com o cwd
    # dele — e essa atribuicao que impede porta 3000 de creditar todo projeto configurado nela.
    assert dono == psutil.Process(os.getpid()).cwd() or dono == projects.DONO_INDETERMINADO


def test_port_info_porta_fechada_nao_inventa_dono(sem_proc):
    with socket.socket() as s:      # bind sem listen: ninguem aceita
        s.bind(("127.0.0.1", 0))
        porta = s.getsockname()[1]
    assert projects._port_info({porta}) == {porta: (False, None)}


def test_dono_indeterminado_nao_e_dono_de_ninguem():
    # Nao pode virar atribuicao falsa: "nao sei quem e" nunca casa com um cwd.
    assert projects._owns(projects.DONO_INDETERMINADO, "/qualquer/coisa") is False


def _status(runs, ports, cwd="/proj", porta=3000):
    return projects._status("p", {"cwd": cwd, "command": "npm run dev", "port": porta},
                            runs, ports)


def test_pane_vivo_com_porta_de_dono_indeterminado_e_running():
    # A regressao que este teste existe pra travar: com o dono indeterminavel (macOS sem root),
    # `not mine` era verdadeiro e o card ficava "starting" PARA SEMPRE, com o servidor servindo.
    runs = {projects.runner._slug("/proj"): RunInfo(command="npm run dev", since=1)}
    st = _status(runs, {3000: (True, projects.DONO_INDETERMINADO)})
    assert st.state == "running"


def test_sem_pane_com_dono_indeterminado_continua_stopped():
    # O contrario NAO vale: sem pane nosso nao ha nada pra creditar, e chamar de "external" seria
    # atribuir a este projeto uma porta que pode ser de qualquer outro processo da maquina.
    st = _status({}, {3000: (True, projects.DONO_INDETERMINADO)})
    assert st.state == "stopped"


def test_pane_vivo_com_porta_de_OUTRO_projeto_segue_starting():
    # Dono conhecido e alheio continua sendo "nao e meu" — a mudanca acima nao pode ter afrouxado
    # isto, senao o EADDRINUSE some do radar.
    runs = {projects.runner._slug("/proj"): RunInfo(command="npm run dev", since=1)}
    st = _status(runs, {3000: (True, "/outro/projeto")})
    assert st.state == "starting"
