import asyncio
import pytest
from app import stall_watch


@pytest.fixture(autouse=True)
def _reset_notified():
    # Sets globais (dedupe entre ciclos) -> zera entre testes pra nao vazar de um pro outro.
    stall_watch._notified.clear()
    stall_watch._notified_limited.clear()
    stall_watch._armed_limited.clear()
    yield
    stall_watch._notified.clear()
    stall_watch._notified_limited.clear()
    stall_watch._armed_limited.clear()


class _Info:
    def __init__(self, name, stalled, limited=False, limit_reset=None):
        self.name, self.stalled = name, stalled
        self.limited, self.limit_reset = limited, limit_reset


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


# ---------------------------------------------------------------------------
# Feature #8: rate-limit radar — notify-once + dedupe (mesmo padrao do stall acima)
# ---------------------------------------------------------------------------

def test_tick_notifies_once_while_limited(monkeypatch):
    sent = []
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda name, reset=None: sent.append((name, reset)))

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="3pm")]

    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    assert sent == [("cc", "3pm")]  # 1 push so, nao 3


def test_tick_rearms_limited_after_session_recovers(monkeypatch):
    sent = []
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda name, reset=None: sent.append(name))
    seq = [
        [_Info("cc", False, limited=True, limit_reset="3pm")],
        [_Info("cc", False, limited=False)],
        [_Info("cc", False, limited=True, limit_reset="3pm")],
    ]

    async def list_fn():
        return seq.pop(0)

    asyncio.run(stall_watch._tick(list_fn))  # entra em limited -> notifica
    asyncio.run(stall_watch._tick(list_fn))  # saiu -> re-arma (sem notificar de novo)
    asyncio.run(stall_watch._tick(list_fn))  # limited de novo -> notifica outra vez
    assert sent == ["cc", "cc"]


def test_tick_stalled_and_limited_are_independent(monkeypatch):
    # Uma sessao pode ficar SO limited (nao stalled) e vice-versa: sets de dedupe separados.
    stalled_sent, limited_sent = [], []
    monkeypatch.setattr(stall_watch.push, "notify_stalled", stalled_sent.append)
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda name, reset=None: limited_sent.append(name))

    async def list_fn():
        return [_Info("cc", True, limited=True, limit_reset="15:30")]

    asyncio.run(stall_watch._tick(list_fn))
    assert stalled_sent == ["cc"]
    assert limited_sent == ["cc"]


# ---------------------------------------------------------------------------
# Feature #8: auto-resume opt-in (CP_AUTO_RESUME) — so arma com flag ON + fila nao-entregue + reset parseavel
# ---------------------------------------------------------------------------

def test_auto_resume_off_by_default(monkeypatch, tmp_path):
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "auto_resume", False)
    armed = []
    monkeypatch.setattr(stall_watch.threading, "Timer", lambda *a, **k: armed.append(a) or _FakeTimer())
    monkeypatch.setattr(stall_watch, "_has_undelivered_queue", lambda name: True)

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="3pm")]

    asyncio.run(stall_watch._tick(list_fn))
    assert armed == []  # flag desligada (default) -> nunca arma, mesmo com fila + reset bom


class _FakeTimer:
    def start(self):
        pass


def test_auto_resume_skips_without_queued_work(monkeypatch):
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "auto_resume", True)
    monkeypatch.setattr(stall_watch, "_has_undelivered_queue", lambda name: False)
    armed = []
    monkeypatch.setattr(stall_watch.threading, "Timer", lambda *a, **k: armed.append(a) or _FakeTimer())

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="3pm")]

    asyncio.run(stall_watch._tick(list_fn))
    assert armed == []  # flag ON, mas sem fila pendente -> nao arma


def test_auto_resume_skips_unparseable_reset(monkeypatch):
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "auto_resume", True)
    monkeypatch.setattr(stall_watch, "_has_undelivered_queue", lambda name: True)
    armed = []
    monkeypatch.setattr(stall_watch.threading, "Timer", lambda *a, **k: armed.append(a) or _FakeTimer())

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="algum dia")]  # nao parseia

    asyncio.run(stall_watch._tick(list_fn))
    assert armed == []  # reset nao-parseavel -> nao arma timer sobre um chute ruim


def test_auto_resume_arms_timer_at_drain(monkeypatch):
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "auto_resume", True)
    monkeypatch.setattr(stall_watch, "_has_undelivered_queue", lambda name: True)
    armed = []

    def _fake_timer(delay, fn, args=()):
        armed.append((delay, fn, args))
        return _FakeTimer()

    monkeypatch.setattr(stall_watch.threading, "Timer", _fake_timer)

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="3pm")]

    asyncio.run(stall_watch._tick(list_fn))
    assert len(armed) == 1
    delay, fn, args = armed[0]
    assert args == ("cc",)
    assert delay > 0
    # Nao re-arma no proximo tick (mesma sessao ainda limited): 1 Timer so por janela.
    asyncio.run(stall_watch._tick(list_fn))
    assert len(armed) == 1
