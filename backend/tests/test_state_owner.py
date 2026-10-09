"""Troca de dono do estado (parte 4, Task 5): com o Rust de pé, o Python não observa Claude com
terminal em nenhuma porta. A conexão interna do hub fica sem estado, prévia, pergunta e sugestão;
quem entra pelo Python (convite 8766, Connect 8768) lê esses quatro do canal privado do hub."""
import asyncio
import json

import pytest

from app import list_bridge, plugin_bridge, pqueue, runtime_coordinator, sse
from app.models import ChatEvent

_SECRET = "ef" * 32
_RUST_EVENTS = [
    ("state", json.dumps({"session": "s", "state": "awaiting_input", "question": "Qual?"})),
    ("ask_question", json.dumps({"questions": []})),
    ("preview", json.dumps({"session": "s", "text": "em voo", "md": False, "full": False, "vivo": False})),
    ("suggest", json.dumps({"text": "roda os testes"})),
]


class _Dono:
    """Coordenador com o Rust dono do processo; nenhuma sessão aberta nele."""

    def __init__(self, mode, owns=(("claude", True), ("claude", False), ("codex", True))):
        self.mode = mode
        self.legacy = None
        self.owns = set(owns)

    def rust_owns(self, provider, headless):
        return (provider, headless) in self.owns

    def managed_queue(self, name):
        return False

    def managed_runtime(self, name):
        return False


class _Adapter:
    provider = "claude"

    def __init__(self):
        self.drains = []
        self.tails = []

    async def _transcript(self, path, start_offset=None):
        self.tails.append(path)
        yield ChatEvent(kind="assistant_msg", id="a1", text="resposta gravada")
        await asyncio.Event().wait()

    def transcript_stream(self, path, start_offset=None):
        return self._transcript(path, start_offset)

    def state_monitor(self, name, sid_get, **kw):
        raise AssertionError("StateMonitor do Python para sessão do Rust")

    async def drain(self, name, path):
        self.drains.append((name, path))
        return 0


def _sem_broker(*a, **kw):
    raise AssertionError("PreviewBroker do Python para sessão do Rust")


@pytest.fixture
def rust(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(sse, "_nav_arquivo", lambda: tmp_path / "nav.json")
    adapter = _Adapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    monkeypatch.setattr(sse.PreviewBroker, "get", _sem_broker)
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono("rust"))
    jsonl = tmp_path / "abc123.jsonl"
    jsonl.write_text("")
    return adapter, jsonl


class _CanalRust:
    """Porta privada do Rust de mentira: guarda os pedidos e responde o canal do estado."""

    def __init__(self, eventos=None):
        self.pedidos = []
        self.eventos = eventos or _RUST_EVENTS

    async def __aenter__(self):
        self.server = await asyncio.start_server(self._atende, "127.0.0.1", 0)
        port = self.server.sockets[0].getsockname()[1]
        list_bridge.configure(f"127.0.0.1:{port}", _SECRET)
        return self

    async def __aexit__(self, *exc):
        list_bridge.configure(None, None)
        self.server.close()

    async def _atende(self, reader, writer):
        cabeca = await reader.readuntil(b"\r\n\r\n")
        self.pedidos.append(cabeca.decode())
        writer.write(b"HTTP/1.0 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
        writer.write(b"event: ping\r\ndata: {}\r\n\r\n")
        for event, data in self.eventos:
            writer.write(f"event: {event}\r\ndata: {data}\r\n\r\n".encode())
        await writer.drain()
        await asyncio.Event().wait()


async def _coleta(gen, ate, limite=5.0):
    vistos = []
    try:
        async with asyncio.timeout(limite):
            async for ev in gen:
                vistos.append(ev)
                if ate(vistos):
                    return vistos
    finally:
        await gen.aclose()
    return vistos


def _nomes(vistos):
    return [e["event"] for e in vistos]


@pytest.mark.parametrize("porta", ["interno", "convidado", "connect"])
@pytest.mark.parametrize("modo", ["rust", "pending"])
async def test_no_python_state_monitor_for_rust_terminal(rust, monkeypatch, porta, modo):
    adapter, jsonl = rust
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono(modo))
    side, count_app = {"interno": (True, True), "convidado": (False, False), "connect": (False, True)}[porta]
    async with _CanalRust():
        gen = sse.merged_events("s", str(jsonl), side=side, count_app=count_app)
        vistos = await _coleta(gen, lambda v: "ping" in _nomes(v) and (side or "suggest" in _nomes(v)))
    # O adapter e o broker levantariam: chegar aqui já prova que nada do estado subiu no Python.
    assert "ping" in _nomes(vistos)
    await asyncio.sleep(0.05)
    assert adapter.drains == [], "a entrega é pedida pelo Rust (session.deliverable), não pelo SSE"
    if side:
        assert not {"state", "preview", "ask_question", "suggest"} & set(_nomes(vistos))
        assert adapter.tails == [], "o tail_pump da conexão interna só servia à prévia do Python"


