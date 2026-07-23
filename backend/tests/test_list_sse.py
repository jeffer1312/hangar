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
    # Refresher unico (singleton): poll rapido pros testes; re-bind por event loop cuida do reset.
    monkeypatch.setattr(sse._list_refresher, "poll", 0.001)


class _Info:
    def __init__(self, name, state):
        self.name, self.state, self.cwd, self.jsonl, self.tracked, self.last_activity = name, state, "/p", f"/x/{name}.jsonl", True, None
        self.question = None
        self.stalled = False
        self.limited = False
        self.limit_reset = None
        self.then_target = None

    def model_dump(self, mode="json"):
        return {"name": self.name, "state": self.state, "cwd": self.cwd, "jsonl": self.jsonl, "tracked": self.tracked, "last_activity": self.last_activity, "question": self.question, "stalled": self.stalled, "limited": self.limited, "limit_reset": self.limit_reset, "then_target": self.then_target}


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
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 1))
    assert evs[0]["event"] == "sessions"
    assert json.loads(evs[0]["data"])[0]["name"] == "cc"


def test_emits_again_only_on_change(monkeypatch):
    seq = [[_Info("cc", "idle")], [_Info("cc", "idle")], [_Info("cc", "working")]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # connect-emit (idle), unchanged (idle, no emit), then working -> 2nd emit
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
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
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
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
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
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
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
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
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[0]["data"])[0]["limited"] is False
    assert json.loads(evs[1]["data"])[0]["limited"] is True
    assert json.loads(evs[1]["data"])[0]["limit_reset"] == "3pm"


def test_reemit_on_limit_reset_change(monkeypatch):
    # Mesmo com limited seguindo True, mudar SO o horario de reset re-emite (feature #8): o chip
    # "limitado · HH:MM" atualiza. Sem limit_reset na sig, a 2a emissao nao viria.
    a0 = _Info("cc", "working"); a0.limited = True; a0.limit_reset = "3pm"
    a1 = _Info("cc", "working"); a1.limited = True; a1.limit_reset = "4pm"
    seq = [[a0], [a1]]
    calls = {"i": 0}
    async def fake_list(_snap=None):
        r = seq[min(calls["i"], len(seq) - 1)]; calls["i"] += 1; return r
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
    assert [e["event"] for e in evs] == ["sessions", "sessions"]
    assert json.loads(evs[1]["data"])[0]["limit_reset"] == "4pm"


def test_ping_emitted_on_cadence(monkeypatch):
    async def fake_list(_snap=None):
        return [_Info("cc", "idle")]
    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_list)
    # ping em timer FIXO por conexao: apos o snapshot, o ping chega pelo seu proprio timer.
    evs = asyncio.run(_take(sse.list_events(ping_secs=0.001), 2))
    assert evs[0]["event"] == "sessions"
    assert any(e["event"] == "ping" for e in evs[1:])


def test_ping_not_blocked_by_slow_refresher(monkeypatch):
    # CERNE do desenho: o refresher (shared) pendurado num list_with_state lento (git status) NAO pode
    # atrasar o ping — a conexao so LE o snapshot e pinga em task propria. Ping chega mesmo travado.
    async def slow_list(_snap=None):
        await asyncio.sleep(9999)   # trava o refresher no 1o ciclo (nenhum snapshot pronto)

    monkeypatch.setattr(sse._list_registry, "list_with_state", slow_list)
    evs = asyncio.run(_take(sse.list_events(ping_secs=0.001), 1))
    assert evs[0]["event"] == "ping"   # ping chegou apesar do refresher pendurado (sem snapshot ainda)


def test_refresher_error_emits_list_error(monkeypatch):
    # Erro no refresher NAO derruba a conexao E aparece pro USUARIO: emite 'list_error' (front distingue
    # de offline; lista vazia por falha != zero sessoes). Sem ping (9999) -> o 1o evento e o list_error.
    async def boom(_snap=None):
        raise RuntimeError("list quebrou")
    monkeypatch.setattr(sse._list_registry, "list_with_state", boom)
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 1))
    assert evs[0]["event"] == "list_error"


def test_refresher_recovers_after_error_emits_sessions(monkeypatch):
    # Ciclo bom depois de um erro re-emite 'sessions' (o front LIMPA o list_error). 1o falha, depois ok.
    calls = {"i": 0}

    async def flaky(_snap=None):
        calls["i"] += 1
        if calls["i"] == 1:
            raise RuntimeError("primeiro ciclo falhou")
        return [_Info("cc", "idle")]

    monkeypatch.setattr(sse._list_registry, "list_with_state", flaky)
    evs = asyncio.run(_take(sse.list_events(ping_secs=9999), 2))
    assert evs[0]["event"] == "list_error"
    assert evs[1]["event"] == "sessions"
    assert json.loads(evs[1]["data"])[0]["name"] == "cc"


def test_status_sig_reduz_sem_relogio_e_custo():
    # Sig da statusline: modelo + ctx (baldes de 5%) + ⚡5h% + 📅7d%; relogio/custo fora — a linha
    # crua muda a cada captura e re-emitiria a lista inteira sem nada visivel mudar.
    from app import sse
    a = "🤖 Opus4.8·1M (high✦) │ 💬 20k/1k 40k/200k │ 💵 $1.23 │ ⚡5h:46% ↺34m │ 📅7d:57% │ ⏱ 2h37m"
    b = "🤖 Opus4.8·1M (high✦) │ 💬 20k/1k 40k/200k │ 💵 $9.99 │ ⚡5h:46% ↺12m │ 📅7d:57% │ ⏱ 2h41m"
    assert sse._status_sig(a) == sse._status_sig(b)          # so relogio/custo mudaram -> sig igual
    assert sse._status_sig(a)[0].startswith("Opus")
    assert sse._status_sig(a)[1] == 4                        # 40k/200k = 20% -> balde 4
    assert sse._status_sig(a)[2] == "46" and sse._status_sig(a)[3] == "57"
    c = a.replace("⚡5h:46%", "⚡5h:47%")
    assert sse._status_sig(a) != sse._status_sig(c)          # % mudou -> sig muda
    assert sse._status_sig(None) is None
    assert sse._status_sig("sem emojis") == (None, None, None, None)
