import asyncio, json
import pytest
from app import sse


@pytest.fixture(autouse=True)
def _stub_cached_list(monkeypatch):
    # list_events resolve via snapshot compartilhado (_cached_list) antes do list_with_state; stubba
    # pra nao varrer tmux//proc reais no teste (o fake de list_with_state ignora o snapshot).
    async def _fc():
        return []
    monkeypatch.setattr(sse, "_cached_list", _fc)


class _Info:
    def __init__(self, name, state):
        self.name, self.state, self.cwd, self.jsonl, self.tracked, self.last_activity = name, state, "/p", f"/x/{name}.jsonl", True, None
        self.question = None
        self.stalled = False
        self.limited = False
        self.limit_reset = None

    def model_dump(self, mode="json"):
        return {"name": self.name, "state": self.state, "cwd": self.cwd, "jsonl": self.jsonl, "tracked": self.tracked, "last_activity": self.last_activity, "question": self.question, "stalled": self.stalled, "limited": self.limited, "limit_reset": self.limit_reset}


async def _take(gen, n):
    out = []
    async for ev in gen:
        out.append(ev)
        if len(out) >= n:
            break
    return out


def test_emits_snapshot_on_connect(monkeypatch):
    async def fake_list(_snap=None):
        return [_Info("cc", "idle")]
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 1))
    assert evs[0]["event"] == "sessions"
    assert json.loads(evs[0]["data"])[0]["name"] == "cc"


def test_emits_again_only_on_change(monkeypatch):
    seq = [[_Info("cc", "idle")], [_Info("cc", "idle")], [_Info("cc", "working")]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # connect-emit (idle), unchanged (idle, no emit), then working -> 2nd emit
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["state"] == "idle"
    assert json.loads(evs[1]["data"])[0]["state"] == "working"


def test_no_reemit_on_last_activity_only_change(monkeypatch):
    # last_activity = mtime do jsonl (float sub-segundo); muda a cada escrita de uma sessao ativa.
    # Sozinho NAO pode re-emitir (senao a lista inteira pisca a cada poll). State change SIM emite.
    a0 = _Info("cc", "working"); a0.last_activity = 1.0
    a1 = _Info("cc", "working"); a1.last_activity = 2.5   # so last_activity mudou -> sem emit
    a2 = _Info("cc", "idle");    a2.last_activity = 3.0   # state mudou -> emite
    seq = [[a0], [a1], [a2]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["state"] == "working"
    assert json.loads(evs[1]["data"])[0]["state"] == "idle"
    assert json.loads(evs[1]["data"])[0]["last_activity"] == 3.0  # ainda no payload


def test_reemit_on_question_change(monkeypatch):
    # Uma sessao awaiting que troca a pergunta re-emite a lista (feature #1): a linha atualiza o texto,
    # mesmo com name/state/cwd/jsonl iguais. Sem question na sig, a 2a emissao nao viria.
    a0 = _Info("cc", "awaiting_input"); a0.question = "Qual ambiente?"
    a1 = _Info("cc", "awaiting_input"); a1.question = "Confirma o deploy?"
    seq = [[a0], [a1]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[1]["data"])[0]["question"] == "Confirma o deploy?"


def test_reemit_on_stalled_change(monkeypatch):
    # Uma sessao "working" que passa a stalled re-emite a lista (feature #7): a linha tinge mesmo com
    # name/state/cwd/jsonl/question iguais. Sem stalled na sig, a 2a emissao nao viria.
    a0 = _Info("cc", "working"); a0.stalled = False
    a1 = _Info("cc", "working"); a1.stalled = True
    seq = [[a0], [a1]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["stalled"] is False
    assert json.loads(evs[1]["data"])[0]["stalled"] is True


def test_reemit_on_limited_change(monkeypatch):
    # Uma sessao que bate no rate-limit re-emite a lista (feature #8): o chip "limitado" aparece mesmo
    # com name/state/cwd/jsonl/question/stalled iguais. Sem limited na sig, a 2a emissao nao viria.
    a0 = _Info("cc", "working"); a0.limited = False
    a1 = _Info("cc", "working"); a1.limited = True; a1.limit_reset = "3pm"
    seq = [[a0], [a1]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["limited"] is False
    assert json.loads(evs[1]["data"])[0]["limited"] is True
    assert json.loads(evs[1]["data"])[0]["limit_reset"] == "3pm"


def test_ping_emitted_on_cadence(monkeypatch):
    async def fake_list(_snap=None):
        return [_Info("cc", "idle")]
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # ping_every=1 -> after the connect snapshot, the next tick has no change but emits a ping
    evs = asyncio.run(_take(sse.list_events(poll=0.001, ping_every=1), 2))
    assert evs[0]["event"] == "sessions"
    assert evs[1]["event"] == "ping"