async def test_python_mode_still_runs_python_state(rust, monkeypatch):
    adapter, jsonl = rust
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono("python"))
    gen = sse.merged_events("s", str(jsonl), side=True)
    with pytest.raises(AssertionError, match="StateMonitor|PreviewBroker"):
        await _coleta(gen, lambda v: False, limite=2.0)


async def test_guest_chat_reads_rust_channel(rust):
    adapter, jsonl = rust
    async with _CanalRust() as canal:
        gen = sse.merged_events("sessão x", str(jsonl), count_app=False)
        vistos = await _coleta(gen, lambda v: "suggest" in _nomes(v))
    do_rust = [(e["event"], e["data"]) for e in vistos if e["event"] in {"state", "preview", "ask_question", "suggest"}]
    assert do_rust == _RUST_EVENTS, "os quatro passam como o hub os publicou, na ordem"
    pedido = canal.pedidos[0]
    assert pedido.startswith("GET /__hangar_server/state/sess%C3%A3o%20x/events HTTP/1.0\r\n")
    assert f"x-hangar-internal: {_SECRET}" in pedido
    assert "message" in _nomes(vistos), "o transcript do convidado continua vindo do Python"


async def test_guest_channel_failure_closes_the_stream(rust):
    _adapter, jsonl = rust
    list_bridge.configure(None, None)
    gen = sse.merged_events("s", str(jsonl), count_app=False)
    with pytest.raises(sse.RustStateChannelError) as exc:
        await _coleta(gen, lambda v: False)
    assert exc.value.code == "state_channel_off"


async def test_side_events_keeps_info_queue_nav_ping_toast(rust):
    adapter, jsonl = rust
    entry = pqueue.PromptQueue("s").append("manda isso depois", delivered=False)
    sse.nav_pendente("s", "http://localhost:5173")
    plugin_bridge._store_toast("s", "Jenkins configurado.", 9000, "demo")
    gen = sse.merged_events("s", str(jsonl), side=True)
    quer = {"info", "message", "nav", "ping", "plugin_toast"}
    vistos = await _coleta(gen, lambda v: quer <= set(_nomes(v)))
    assert quer <= set(_nomes(vistos))
    assert vistos[0]["event"] == "info"
    msg = next(json.loads(e["data"]) for e in vistos if e["event"] == "message")
    assert msg["id"] == f"queued-{entry['id']}"
    assert not {"state", "preview", "ask_question", "suggest"} & set(_nomes(vistos))
    assert adapter.drains == [] and adapter.tails == []
    sse.nav_confirmar("s")


# --- Codex sem terminal: o feed do hub do Rust é dono dos seis eventos (5B Task 9) ---

_SEIS = {"state", "preview", "ask_question", "suggest", "pensamento", "ferramenta"}
_CODEX_EVENTS = [
    ("state", json.dumps({"session": "cx", "state": "working", "headless": True})),
    ("ask_question", "null"),
    ("preview", json.dumps({"session": "cx", "text": "em voo", "md": True, "full": True, "vivo": True})),
    ("pensamento", json.dumps({"text": "pensando"})),
    ("ferramenta", json.dumps({"text": ""})),
]


class _CodexAdapter(_Adapter):
    provider = "codex"

    async def _transcript(self, path, start_offset=None):
        self.tails.append(path)
        yield ChatEvent(kind="user_msg", id="u1", text="oi")
        yield ChatEvent(kind="assistant_msg", id="a1", text="resposta gravada")
        await asyncio.Event().wait()


