import pytest
import asyncio
import json
from app.sse import merged_events
from app.models import ChatEvent, StateEvent
from app.adapters.codex.preview import CodexPreviewSource


class _StubModel:
    def model_dump(self):
        return {}


async def _empty_agen():
    return
    yield  # make it an async generator


async def _raising_agen():
    raise FileNotFoundError("simulated missing dir")
    yield  # make it an async generator


class _StubAdapterRaises:
    # merged_events pega o adapter via get_adapter(provider) — stub substitui o Adapter inteiro
    # (nao TranscriptTailer/StateMonitor direto, que sse.py nao referencia mais desde a introducao
    # do Adapter Protocol).
    provider = "claude"

    def transcript_stream(self, path, start_offset=None):
        return _raising_agen()

    def state_monitor(self, name, sid_get):
        return _empty_agen()

    async def drain(self, name, path):
        return 0


@pytest.mark.asyncio
async def test_pump_error_propagates(monkeypatch):
    """If a pump raises, merged_events must re-raise instead of hanging."""
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _StubAdapterRaises())

    with pytest.raises(FileNotFoundError):
        async for _ in merged_events("x", "y"):
            pass


async def _one_chat_event():
    yield ChatEvent(kind="user_msg", id="1", text="hi")  # tool_name etc. stay None


class _StubAdapterOne:
    provider = "claude"

    def transcript_stream(self, path, start_offset=None):
        return _one_chat_event()

    def state_monitor(self, name, sid_get):
        return _empty_agen()

    async def drain(self, name, path):
        return 0


@pytest.mark.asyncio
async def test_sse_data_is_json_string(monkeypatch):
    """SSE `data` must be a JSON string (browser does JSON.parse(e.data)); a raw dict
    gets str()'d into Python repr (None / single quotes) = invalid JSON."""
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _StubAdapterOne())

    async for ev in merged_events("cc", "j"):
        assert ev["event"] == "message"
        assert isinstance(ev["data"], str)
        parsed = json.loads(ev["data"])  # must not raise
        assert parsed["kind"] == "user_msg"
        assert parsed["tool_name"] is None      # serialized as JSON null
        assert "null" in ev["data"] and "None" not in ev["data"]
        break


async def _seq_states():
    # overlay aberto (nao-entregavel) -> idle (entregavel): a transicao dispara o drain UMA vez.
    yield StateEvent(session="cc", state="awaiting_input", overlay=True)
    yield StateEvent(session="cc", state="idle", overlay=False)
    yield StateEvent(session="cc", state="idle", overlay=False)   # repetido NAO redispara


class _StubAdapterSeq:
    provider = "claude"

    def __init__(self):
        self.drain_calls = []

    def transcript_stream(self, path, start_offset=None):
        return _one_chat_event()

    def state_monitor(self, name, sid_get):
        return _seq_states()

    async def drain(self, name, path):
        self.drain_calls.append((name, path))
        return 0


class _StubAdapterCodex:
    # provider="codex" -> merged_events deve ramificar pro CodexPreviewSource (push), NAO pro
    # PreviewBroker (poll de pane, que nem existe pro Codex).
    provider = "codex"

    def transcript_stream(self, path, start_offset=None):
        return _empty_agen()

    def state_monitor(self, name, sid_get):
        return _empty_agen()

    async def drain(self, name, path):
        return 0


@pytest.mark.asyncio
async def test_codex_provider_uses_codex_preview_source(monkeypatch):
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _StubAdapterCodex())
    name = "codex-sse-preview"
    await CodexPreviewSource.get(name).push("ok")  # simula delta ja acumulado pelo state_monitor
    async for ev in merged_events(name, "j", provider="codex"):
        if ev["event"] == "preview":
            assert json.loads(ev["data"])["text"] == "ok"
            break


@pytest.mark.asyncio
async def test_drain_fires_once_on_overlay_to_idle(monkeypatch):
    stub = _StubAdapterSeq()
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: stub)
    seen_idle = 0
    async for ev in merged_events("cc", "j"):
        if ev["event"] == "state" and json.loads(ev["data"])["state"] == "idle":
            seen_idle += 1
            if seen_idle >= 2:
                await asyncio.sleep(0.05)   # deixa o drain (task fire-and-forget) rodar
                break
    assert stub.drain_calls == [("cc", "j")]  # exatamente 1 drain, no jsonl corrente


# --- diagnostico do "medição indisponível" --------------------------------------------------

def test_context_pairs_conta_os_dois_pares():
    from app.sse import context_pairs
    # statusline REAL capturado do pane (Opus5, janela de 1M)
    sl = "🤖 Opus5 (high✦) │ 📁 hangar [main] │ 💬 156k/2 160k/1M"
    assert context_pairs(sl) == 2


