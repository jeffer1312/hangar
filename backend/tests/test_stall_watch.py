import asyncio
import pytest
from app import stall_watch


@pytest.fixture(autouse=True)
def _reset_notified():
    # Sets globais (dedupe entre ciclos) -> zera entre testes pra nao vazar de um pro outro.
    def _clear():
        stall_watch._notified.clear()
        stall_watch._notified_limited.clear()
        stall_watch._armed_limited.clear()
        stall_watch._seen_live.clear()
        stall_watch._notified_dead.clear()
    _clear()
    yield
    _clear()


class _Info:
    def __init__(self, name, stalled, limited=False, limit_reset=None, jsonl=None):
        self.name, self.stalled = name, stalled
        self.limited, self.limit_reset = limited, limit_reset
        self.jsonl = jsonl


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
# Feature #2: death ping — sessao que sumiu da lista entre ciclos = morreu (o marcador do state_hook
# nunca emite "dead", entao o watchdog e o unico gatilho do push de "caiu")
# ---------------------------------------------------------------------------

def test_tick_notifies_dead_when_session_disappears(monkeypatch):
    monkeypatch.setattr(stall_watch.push, "notify_stalled", lambda *a, **k: None)
    dead = []
    monkeypatch.setattr(stall_watch.push, "notify_dead", dead.append)
    seq = [[_Info("cc", False)], []]  # viva -> ausente

    async def list_fn():
        return seq.pop(0)

    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    assert dead == ["cc"]


def test_tick_dead_notifies_once(monkeypatch):
    # Ausente por varios ciclos -> so a transicao viva->ausente pinga (dedupe), nao a cada poll.
    monkeypatch.setattr(stall_watch.push, "notify_stalled", lambda *a, **k: None)
    dead = []
    monkeypatch.setattr(stall_watch.push, "notify_dead", dead.append)
    seq = [[_Info("cc", False)], [], []]

    async def list_fn():
        return seq.pop(0)

    for _ in range(3):
        asyncio.run(stall_watch._tick(list_fn))
    assert dead == ["cc"]


def test_tick_dead_cleans_working_started(monkeypatch):
    # Morte limpa o _working_started (api.py, keyed por session_id = stem do jsonl) pra nao vazar.
    from app import api
    monkeypatch.setattr(stall_watch.push, "notify_stalled", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.push, "notify_dead", lambda *a, **k: None)
    api._working_started["uuid1"] = 100.0
    seq = [[_Info("cc", False, jsonl="/x/uuid1.jsonl")], []]

    async def list_fn():
        return seq.pop(0)

    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    assert "uuid1" not in api._working_started


def test_tick_dead_respects_notify_flag(monkeypatch):
    monkeypatch.setattr(stall_watch.push, "notify_stalled", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "notify_dead", False)
    dead = []
    monkeypatch.setattr(stall_watch.push, "notify_dead", dead.append)
    seq = [[_Info("cc", False)], []]

    async def list_fn():
        return seq.pop(0)

    asyncio.run(stall_watch._tick(list_fn))
    asyncio.run(stall_watch._tick(list_fn))
    assert dead == []  # CP_NOTIFY_DEAD off -> nao pinga


def test_tick_dead_rearms_on_reappear(monkeypatch):
    # Nome reusado por sessao nova: morreu, reapareceu vivo, morreu de novo -> 2 pings (nao fica mudo).
    monkeypatch.setattr(stall_watch.push, "notify_stalled", lambda *a, **k: None)
    dead = []
    monkeypatch.setattr(stall_watch.push, "notify_dead", dead.append)
    seq = [[_Info("cc", False)], [], [_Info("cc", False)], []]

    async def list_fn():
        return seq.pop(0)

    for _ in range(4):
        asyncio.run(stall_watch._tick(list_fn))
    assert dead == ["cc", "cc"]


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


def test_auto_resume_requires_master_switch(monkeypatch):
    # Feature #12: o kill-switch mestre (automations_enabled) e um portao ADICIONAL -- auto-resume so
    # arma com ELE *e* o proprio CP_AUTO_RESUME ligados. Aqui o auto_resume esta ON mas o mestre OFF.
    monkeypatch.setattr(stall_watch.push, "notify_limited", lambda *a, **k: None)
    monkeypatch.setattr(stall_watch.settings, "auto_resume", True)
    monkeypatch.setattr(stall_watch.settings, "automations", False)
    monkeypatch.setattr(stall_watch, "_has_undelivered_queue", lambda name: True)
    armed = []
    monkeypatch.setattr(stall_watch.threading, "Timer", lambda *a, **k: armed.append(a) or _FakeTimer())

    async def list_fn():
        return [_Info("cc", False, limited=True, limit_reset="3pm")]

    asyncio.run(stall_watch._tick(list_fn))
    assert armed == []  # mestre desligado -> nao arma, mesmo com auto_resume ON + fila + reset bons
