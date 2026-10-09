import pytest
import asyncio
import json
from app.sse import merged_events
from app.models import ChatEvent, SessionInfo, StateEvent
from app.adapters.preview_push import PushPreviewSource


class _StubModel:
    def model_dump(self):
        return {}


def test_lista_reemite_quando_a_ultima_resposta_muda():
    from app.sse import _list_sig

    before = SessionInfo(name="s", last_reply="primeira", last_reply_at=1)
    after = SessionInfo(name="s", last_reply="segunda", last_reply_at=2)

    assert _list_sig([before]) != _list_sig([after])


def test_lista_reemite_quando_o_modo_sem_terminal_muda():
    from app.sse import _list_sig

    terminal = SessionInfo(name="s", headless=False)
    headless = SessionInfo(name="s", headless=True)

    assert _list_sig([terminal]) != _list_sig([headless])


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


async def test_claude_emite_baixa_da_fila_ao_reabrir(monkeypatch, tmp_path):
    from app import pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _StubAdapterOne())
    queue = pqueue.PromptQueue("confirmada")
    entry = queue.append("pode continuar agr", delivered=True, ts=1)
    queue.reconcile_delivered({entry["text"]}, 0, 10, confirm_only=True)
    stream = merged_events("confirmada", str(tmp_path / "transcript.jsonl"))
    try:
        async with asyncio.timeout(3):
            async for event in stream:
                if event["event"] == "queue_confirmed":
                    payload = json.loads(event["data"])
                    assert payload["id"] == f"queued-{entry['id']}"
                    assert payload["queued_confirmed"] is True
                    break
    finally:
        await stream.aclose()


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
    # provider="codex" -> merged_events deve ramificar pro PushPreviewSource (push), NAO pro
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
    await PushPreviewSource.get(name).push("ok")  # simula delta ja acumulado pelo state_monitor
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


@pytest.mark.asyncio
async def test_clear_reinstalls_the_live_preview_getter_after_another_chat_closed(monkeypatch):
    """O broker de prévia usa o leitor de transcript da conexão mais recente. Fechada essa conexão,
    o leitor dela fica congelado no transcript anterior; no /clear da conexão viva a prévia achava
    que a sessão tinha trocado de dono e parava de vez. O reset reinstala o leitor de quem segue aberto."""
    from app.registry import session_key
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _AdapterPorProvider(provider))

    class _Info:
        name = "s1"
        jsonl = "/c/a.jsonl"
        provider = "claude"

    async def _lista():
        return [_Info()]

    monkeypatch.setattr("app.sse._cached_list", _lista)
    getters = []

    def _get(name, provider, stem_get=None):
        if stem_get is not None:
            getters.append(stem_get)
        return type("_S", (), {"subscribe": lambda self: _empty_agen(), "reset": lambda self: None})()

    monkeypatch.setattr("app.sse.PreviewBroker", type("_B", (), {"get": staticmethod(_get)}))
    live = merged_events("s1", "/c/a.jsonl", provider="claude")
    closed = merged_events("s1", "/c/a.jsonl", provider="claude")
    vistos = []

    async def _consumir():
        async for ev in live:
            vistos.append(ev["event"])
            if ev["event"] == "reset":
                return

    consumer = asyncio.create_task(_consumir())
    other = asyncio.create_task(anext(closed))
    await asyncio.sleep(0.2)
    other.cancel()
    await asyncio.gather(other, return_exceptions=True)
    await closed.aclose()
    assert len(getters) == 2
    _Info.jsonl = "/c/b.jsonl"
    await asyncio.wait_for(consumer, timeout=15)
    await live.aclose()
    assert getters[-1]() == session_key("/c/b.jsonl")



def test_list_sig_reemits_when_conversation_life_or_transfer_changes():
    from app.models import SessionInfo
    from app.sse import _list_sig
    original = SessionInfo(name="s", lifecycle_id="k:old")
    for fields in ({"lifecycle_id": "k:new"}, {"transfer_id": "operation"},
                   {"transfer_phase": "source_stopped"}, {"transfer_phase": "restore_failed"}):
        assert _list_sig([original]) != _list_sig([original.model_copy(update=fields)])


