"""Nada que um teste agenda sobrevive a ele.

Em fila, um timer que sobra dispara centenas de testes depois e não incomoda ninguém; com a suíte
repartida entre processos (pytest-xdist, `scripts/verificar-local`), ele dispara no arquivo seguinte
e fala com o tmux no meio dele. Os dois testes rodam em ordem: o primeiro deixa o rastro, o segundo
confere que o conftest o limpou.
"""
from app import api

_scheduled = []


def test_a_send_schedules_the_delivery_confirmation():
    api._agendar_confirmacao("isolation-fixture", 60)
    timer, _ = api._confirm_pend["isolation-fixture"]
    _scheduled.append(timer)


def test_the_previous_test_confirmation_does_not_outlive_it():
    assert "isolation-fixture" not in api._confirm_pend
    assert [timer.is_alive() for timer in _scheduled] == [False]
