"""Nada que um teste agenda sobrevive a ele.

Em fila, um timer que sobra dispara centenas de testes depois e não incomoda ninguém; com a suíte
repartida entre processos (pytest-xdist, `scripts/verificar-local`), ele dispara no arquivo seguinte
e fala com o tmux no meio dele. Os dois testes rodam em ordem: o primeiro deixa o rastro, o segundo
confere que o conftest o limpou.
"""
import threading

from app import api


def test_a_send_schedules_the_delivery_confirmation():
    api._agendar_confirmacao("isolation-fixture", 60)
    assert "isolation-fixture" in api._confirm_pend


def test_the_previous_test_confirmation_does_not_outlive_it():
    assert api._confirm_pend == {}
    alive = [t for t in threading.enumerate() if isinstance(t, threading.Timer) and t.is_alive()]
    assert alive == []
