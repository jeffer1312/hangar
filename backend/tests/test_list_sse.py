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
    # None = nada a mandar de terminais de atalho: os testes de `sessions` nao contam esse evento.
    monkeypatch.setattr(sse, "_shortcuts_snapshot", lambda: None)


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
    async with asyncio.timeout(2):
        async for ev in gen:
            out.append(ev)
            if len(out) >= n:
                break
    return out


























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
    # So o esforco mudou -> sig DIFERENTE (a lista precisa re-emitir pro painel de orquestracao).
    assert sse._status_sig(a) != sse._status_sig(a.replace("(high✦)", "(medium✦)"))
    assert sse._status_sig(a)[4] == "high✦"
    c = a.replace("⚡5h:46%", "⚡5h:47%")
    assert sse._status_sig(a) != sse._status_sig(c)          # % mudou -> sig muda
    assert sse._status_sig(None) is None
    assert sse._status_sig("sem emojis") == (None, None, None, None, None)
