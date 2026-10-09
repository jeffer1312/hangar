"""Nada que um teste agenda sobrevive a ele.

Em fila, um timer que sobra dispara centenas de testes depois e não incomoda ninguém; com a suíte
repartida entre processos (pytest-xdist, `scripts/verificar-local`), ele dispara no arquivo seguinte
e fala com o tmux no meio dele. Os dois testes rodam em ordem: o primeiro deixa o rastro, o segundo
confere que o conftest o limpou.
"""
import signal

from app import api

_SIGTERM_AT_IMPORT = signal.getsignal(signal.SIGTERM)

_scheduled = []


def test_a_send_schedules_the_delivery_confirmation():
    api._agendar_confirmacao("isolation-fixture", 60)
    timer, _ = api._confirm_pend["isolation-fixture"]
    _scheduled.append(timer)


def test_the_previous_test_confirmation_does_not_outlive_it():
    assert "isolation-fixture" not in api._confirm_pend
    assert [timer.is_alive() for timer in _scheduled] == [False]


def test_a_server_leaves_its_signal_handler_and_sse_exit_flag():
    from sse_starlette.sse import AppStatus
    signal.signal(signal.SIGTERM, lambda *_: None)
    AppStatus.should_exit = True


def test_the_previous_signal_handler_and_sse_exit_flag_do_not_outlive_it():
    # Um tratador de SIGTERM que aponta para um uvicorn encerrado faz o sse-starlette ligar o
    # should_exit global, e toda resposta SSE seguinte no processo termina sem resposta.
    from sse_starlette.sse import AppStatus
    assert signal.getsignal(signal.SIGTERM) is _SIGTERM_AT_IMPORT
    assert AppStatus.should_exit is False