def test_list_sig_reemits_when_only_the_group_changes():
    from app.models import SessionInfo
    from app.sse import _list_sig
    original = SessionInfo(name="s", pair_peers=["a"], pair_gid="g", pair_task="T")
    for fields in ({"pair_peers": None}, {"pair_gid": "h"}, {"pair_task": "U"},
                   {"pair_external": {"alias": "x", "owner": "o", "session": "s"}}):
        assert _list_sig([original]) != _list_sig([original.model_copy(update=fields)])


class _AdapterMudo(_AdapterPorProvider):
    """Sessão parada: nem transcript nem transição de estado."""

    def transcript_stream(self, path, start_offset=None):
        return _empty_agen()


def _idle_session(monkeypatch, session):
    """Uma sessão Claude na lista, sem transcript, estado nem prévia chegando."""
    monkeypatch.setattr("app.sse.get_adapter", lambda provider: _AdapterMudo(provider))

    class _Info:
        name = session
        jsonl = "/claude/a.jsonl"
        provider = "claude"

    async def _lista():
        return [_Info()]

    monkeypatch.setattr("app.sse._cached_list", _lista)
    monkeypatch.setattr("app.sse.PreviewBroker", type("_B", (), {
        "get": staticmethod(lambda *a, **k: type("_S", (), {
            "subscribe": lambda self: _empty_agen(), "reset": lambda self: None})()),
    }))


@pytest.mark.asyncio
async def test_faixa_muda_com_a_sessao_parada_e_sai_no_stream(monkeypatch):
    # O monitor de estado não emite nada: na carona do `state`, a faixa nunca sairia.
    from app import plugin_bridge as pb
    _idle_session(monkeypatch, "faixa1")

    async def _muda():
        await asyncio.sleep(0.2)
        pb._guardar_faixa("faixa1", {"type": "Text", "children": ["review"]}, 80, [])

    async def _consumir():
        async for ev in merged_events("faixa1", "/claude/a.jsonl", provider="claude"):
            if ev["event"] == "plugin_ui":
                return json.loads(ev["data"])

    mudanca = asyncio.create_task(_muda())
    try:
        dado = await asyncio.wait_for(_consumir(), timeout=5)
        assert dado == {"above": {"type": "Text", "children": ["review"]}, "panes": []}
    finally:
        mudanca.cancel()
        pb.esquecer("faixa1")


@pytest.mark.asyncio
async def test_mod_toast_reaches_the_stream_with_the_session_idle(monkeypatch):
    # O aviso não entra no transcript nem muda o estado: sem fonte própria, nunca chegaria ao app.
    from app import plugin_bridge as pb
    _idle_session(monkeypatch, "aviso1")

    async def _consumir():
        async for ev in merged_events("aviso1", "/claude/a.jsonl", provider="claude"):
            if ev["event"] == "plugin_toast":
                return ev

    # Emitido ANTES de o app conectar e ainda dentro do prazo: quem abre a conversa agora o vê.
    pb._store_toast("aviso1", "Jenkins configurado.", 9000, "demo")
    try:
        ev = await asyncio.wait_for(_consumir(), timeout=5)
        dado = json.loads(ev["data"])
        assert (dado["text"], dado["plugin"]) == ("Jenkins configurado.", "demo")
        assert dado["id"] and 0 < dado["timeoutMs"] <= 9000
        assert "id" not in ev  # sem id de SSE: quem repõe na reconexão é a bomba
    finally:
        pb.esquecer("aviso1")


@pytest.mark.asyncio
async def test_guest_stream_never_gets_mod_toasts(monkeypatch):
    from app import plugin_bridge as pb
    _idle_session(monkeypatch, "aviso2")

    async def _consumir():
        async for ev in merged_events("aviso2", "/claude/a.jsonl", provider="claude",
                                      count_app=False):
            if ev["event"] == "plugin_toast":
                return ev

    pb._store_toast("aviso2", "token secreto", 9000, "demo")
    try:
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(_consumir(), timeout=1.5)
    finally:
        pb.esquecer("aviso2")
