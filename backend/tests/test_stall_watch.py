import asyncio
import pytest
from app import stall_watch


@pytest.fixture(autouse=True)
def _reset_notified():
    # Set global (dedupe entre ciclos) -> zera entre testes pra nao vazar de um pro outro.
    stall_watch._notified.clear()
    yield
    stall_watch._notified.clear()


class _Info:
    def __init__(self, name, stalled):
        self.name, self.stalled = name, stalled


def test_tick_notifies_once_while_stalled(monkeypatch):
    # 3 ciclos com a MESMA sessao travada -> so a 1a transicao dispara push (dedupe).
    sent = []
    monkeypatch.setattr(stall_watch.push, "notify_stalled", sent.append)

    async def list_fn():
        return [_Info("cc", True)]

    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    assert sent == ["cc"]  # 1 push so, nao 3


def test_tick_rearms_after_session_recovers(monkeypatch):
    # Travou -> notifica; destravou (state mudou/last_activity avancou); travou nas mesma sessao de
    # novo -> notifica OUTRA vez (nao fica preso mudo pra sempre).
    sent = []
    monkeypatch.setattr(stall_watch.push, "notify_stalled", sent.append)
    seq = [[_Info("cc", True)], [_Info("cc", False)], [_Info("cc", True)]]

    async def list_fn():
        return seq.pop(0)

    asyncio.run(stall_watch._tick(list_fn))  # entra em stalled -> notifica
    asyncio.run(stall_watch._tick(list_fn))  # saiu de stalled -> re-arma (sem notificar de novo)
    asyncio.run(stall_watch._tick(list_fn))  # travou de novo -> notifica outra vez
    assert sent == ["cc", "cc"]


def test_tick_no_notify_when_nothing_stalled(monkeypatch):
    sent = []
    monkeypatch.setattr(stall_watch.push, "notify_stalled", sent.append)

    async def list_fn():
        return [_Info("cc", False)]

    asyncio.run(stall_watch._tick(list_fn))
    assert sent == []