@pytest.fixture
def codex(tmp_path, monkeypatch):
    from app.adapters.codex import sessions as codex_sessions
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(sse, "_nav_arquivo", lambda: tmp_path / "nav.json")
    adapter = _CodexAdapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    monkeypatch.setattr(sse.PreviewBroker, "get", _sem_broker)
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono("rust"))
    sidecar = {"headless": True}
    monkeypatch.setattr(codex_sessions, "load", lambda name: sidecar)
    confirmados = []
    monkeypatch.setattr(sse, "_confirm_codex_queue", lambda name, jsonl: confirmados.append((name, jsonl)))
    jsonl = tmp_path / "rollout-abc.jsonl"
    jsonl.write_text("")
    return adapter, jsonl, sidecar, confirmados


@pytest.mark.parametrize("modo,sidecar,owns,rust", [
    ("rust", {"headless": True}, True, True), ("pending", {"headless": True}, True, True),
    ("python", {"headless": True}, True, False), ("rust", {"headless": False}, True, False),
    ("rust", None, True, False), ("rust", {"headless": True}, False, False)])
def test_codex_state_is_rust_only_headless_and_owned(monkeypatch, modo, sidecar, owns, rust):
    from app.adapters.codex import sessions as codex_sessions
    monkeypatch.setattr(codex_sessions, "load", lambda name: sidecar)
    dono = (("claude", True), ("claude", False)) + ((("codex", True),) if owns else ())
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono(modo, dono))
    assert sse._estado_do_rust("codex", "cx") is rust
    assert sse._estado_do_rust("claude", "cx") is (modo != "python")


async def _por(gen, segundos):
    """Tudo o que o stream manda em `segundos` (o ping seguinte só sai em 10 s)."""
    vistos = []
    try:
        async with asyncio.timeout(segundos):
            async for ev in gen:
                vistos.append(ev)
    except TimeoutError:
        pass
    finally:
        await gen.aclose()
    return vistos


async def test_codex_headless_internal_connection_sends_none_of_the_six(codex, monkeypatch):
    adapter, jsonl, _sidecar, confirmados = codex
    vistos = await _por(sse.merged_events("cx", str(jsonl), provider="codex", side=True), 1.5)
    assert vistos[0]["event"] == "info" and json.loads(vistos[0]["data"])["headless"] is True
    assert not _SEIS & set(_nomes(vistos)), _nomes(vistos)
    assert adapter.tails == [str(jsonl)], "o tail_pump fica: é ele que confirma a fila do Codex"
    # Abertura confirma uma vez; o `user_msg` do transcript, outra.
    assert len(confirmados) >= 2
    assert adapter.drains == [], "a fila do Codex o ator drena sozinho"


async def test_codex_headless_guest_reads_six_from_rust_channel(codex):
    _adapter, jsonl, _sidecar, _ = codex
    async with _CanalRust(_CODEX_EVENTS):
        gen = sse.merged_events("cx", str(jsonl), provider="codex", count_app=False)
        vistos = await _coleta(gen, lambda v: "ferramenta" in _nomes(v))
    do_rust = [(e["event"], e["data"]) for e in vistos if e["event"] in _SEIS]
    assert do_rust == _CODEX_EVENTS, "os seis passam como o hub os publicou, sem cópia do Python"


async def test_codex_headless_flip_reprovides_and_sends_new_info(codex, monkeypatch):
    from app.models import SessionInfo
    _adapter, jsonl, sidecar, _ = codex
    viva = {"headless": True}

    async def lista():
        return [SessionInfo(name="cx", jsonl=str(jsonl), provider="codex", headless=viva["headless"])]

    monkeypatch.setattr(sse, "_cached_list", lista)
    gen = sse.merged_events("cx", str(jsonl), provider="codex", side=True)

    async def vira():
        await asyncio.sleep(0.3)
        sidecar["headless"] = viva["headless"] = False

    task = asyncio.create_task(vira())
    vistos = await _coleta(gen, lambda v: sum(e["event"] == "info" for e in v) >= 2, limite=6.0)
    await task
    infos = [json.loads(e["data"]) for e in vistos if e["event"] == "info"]
    assert [i["headless"] for i in infos[:2]] == [True, False], infos