def test_context_pairs_um_par_so_e_sem_metrica():
    # Pós-/clear (ou payload do Claude Code sem context_window): só o par in/out. Lê-lo como
    # contexto daria 100% falso -> o front mostra "medição indisponível" de propósito.
    assert __import__("app.sse", fromlist=["x"]).context_pairs("🤖 Opus5 │ 💬 156k/2") == 1


def test_context_pairs_sem_segmento():
    from app.sse import context_pairs
    assert context_pairs("🤖 Opus5 │ 💵 $4.61") == 0
    assert context_pairs(None) == 0


def test_context_pairs_nao_vaza_para_o_proximo_segmento():
    from app.sse import context_pairs
    # o '│' delimita: o par de outro segmento não pode contar como contexto.
    assert context_pairs("💬 156k/2 │ ⚡5h:11% ↺3h17m │ 📅7d:2/3") == 1


def test_context_pairs_par_rotulado_ctx_conta_como_metrica():
    from app.sse import context_pairs
    # Linha REAL da statusline do Kimi Code (~/.kimi-code/statusline.js, 2026-08-12): o stdin do
    # Kimi nao traz in/out do turno, entao o par de contexto vem rotulado e SOZINHO — sem contar
    # o rotulo, toda sessao Kimi/Pi caia no log de "sem métrica" com o contexto certo na tela.
    sl = "🤖 K3 (high✦) │ 📁 hangar [main] │ 💬 ctx 77k/1M │ ⚡5h:3% ↺50m │ 📅7d:33% │ 🕐 08:09 ⏱ 15h13m"
    assert context_pairs(sl) == 2


def test_status_sig_usa_o_par_rotulado_ctx():
    from app.sse import _status_sig
    # Sem o rotulo o sig so aceitava >=2 pares -> sessao Kimi/Pi tinha ctx=None e a lista do SSE
    # nao re-emitia quando so o contexto mudava de balde.
    sl = "🤖 K3 (high✦) │ 📁 hangar [main] │ 💬 ctx 500k/1M │ ⚡5h:3% │ 📅7d:33%"
    assert _status_sig(sl) == ("K3", 10, "3", "33", "high✦")  # 500k/1M = 50% -> balde 10 (20 baldes de 5%)


def test_status_sig_linha_do_claude_intacta():
    from app.sse import _status_sig
    # 2+ pares sem rotulo: o ULTIMO continua sendo o contexto (regra de sempre).
    assert _status_sig("🤖 Opus5 (high✦) │ 💬 156k/2 160k/1M │ ⚡5h:11% │ 📅7d:2%")[1] == 3


class _AdapterPorProvider:
    """Dois adapters de mentira: o do provider errado nunca emite bolha, o certo emite uma."""

    def __init__(self, provider):
        self.provider = provider

    def transcript_stream(self, path, start_offset=None):
        if self.provider == "claude":
            return _empty_agen()          # parser errado pro arquivo: nada sai
        return _one_chat_event()

    def state_monitor(self, name, sid_get, **kw):
        return _empty_agen()

    async def drain(self, name, path):
        return 0


@pytest.mark.asyncio
async def test_troca_de_provider_refaz_o_stream(monkeypatch):
    """Sessao Pi/Kimi recem-criada nasce classificada como "claude" (a extensao leva ~15s pra
    publicar o bilhete do pane). O provider era escolhido UMA vez, na abertura: o chat ficava mudo
    ate o usuario sair e voltar, porque so um stream novo pegava o adapter certo. Agora o watcher
    ve a troca, o stream se refaz e o front recebe `reset` pra reler o history pelo caminho certo.
    """
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _AdapterPorProvider(provider))

    class _Info:
        name = "s1"
        jsonl = "/pi/2026_a.jsonl"
        provider = "pi"

    async def _lista():
        return [_Info()]

    monkeypatch.setattr("app.sse._cached_list", _lista)
    monkeypatch.setattr("app.sse.PreviewBroker", type("_B", (), {
        "get": staticmethod(lambda *a, **k: type("_S", (), {
            "subscribe": lambda self: _empty_agen(), "reset": lambda self: None})()),
    }))
    vistos = []

    async def _consumir():
        async for ev in merged_events("s1", "/claude/a.jsonl", provider="claude"):
            vistos.append(ev["event"])
            if ev["event"] == "message":
                return

    # O watcher poda a cada 2s (nao e patchado: o mesmo sleep serve o ping_loop, e zerar ele aqui
    # vira loop quente). 15s de teto = folga larga pra uma troca que leva um poll.
    await asyncio.wait_for(_consumir(), timeout=15)
    # `reset` primeiro (o front tem que reler o history), a bolha do adapter certo depois.
    assert "reset" in vistos and vistos.index("reset") < vistos.index("message")