@pytest.mark.parametrize("count_app", [False, True])
async def test_nav_marker_only_reaches_owner_connections(rust, count_app):
    # A url do rascunho de página leva o token do dono: convidado e par externo (count_app=False)
    # nunca recebem o marcador; o dono pela porta do Connect recebe.
    _adapter, jsonl = rust
    sse.nav_pendente("s", "http://127.0.0.1:8765/api/sessions/s/pages/abc?token=segredo")
    vistos = []
    async with _CanalRust():
        gen = sse.merged_events("s", str(jsonl), count_app=count_app)
        try:
            async with asyncio.timeout(2.5):
                async for ev in gen:
                    vistos.append(ev)
                    if ev["event"] == "nav":
                        break
        except TimeoutError:
            pass
        finally:
            await gen.aclose()
    sse.nav_confirmar("s")
    assert ("nav" in _nomes(vistos)) is count_app
    if not count_app:
        assert not any("segredo" in str(e.get("data", "")) for e in vistos)


# --- Claude sem terminal: o feed do hub é dono dos seis, sugestão incluída ---

_SEIS = {"state", "preview", "ask_question", "pensamento", "ferramenta", "suggest"}
_HEADLESS_EVENTS = [
    ("ask_question", "null"),
    ("state", json.dumps({"session": "h", "state": "working", "headless": True})),
    ("preview", json.dumps({"session": "h", "text": "em voo", "md": True, "full": True, "vivo": True})),
    ("pensamento", json.dumps({"text": "pensando"})),
    ("ferramenta", json.dumps({"text": ""})),
    ("suggest", json.dumps({"text": "do rust"})),
]


@pytest.fixture
def headless(tmp_path, monkeypatch):
    from app import adapters
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(sse, "_nav_arquivo", lambda: tmp_path / "nav.json")
    monkeypatch.setattr(adapters.headless_sessions, "exists", lambda name: True)
    adapter = _Adapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    monkeypatch.setattr(sse.PreviewBroker, "get", _sem_broker)
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono("rust"))
    monkeypatch.setitem(plugin_bridge._sugestoes, "h", "roda os testes")
    jsonl = tmp_path / "sid.jsonl"
    jsonl.write_text("")
    return adapter, jsonl


@pytest.mark.parametrize("modo,owns,rust", [
    ("rust", True, True), ("pending", True, True), ("python", True, False), ("rust", False, False)])
def test_claude_headless_state_is_rust_when_owned(monkeypatch, modo, owns, rust):
    dono = (("claude", False), ("codex", True)) + ((("claude", True),) if owns else ())
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono(modo, dono))
    assert sse._estado_do_rust(sse.CLAUDE_HEADLESS, "h") is rust


async def test_claude_headless_internal_connection_sends_none_of_the_six(headless):
    adapter, jsonl = headless
    vistos = await _por(sse.merged_events("h", str(jsonl), provider="claude", side=True), 1.5)
    assert vistos[0]["event"] == "info" and json.loads(vistos[0]["data"])["provider"] == "claude-headless"
    assert not _SEIS & set(_nomes(vistos)), _nomes(vistos)
    assert adapter.drains == [], "a fila do Claude sem terminal o ator drena sozinho"
    assert adapter.tails == [], "a prévia gravada é suprimida no hub"


async def test_claude_headless_python_mode_runs_python_state(headless, monkeypatch):
    _adapter, jsonl = headless
    monkeypatch.setattr(runtime_coordinator, "_current", _Dono("python"))
    with pytest.raises(AssertionError, match="StateMonitor"):
        await _coleta(sse.merged_events("h", str(jsonl), provider="claude", side=True), lambda v: False, limite=2.0)


async def test_claude_headless_guest_reads_six_from_rust_channel(headless):
    _adapter, jsonl = headless
    async with _CanalRust(_HEADLESS_EVENTS):
        gen = sse.merged_events("h", str(jsonl), provider="claude", count_app=False)
        vistos = await _por(gen, 1.5)
    assert [(e["event"], e["data"]) for e in vistos if e["event"] in _SEIS] == _HEADLESS_EVENTS
