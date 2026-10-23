import asyncio
from types import SimpleNamespace

import pytest

from app import api, runtime_coordinator, sse


class Owner:
    instance = "runtime"
    # Sem hangar-server de pé: nenhuma ponte do Rust entra no caminho (Codex com terminal, pergunta do Claude).
    mode = "python"

    def __init__(self, disposition="accepted", terminal=True):
        self.calls = []
        self.disposition = disposition
        self.loop = None
        self.terminal = terminal

    def slot(self, name):
        return SimpleNamespace(binding=SimpleNamespace(meta={"terminal": {}} if self.terminal else {}))

    def managed_runtime(self, name):
        return name == "session"

    def managed_queue(self, name):
        return self.managed_runtime(name)

    async def prepare_session(self, name, provider, *, launch=False, engine_models=None):
        return self.managed_runtime(name)

    async def op(self, name, command, operation_id):
        self.calls.append((command, operation_id))
        if command["kind"] == "drain":
            return {"sent":1}
        if command["kind"] == "confirm":
            return {"confirmed":1}
        return {"operation_id":operation_id, "disposition":self.disposition, "payload":{}}


@pytest.mark.parametrize("provider,path", [
    ("claude", "input"), ("codex", "input"), ("claude", "group"),
    ("claude", "pair"), ("codex", "peer"), ("claude", "guest"),
])
def test_all_send_producers_use_one_operation(monkeypatch, provider, path):
    owner = Owner()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    monkeypatch.setattr(api, "_provider_of", lambda name: provider)
    monkeypatch.setattr(api, "_headless", lambda name: provider == "claude")
    monkeypatch.setattr(api, "_session_exists", lambda name: True)
    monkeypatch.setattr(api, "_enviar_nativo", lambda *args: pytest.fail("UDS antes do diário"))
    monkeypatch.setattr(api.PromptQueue, "append", lambda *args, **kwargs: pytest.fail("append fora do ator"))
    async def scenario():
        result = await api._enviar("session", "[de: sender] Olá" if path != "input" else "Olá")
        assert result["ok"] and result["delivered"]
    asyncio.run(scenario())
    assert len(owner.calls) == 1
    assert owner.calls[0][0]["kind"] == "submit"
    assert owner.calls[0][1]


def test_unknown_send_waits_in_queue_without_error_or_retry(monkeypatch):
    # Entrega sem prova não é erro na tela (o dono reenviaria e duplicaria): a fila confirma pelo
    # transcript, uma vez.
    owner = Owner("unknown")
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    result = asyncio.run(api._send_managed("session", "Olá", "claude", track_entry=True))
    assert result["ok"] and not result["delivered"] and result["uncertain"] and result["error"] is None
    assert result["entry_id"] == owner.calls[0][1]
    assert len(owner.calls) == 1


@pytest.mark.parametrize("text,terminal", [("/model", True), ("Olá", False)])
def test_unknown_without_transcript_proof_stays_an_error(monkeypatch, text, terminal):
    # Comando `/` não tem linha na fila; sessão sem terminal não reconcilia pelo transcript.
    owner = Owner("unknown", terminal=terminal)
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    result = asyncio.run(api._send_managed("session", text, "claude"))
    assert not result["ok"] and len(owner.calls) == 1


@pytest.mark.parametrize("text,terminal,ok", [("/clear", True, False), ("Olá", True, True), ("/clear", False, True)])
def test_deferred_slash_on_terminal_is_an_error_not_queued(monkeypatch, text, terminal, ok):
    # Item 16: o /clear adiado (pergunta ainda aberta) não tem linha na fila e nunca roda depois; o
    # 200 "na fila" o fazia sumir calado.
    owner = Owner("deferred", terminal=terminal)
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    result = asyncio.run(api._send_managed("session", text, "claude"))
    assert result["ok"] is ok and len(owner.calls) == 1
    if not ok:
        assert result["error"]["code"] == "erro_comando_nao_executado" and result["error"]["params"]["comando"] == "/clear"


def test_send_without_rust_answer_is_uncertain_not_failed(monkeypatch):
    owner = Owner("unknown")
    async def lost(name, command, operation_id):
        owner.calls.append((command, operation_id))
        return {"operation_id":operation_id, "disposition":"unknown", "payload":{"transport_lost":True}}
    owner.op = lost
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    result = asyncio.run(api._send_managed("session", "Olá", "claude", track_entry=True))
    assert result["ok"] and not result["delivered"] and result["uncertain"]
    assert result["error"] is None and result["entry_id"] == owner.calls[0][1]


@pytest.mark.parametrize("source", ["hook", "drain_session", "turn_end", "codex_confirm"])
def test_background_producers_reach_owner(monkeypatch, source):
    owner = Owner()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: SimpleNamespace(jsonl="chat", provider="claude"))
    monkeypatch.setattr(api.PromptQueue, "load", lambda *args: pytest.fail("confirmação por texto antigo"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        if source == "codex_confirm":
            await asyncio.to_thread(sse._confirm_codex_queue, "session", "chat")
        elif source == "turn_end":
            await asyncio.to_thread(api._confirm_and_drain, "session")
        elif source == "drain_session":
            await asyncio.to_thread(api._drain_session, "session")
        else:
            await asyncio.to_thread(api._drenar, "session", "chat", "claude")
    asyncio.run(scenario())
    assert [call[0]["kind"] for call in owner.calls] == (["confirm", "drain"] if source == "turn_end" else ["confirm"] if source == "codex_confirm" else ["drain"])


def test_ownership_moving_never_breaks_stream_worker_or_route(monkeypatch):
    # Sessão recém-criada passando ao Rust: o SSE e o drain de fundo seguem, a rota responde 409.
    import json
    owner = Owner()
    async def moving(name, command, operation_id):
        raise runtime_coordinator.TransferInProgress("sessão em transferência; aguarde a posse ser confirmada")
    owner.op = moving
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        assert await asyncio.to_thread(sse._confirm_codex_queue, "session", "chat") is None
        assert await asyncio.to_thread(api._drenar, "session", "chat", "codex") == 0
    asyncio.run(scenario())
    response = asyncio.run(api._ownership_moving(None, runtime_coordinator.TransferInProgress("x")))
    assert response.status_code == 409
    assert json.loads(response.body)["detail"]["code"] == "session_transfer_busy"


def test_terminal_and_other_providers_keep_legacy_route(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "_current", Owner())
    monkeypatch.setattr(api, "_headless", lambda name: False)
    monkeypatch.setattr(api, "drain", lambda *args: 3)
    assert api._drenar("terminal", "chat", "pi") == 3


def test_native_receipt_updates_registered_operation_only(tmp_path, monkeypatch):
    import json
    import uuid
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    owner = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    slot = owner.register(Binding("session", "key", "claude", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    slot.store.exec(1, "prepare", {"epoch_s":1, "monotonic_s":1},
        {"kind":"prepare", "id":"entry", "entry_id":None, "payload":{"kind":"input"}})
    calls = []
    async def op(name, command, operation_id):
        calls.append(command)
    owner.op = op
    mid = str(uuid.uuid5(uuid.NAMESPACE_URL, "hangar:key:entry"))
    try:
        assert asyncio.run(owner.native_receipt(mid, "rejected"))
        assert calls[0]["action"]["id"] == "entry"
        assert calls[0]["action"]["status"] == "rejected"
    finally:
        owner.close_python_leases()



def test_native_receipt_with_rust_up_leaves_the_state_file_alone(tmp_path, monkeypatch):
    # Com o Rust de pé o recibo volta à caixa dele: o Python não lê o arquivo que o Rust troca.
    import uuid
    from pathlib import Path
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    owner = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    slot = owner.register(Binding("session", "key", "claude", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    slot.store.exec(1, "prepare", {"epoch_s":1, "monotonic_s":1},
        {"kind":"prepare", "id":"entry", "entry_id":None, "payload":{"kind":"input"}})
    opened = []
    real = Path.read_bytes
    monkeypatch.setattr(Path, "read_bytes", lambda path: opened.append(path) or real(path))
    owner.mode = "rust"
    mid = str(uuid.uuid5(uuid.NAMESPACE_URL, "hangar:key:entry"))
    try:
        assert asyncio.run(owner.native_receipt(mid, "rejected")) is False
        assert tmp_path / "state" not in opened
    finally:
        owner.close_python_leases()

# --- Sessão sem terminal nasce direto no Rust (dono único, Task 2) ---

SID = "11111111-1111-1111-1111-111111111111"


class Transport:
    """Rust falso: conta as operações; `open` pode demorar ou recusar com um código."""
    instance = "runtime"

    def __init__(self, fail=None, delay=0.0, view=None):
        self.ops, self.commands, self.fail, self.delay, self.view = [], [], fail, delay, view or {}

    async def op(self, descriptor, command, operation_id, clock):
        from app.rust_server import RustOpError
        if command["kind"] == "ingress":
            # A porta de escrita é conferida em test_runtime_ingress; aqui ela não entra na sequência.
            return {"closed": command["closed"]}
        self.ops.append((command["kind"], descriptor))
        self.commands.append(command)
        if command["kind"] == "open":
            await asyncio.sleep(self.delay)
            if self.fail:
                raise RustOpError(f"IPC recusou a operação (503: {self.fail})", 503, self.fail)
            view = {"alive":True, "initialized":False, "iniciando":True,
                    "public_state":{"session":descriptor["name"], "state":"working", "headless":True}, **self.view}
            return {"opened":True, "instance":self.instance, "key":descriptor["key"], "generation":descriptor["generation"],
                    "state":{"key":descriptor["key"], "generation":descriptor["generation"], "revision":1,
                             "view":view, "channels":{}, "error":None}}
        if command["kind"] == "submit":
            return {"operation_id":operation_id, "disposition":"deferred", "payload":{}}
        if command["kind"] == "drain":
            return {"sent":0}
        return {}

    def kinds(self):
        return [kind for kind, _ in self.ops]


@pytest.fixture
def birth(tmp_path, monkeypatch):
    from app import pqueue
    from app.adapters.claude_headless import adapter as A, sessions as S
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
    from app.runtime_adapter import LegacyBridge
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    from app.adapters.codex import sessions as codex_sessions
    monkeypatch.setattr(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions")
    monkeypatch.setattr(A.log_paths, "base", lambda: tmp_path / "logs")
    monkeypatch.setattr(A, "_dir_marcadores", lambda meta: tmp_path / "state")
    monkeypatch.setattr(A, "_esforco_padrao", lambda config_dir: None)
    monkeypatch.setattr(A, "_marca_config", lambda config_dir: "marca")
    S.save("s1", str(tmp_path), SID, model="haiku", permission_mode="manual")
    state = SimpleNamespace(kills=[])
    monkeypatch.setattr(A, "_matar_grupo", lambda pid, name: state.kills.append(pid))
    async def no_client(*args, **kwargs):
        pytest.fail("cliente Python aberto no cano de uma sessão migrada")
    monkeypatch.setattr(A, "conectar_cano", no_client)
    monkeypatch.setattr(ClaudeHeadlessAdapter, "_conectar", no_client)
    adapter = ClaudeHeadlessAdapter()
    def build(transport):
        owner = runtime_coordinator.RuntimeCoordinator(transport)
        owner.legacy = LegacyBridge(owner, {"claude":adapter})
        monkeypatch.setattr(runtime_coordinator, "_current", owner)
        return owner
    state.adapter, state.build, state.sessions = adapter, build, S
    yield state
    current = runtime_coordinator._current
    if current is not None:
        current.close_python_leases()


def test_new_headless_session_never_opens_python_client(birth):
    transport = Transport()
    owner = birth.build(transport)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        birth.adapter.acordar("s1")
        for _ in range(200):
            if transport.ops:
                break
            await asyncio.sleep(0.01)
        await asyncio.sleep(0.05)
    asyncio.run(scenario())
    assert transport.kinds() == ["open"]
    command = transport.commands[0]
    assert command["launch"] is True, "quem sobe o processo é o Rust"
    assert command["descriptor"]["sidecar_dir"] == str(birth.sessions._dir())
    slot = owner.slot("s1")
    assert slot.phase == runtime_coordinator.Phase.Rust and slot.store is None and slot.lease is None


def test_first_message_right_after_create_is_delivered_once(birth, monkeypatch):
    transport = Transport(delay=0.3)
    owner = birth.build(transport)
    monkeypatch.setattr(api, "_enviar_nativo", lambda *args: pytest.fail("socket nativo direto"))
    monkeypatch.setattr(api.PromptQueue, "append", lambda *args, **kwargs: pytest.fail("fila Python"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        birth.adapter.acordar("s1")
        await asyncio.sleep(0.1)
        return await api._send_one_headless("s1", "Olá")
    result = asyncio.run(scenario())
    assert result["ok"] and not result["delivered"]
    assert transport.kinds() == ["open", "submit"], "abre uma vez; o Rust entrega depois do initialize"
    assert transport.commands[0]["launch"] is True


def test_migrated_send_never_falls_to_legacy_path(birth, monkeypatch):
    owner = birth.build(Transport(fail="queue_io"))
    monkeypatch.setattr(api, "_enviar_nativo", lambda *args: pytest.fail("socket nativo direto"))
    monkeypatch.setattr(api.PromptQueue, "append", lambda *args, **kwargs: pytest.fail("fila Python"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        return await api._send_one_headless("s1", "Olá")
    result = asyncio.run(scenario())
    assert not result["ok"]
    assert "queue_io" in str(result["error"])
    assert birth.kills == [], "recusa que não é de conexão não mata o cano"


def test_wait_initialized_reports_cli_refusal(birth):
    view = {"iniciando":False, "public_state":{"session":"s1", "state":"idle", "headless":True,
            "problema":"headless_nao_subiu", "problema_detalhe":"initialize recusado: conta sem acesso"}}
    owner = birth.build(Transport(view=view))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1", wait_initialized=True)
    with pytest.raises(RuntimeError, match="conta sem acesso"):
        asyncio.run(scenario())


def test_reads_and_stop_never_launch_a_process(birth):
    transport = Transport()
    owner = birth.build(transport)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        assert await birth.adapter.deliverable("s1") is False
        await birth.adapter.parar("s1")
    asyncio.run(scenario())
    assert transport.kinds() == []


def test_python_registration_without_cano_moves_to_rust_on_send(birth, monkeypatch):
    transport = Transport()
    owner = birth.build(transport)
    monkeypatch.setattr(api, "_enviar_nativo", lambda *args: pytest.fail("socket nativo direto"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await birth.adapter.deliverable("s1")      # leitura registra no Python, sem processo
        assert owner.slot("s1").phase == runtime_coordinator.Phase.Python
        return await api._send_one_headless("s1", "Olá")
    result = asyncio.run(scenario())
    assert result["ok"]
    slot = owner.slot("s1")
    assert slot.phase == runtime_coordinator.Phase.Rust and slot.lease is None
    assert transport.kinds() == ["open", "submit"]



# --- Administração da sessão sem terminal por close/open (dono único, Task 4) ---

class LockingTransport(Transport):
    """Rust falso que segura a trava da fila enquanto a sessão está aberta, como o real."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.lease, self.revision = None, 1

    async def op(self, descriptor, command, operation_id, clock):
        from app.runtime_coordinator import WriterLease
        kind = command["kind"]
        if kind not in {"close", "snapshot"}:
            result = await super().op(descriptor, command, operation_id, clock)
            if kind == "open":
                self.lease = WriterLease(descriptor["lock_path"])
            return result
        self.ops.append((kind, descriptor))
        if kind == "close":
            if self.lease is not None:
                self.lease.close()
                self.lease = None
            return {"closed":True}
        self.revision += 1
        view = {"alive":True, "initialized":True, "iniciando":False,
                "public_state":{"session":descriptor["name"], "state":"idle", "headless":True}, **self.view}
        return {"key":descriptor["key"], "generation":descriptor["generation"], "revision":self.revision,
                "view":view, "channels":{}, "error":None}


def _alive_cano(birth):
    import os
    birth.sessions.update("s1", cano={"pid":os.getpid(), "escuta":"unix:/tmp/vivo.sock", "token":"t", "ts":1.0, "versao":2})
    return os.getpid()


def _opened(birth, **view):
    pid = _alive_cano(birth)
    transport = LockingTransport(view=view)
    return birth.build(transport), transport, pid


def test_kill_closes_in_rust_and_stops_cano(birth):
    owner, transport, pid = _opened(birth)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        async def kill():
            meta = birth.sessions.load("s1")
            birth.sessions.delete("s1")
            await asyncio.to_thread(birth.adapter.close_sync, "s1", meta)
        await owner.change("s1", kill, remove=True)
    asyncio.run(scenario())
    assert transport.kinds() == ["open", "snapshot", "close"]
    assert birth.kills == [pid] and not owner.managed_queue("s1")
    assert transport.lease is None


def test_mode_switch_to_terminal_closes_rust_first(birth, monkeypatch):
    from app import runtime_terminal
    owner, transport, pid = _opened(birth)
    monkeypatch.setattr(runtime_terminal, "_collect", lambda name: None)
    monkeypatch.setattr(runtime_terminal, "_session_proof", lambda name: "pane-novo")
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        async def to_terminal():
            assert transport.kinds()[-1] == "close" and transport.lease is None, "o Rust soltou antes da ação"
            meta = birth.sessions.load("s1")
            birth.sessions.delete("s1")
            await asyncio.to_thread(birth.adapter.close_sync, "s1", meta)
        await owner.change("s1", to_terminal)
    asyncio.run(scenario())
    slot = owner.slot("s1")
    assert transport.kinds() == ["open", "snapshot", "close"], "vínculo pendente não abre no Rust"
    assert slot.binding.meta["pending_terminal"] == "pane-novo" and not slot.binding.headless
    assert birth.kills == [pid]


def test_failed_headless_claude_is_reopened_by_the_rust_not_by_the_python(birth):
    transport = Transport()
    owner = birth.build(transport)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        slot = await owner.ensure_open("s1")
        slot.view = {**slot.view, "error":"cano_exited"}
        await owner.op("s1", {"kind":"submit", "text":"oi", "steer":False, "pre_transcript":False}, "op-1")
    asyncio.run(scenario())
    assert transport.kinds() == ["open", "submit"]


def test_reload_reopens_in_rust(birth):
    owner, transport, pid = _opened(birth)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        await birth.adapter.recarregar("s1")
    asyncio.run(scenario())
    assert transport.kinds() == ["open", "snapshot", "close", "open"]
    assert birth.kills == [pid], "mata pelo sidecar"
    assert transport.commands[-1]["launch"] is True and transport.ops[3][1]["generation"] == 2, "o Rust relança"
    slot = owner.slot("s1")
    assert slot.phase == runtime_coordinator.Phase.Rust and slot.lease is None


def test_account_switch_reopens_and_waits_initialize(birth):
    owner, transport, pid = _opened(birth)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        transport.view = {"iniciando":False, "public_state":{"session":"s1", "state":"idle", "headless":True,
            "problema":"headless_nao_subiu", "problema_detalhe":"initialize recusado: conta sem acesso"}}
        async def move():
            await birth.adapter.parar("s1")
            await asyncio.to_thread(birth.sessions.update, "s1", config_dir="/outra-conta")
            await birth.adapter.ensure_running("s1", require_initialize=True)
        await owner.change("s1", move)
    with pytest.raises(RuntimeError, match="conta sem acesso"):
        asyncio.run(scenario())
    assert transport.kinds() == ["open", "snapshot", "close", "open"]
    assert birth.kills == [pid] and transport.commands[-1]["launch"] is True
    assert owner.slot("s1").phase == runtime_coordinator.Phase.Rust, "a recusa fica visível no Rust"


def test_switch_to_headless_opens_in_rust(birth, monkeypatch):
    # Regressão: terminal no Rust → sidecar sem terminal → abre sem terminal no Rust (a adoção de cano não existe mais).
    from app import runtime_terminal
    from app.runtime_coordinator import Binding
    owner = birth.build(LockingTransport())
    transport = owner.transport
    from app import pqueue
    directory, key = pqueue._queue_dir(), birth.sessions.load("s1")["key"]
    terminal = Binding("t1", key, "claude", False,
        {"key":key, "session_id":SID, "fingerprint":"vida",
         "terminal":{"name":"t1", "pane":"%1", "conversation":SID, "generation":1, "created":1,
                     "mux_argv":["tmux"], "windows":False, "clipboard_lock_path":None}},
        "", directory, directory / "runtime" / f"{key}.json", directory / "runtime" / f"{key}.lock", 1)
    monkeypatch.setattr(runtime_terminal, "_session_proof", lambda name: None)
    meta = birth.sessions.load("s1")
    birth.sessions.delete("s1")
    pane = {"binding":terminal}
    monkeypatch.setattr(runtime_terminal, "resolve_binding", lambda name, previous=None: pane["binding"])
    monkeypatch.setattr(runtime_terminal, "validate_binding", lambda descriptor: pane["binding"])
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session("t1", "claude")
        assert owner.slot("t1").phase == runtime_coordinator.Phase.Rust
        async def to_headless():
            pane["binding"] = None
            birth.sessions.save("t1", meta["cwd"], SID, model="haiku", permission_mode="manual")
            birth.sessions.update("t1", key=terminal.key)
        await owner.change("t1", to_headless)
        await birth.adapter.ensure_running("t1", esperar_pronta=False)
    asyncio.run(scenario())
    assert transport.kinds() == ["open", "snapshot", "close", "open"]
    assert transport.ops[3][1]["headless"] is True and transport.commands[-1]["launch"] is True
    assert owner.slot("t1").phase == runtime_coordinator.Phase.Rust


def test_transfer_source_idle_reads_runtime_view(birth):
    from app.conversation_transfer import TransferError, _check_source_idle
    owner, transport, _ = _opened(birth, in_progress=True)
    meta = {"headless":True, "jsonl":str(birth.adapter.transcript_path_de(birth.sessions.load("s1")))}
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        await owner.refresh_snapshot("s1")
        async def busy():
            with pytest.raises(TransferError, match="session_transfer_source_busy"):
                await _check_source_idle(api.registry, "s1", meta)
        await owner.change("s1", busy)
        transport.view = {}
        await owner.refresh_snapshot("s1")
        async def idle():
            await _check_source_idle(api.registry, "s1", meta)
        await owner.change("s1", idle)
    asyncio.run(scenario())
    assert transport.kinds().count("close") == 2


def test_transfer_stops_source_without_python_client(birth):
    owner, transport, pid = _opened(birth)
    record = SimpleNamespace(name="s1", id="transfer-1", origin_meta={"headless":True})
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        async def stop():
            await api.registry.stop_transfer_source(record, {"processes":{}, "original":birth.sessions.load("s1")})
        await owner.change("s1", stop)
    asyncio.run(scenario())
    assert birth.kills == [pid]
    assert transport.kinds() == ["open", "snapshot", "close"], "parada não reabre"
    assert birth.sessions.load("s1")["transfer_id"] == "transfer-1"


def test_shutdown_leaves_canos_alive_and_touches_no_session(birth, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    owner, transport, _ = _opened(birth)
    monkeypatch.setattr(LegacyBridge, "quiesce", lambda *args: pytest.fail("quiesce na parada"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        await owner.shutdown()
    asyncio.run(scenario())
    assert transport.kinds() == ["open"] and birth.kills == []
    transport.lease.close()


# --- Modo do processo, restart e a guarda do cliente legado (dono único, Task 5) ---

def _queue_with_pending(birth):
    from app import pqueue, runtime_queue
    from app.runtime_coordinator import WriterLease
    key = birth.sessions.load("s1")["key"]
    directory = pqueue._queue_dir()
    lease = WriterLease(directory / "runtime" / f"{key}.lock")
    try:
        runtime_queue.QueueStore(directory / "runtime" / f"{key}.json", directory, runtime_queue.initial_state(
            key, 1, "s1", [{"id":"e1", "text":"oi", "delivered":False, "ts":1.0}]))
    finally:
        lease.close()


def _pending_owner(birth, transport=None):
    owner = birth.build(None)
    owner.mode = "pending"
    return owner


async def _enter(owner, transport):
    owner.loop = asyncio.get_running_loop()
    owner.configure_transport(transport)
    await owner.adoption_task
    return owner


def test_lifespan_with_rust_expected_opens_no_cano_client(birth, monkeypatch):
    _alive_cano(birth)
    owner = _pending_owner(birth)
    calls = []
    from app.adapters.claude_headless import adapter as A
    monkeypatch.setattr(A, "matar_orfaos", lambda: 0)     # a varredura real olha os processos da máquina
    monkeypatch.setattr(birth.adapter, "reconectar_todas", lambda: calls.append("reconectar"))
    monkeypatch.setattr(api, "_recover_pending_transfers", lambda: calls.append("transferencias"))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.start_sessions({"claude":birth.adapter, "codex":SimpleNamespace(_sessions={})})
        await api._boot_sessions(owner)
    asyncio.run(scenario())
    assert not owner.managed_queue("s1"), "nada da sessão Claude sem terminal registrado antes do Rust"
    assert calls == [] and set(owner.mode_hooks) == {"rust", "python"}


def test_rust_up_leaves_claude_headless_to_the_rust(birth):
    # Cano vivo ou fila pendente: o Rust reconhece a sessão sozinho na subida; o Python não a abre.
    _alive_cano(birth)
    _queue_with_pending(birth)
    owner = _pending_owner(birth)
    transport = LockingTransport()
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.start_sessions({"claude":birth.adapter, "codex":SimpleNamespace(_sessions={})})
        await _enter(owner, transport)
    asyncio.run(scenario())
    assert transport.kinds() == [] and owner.mode == "rust"
    assert not owner.managed_queue("s1")


def test_crash_before_limit_keeps_sessions_out_of_python(birth, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    owner, first, _ = _opened(birth)
    monkeypatch.setattr(LegacyBridge, "reconnect", lambda *args: pytest.fail("Python religou a sessão"))
    second = LockingTransport()
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        first.lease.close()             # o Rust caiu: o sistema soltou a trava dele
        await owner.enter_pending()
        slot = owner.slot("s1")
        assert owner.mode == "pending" and slot.phase == runtime_coordinator.Phase.Rust and slot.lease is None
        await _enter(owner, second)
    asyncio.run(scenario())
    # O Rust novo reabre o Claude sem terminal sozinho: o registro daqui sai, sem `open`.
    assert second.kinds() == [] and owner.mode == "rust"
    assert not owner.managed_queue("s1")


def test_send_during_crash_waits_and_repeats_same_operation(birth):
    owner, first, _ = _opened(birth)
    second = LockingTransport()
    first.alive = False
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        owner.mode = "rust"
        original = first.op
        async def dropped(descriptor, command, operation_id, clock):
            if command["kind"] == "submit":
                first.lease.close()
                raise ConnectionResetError("Rust caiu no meio do envio")
            return await original(descriptor, command, operation_id, clock)
        first.op = dropped
        async def supervisor():
            await asyncio.sleep(0.1)
            await owner.enter_pending()
            await asyncio.sleep(0.1)
            await _enter(owner, second)
        task = asyncio.create_task(supervisor())
        result = await owner.op("s1", {"kind":"submit", "text":"Olá"}, "op-crash")
        await task
        return result
    result = asyncio.run(scenario())
    assert result["disposition"] == "deferred"
    assert [(kind, d) for kind, d in second.ops if kind == "submit"] and second.kinds() == ["open", "submit"]
    second.lease.close()


def test_pending_wait_has_ceiling(birth, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "PENDING_WAIT_S", 0.2)
    owner = _pending_owner(birth)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.prepare_session("s1", "claude", launch=True)
    with pytest.raises(RuntimeError) as caught:
        asyncio.run(scenario())
    assert getattr(caught.value, "code", "") == "runtime_starting"


def test_third_crash_recovers_each_session_once(birth, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    owner, transport, _ = _opened(birth)
    reconnected, hooks = [], []
    async def reconnect(self, descriptor, carry):
        reconnected.append(descriptor["name"])
        return {"hydrated":True}
    monkeypatch.setattr(LegacyBridge, "reconnect", reconnect)
    async def python_hook():
        hooks.append("python")
    owner.mode_hooks["python"] = python_hook
    transport.alive = False
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        transport.lease.close()
        await owner.enter_pending()
        await owner.enter_python(SimpleNamespace(containment_clean=lambda: True))
    asyncio.run(scenario())
    assert reconnected == ["s1"] and hooks == ["python"] and owner.mode == "python"
    assert owner.slot("s1").phase == runtime_coordinator.Phase.Python


def test_rust_back_after_give_up_takes_python_sessions_back(birth, monkeypatch):
    from app.adapters.claude_headless.adapter import _Sessao
    from app.runtime_adapter import LegacyBridge
    owner, transport, _ = _opened(birth)
    async def reconnect(self, descriptor, carry):
        return {"hydrated":True}
    monkeypatch.setattr(LegacyBridge, "reconnect", reconnect)
    transport.alive = False
    second = LockingTransport()
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        transport.lease.close()
        await owner.enter_pending()
        await owner.enter_python(SimpleNamespace(containment_clean=lambda: True))
        assert owner.slot("s1").phase == runtime_coordinator.Phase.Python
        birth.adapter._sessions["s1"] = _Sessao("s1", birth.sessions.load("s1"))    # cliente da reserva
        await owner.leave_python()
        assert owner.mode == "pending"
        await _enter(owner, second)
    asyncio.run(scenario())
    # O Rust novo reabre o Claude sem terminal sozinho: o registro daqui sai, sem `open`.
    assert second.kinds() == [] and owner.mode == "rust"
    assert not owner.managed_queue("s1")
    assert "s1" not in birth.adapter._sessions, "o cliente Python solta o cano antes de o Rust abrir"


def test_failed_retry_gives_sessions_back_to_python(birth, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    owner, transport, _ = _opened(birth)
    reconnected, hooks = [], []
    async def reconnect(self, descriptor, carry):
        reconnected.append(descriptor["name"])
        return {"hydrated":True}
    monkeypatch.setattr(LegacyBridge, "reconnect", reconnect)
    async def python_hook():
        hooks.append("python")
    owner.mode_hooks["python"] = python_hook
    transport.alive = False
    contained = SimpleNamespace(containment_clean=lambda: True)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("s1")
        transport.lease.close()
        await owner.enter_pending()
        await owner.enter_python(contained)
        await owner.leave_python()          # a tentativa de religar o Rust não subiu
        await owner.enter_python(contained)
    asyncio.run(scenario())
    assert reconnected == ["s1", "s1"] and hooks == ["python", "python"] and owner.mode == "python"
    slot = owner.slot("s1")
    assert slot.phase == runtime_coordinator.Phase.Python and slot.lease is not None


def test_transfer_recovery_waits_for_owner(birth):
    owner = _pending_owner(birth)
    ran = []
    async def recovery():
        ran.append(owner.mode)
    owner.mode_hooks["rust"] = recovery
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.start_sessions({"claude":birth.adapter, "codex":SimpleNamespace(_sessions={})})
        assert ran == []
        await _enter(owner, LockingTransport())
        await owner.enter_pending()
        await _enter(owner, LockingTransport())
    asyncio.run(scenario())
    assert ran == ["rust"], "uma vez, depois do Rust de pé"


def test_legacy_client_refused_while_rust_owns(birth):
    import inspect
    _alive_cano(birth)
    owner = birth.build(LockingTransport())
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        for mode in ("pending", "rust"):
            owner.mode = mode
            with pytest.raises(RuntimeError, match="cliente Python"):
                await inspect.unwrap(birth.adapter.ensure_running)(birth.adapter, "s1", so_reconectar=True)
    asyncio.run(scenario())


def test_codex_headless_stays_python_while_rust_does_not_announce_it(birth, monkeypatch):
    # Codex sem terminal é do Rust quando a saúde dele o anuncia em `owns` (o padrão já o inclui); um
    # hangar-server que não o anuncia deixa a sessão com o Python.
    from app.adapters.codex import sessions as codex_sessions
    from app.runtime_adapter import LegacyBridge, assert_legacy
    meta = {"name":"c1", "key":"codexkey", "headless":True, "thread_id":"thread", "rollout_path":"/tmp/rollout.jsonl"}
    monkeypatch.setattr(codex_sessions, "load", lambda name: meta if name == "c1" else None)
    transport = LockingTransport()
    owner = birth.build(transport)
    owner.legacy = LegacyBridge(owner, {"claude":birth.adapter, "codex":SimpleNamespace(_sessions={})})
    owner.mode = "rust"
    owner._owns = {("claude", True), ("claude", False)}
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session("c1", "codex", launch=True)
    asyncio.run(scenario())
    assert transport.kinds() == [] and owner.slot("c1").phase == runtime_coordinator.Phase.Python
    assert_legacy("c1")         # o cliente Python do Codex segue permitido


def test_rust_back_after_a_crash_takes_the_codex_headless_without_python_opening(codex_birth, monkeypatch):
    # T-014 (D-014): o Rust novo adota sozinho o Codex sem terminal de cano vivo; o registro daqui sai sem
    # `open`, e nada de reabertura falha ou interrompida no diário.
    import os
    from app import diag
    events = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **campos: events.append((evento, nivel)))
    codex_birth.codex_sessions.update("cx", cano={"pid":os.getpid(), "escuta":"unix:/tmp/vivo.sock", "token":"t", "ts":1.0, "versao":2})
    first = LockingTransport()
    owner = codex_birth.build_codex(first)
    second = LockingTransport()
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        first.lease.close()
        await owner.enter_pending()
        await _enter(owner, second)
    asyncio.run(scenario())
    assert second.kinds() == [] and owner.mode == "rust" and not owner.managed_queue("cx")
    names = {evento for evento, _ in events}
    assert not names & {"runtime.reopen_failed", "runtime.reopen_interrupted", "runtime.registration_failed"}


def test_connection_error_outside_an_opening_is_not_a_rust_crash(monkeypatch):
    from app import diag
    events = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **campos: events.append((evento, nivel)))
    runtime_coordinator._registration_failed("runtime.recover_failed", "s1", ConnectionResetError("cano"))
    assert events == [("runtime.recover_failed", "erro")]


# --- Codex sem terminal nasce e religa no Rust (5B Task 4) ---

CODEX_OWNS = [{"provider":"claude", "headless":True}, {"provider":"claude", "headless":False}, {"provider":"codex", "headless":True}]


@pytest.fixture
def codex_birth(birth, tmp_path, monkeypatch):
    from app.adapters.codex import sem_terminal, sessions as codex_sessions
    from app.adapters.codex.adapter import CodexAdapter
    from app.runtime_adapter import LegacyBridge
    codex_sessions.save("cx", None, "", str(tmp_path), headless=True, key="c" * 32)
    async def no_python_launch(*args, **kwargs):
        pytest.fail("o Python subiu o processo de uma sessão Codex do Rust")
    monkeypatch.setattr(sem_terminal, "subir", no_python_launch)
    monkeypatch.setattr(sem_terminal, "conectar", no_python_launch)
    codex = CodexAdapter()
    def build(transport, mode="rust"):
        owner = birth.build(transport)
        owner.legacy = LegacyBridge(owner, {"claude":birth.adapter, "codex":codex})
        owner._owns = {(item["provider"], item["headless"]) for item in CODEX_OWNS}
        owner.mode = mode
        return owner
    birth.codex, birth.build_codex, birth.codex_sessions = codex, build, codex_sessions
    return birth


class LaunchTransport(Transport):
    """Rust falso que guarda o comando do `open` e o que o coordenador tinha registrado naquela hora."""

    def __init__(self, owner_ref, **kwargs):
        super().__init__(**kwargs)
        self.commands, self.owner_ref, self.phase_at_open = [], owner_ref, None

    async def op(self, descriptor, command, operation_id, clock):
        if command["kind"] == "open":
            self.commands.append(command)
            slot = self.owner_ref[0].slots.get(descriptor["key"])
            # As políticas da subida (launch_env, patch_meta) exigem a posse do Rust já durante o `open`.
            self.phase_at_open = slot.phase if slot is not None else None
        return await super().op(descriptor, command, operation_id, clock)


def test_codex_headless_without_cano_opens_with_launch_in_rust(codex_birth, tmp_path):
    ref = []
    transport = LaunchTransport(ref)
    owner = codex_birth.build_codex(transport)
    ref.append(owner)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
    asyncio.run(scenario())
    assert transport.kinds() == ["open"]
    command = transport.commands[0]
    assert command["launch"] is True
    assert command["descriptor"]["sidecar_dir"] == str(tmp_path / "codex-sessions")
    assert transport.phase_at_open == runtime_coordinator.Phase.Rust
    slot = owner.slot("cx")
    assert slot.phase == runtime_coordinator.Phase.Rust and slot.lease is None and slot.binding.provider == "codex"


def test_codex_ensure_running_goes_to_rust(codex_birth):
    ref = []
    transport = LaunchTransport(ref)
    owner = codex_birth.build_codex(transport)
    ref.append(owner)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await codex_birth.codex.ensure_running("cx")
    asyncio.run(scenario())
    assert transport.kinds() == ["open"] and transport.commands[0]["launch"] is True
    assert "cx" not in codex_birth.codex._sessions, "nenhum cliente Python no cano"


def test_codex_open_failure_leaves_no_registration(codex_birth):
    ref = []
    transport = LaunchTransport(ref, fail="cano_nao_escutou")
    owner = codex_birth.build_codex(transport)
    ref.append(owner)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
    with pytest.raises(Exception) as caught:
        asyncio.run(scenario())
    assert caught.value.code == "cano_nao_escutou"
    assert not owner.managed_queue("cx") and owner.slots == {}


@pytest.mark.parametrize("mode", ["rust", "pending"])
def test_warm_sessions_skips_codex_sessions_of_the_rust(codex_birth, monkeypatch, mode):
    owner = codex_birth.build_codex(Transport(), mode=mode)
    called = []
    async def ensure_running(name):
        called.append(name)
    monkeypatch.setattr(codex_birth.codex, "ensure_running", ensure_running)
    asyncio.run(codex_birth.codex.warm_sessions())
    assert called == [], "backend Python reiniciado com o Rust de pé não toca a sessão"
    owner.mode = "python"
    asyncio.run(codex_birth.codex.warm_sessions())
    assert called == ["cx"]


def test_rust_up_leaves_live_codex_headless_to_the_rust(codex_birth):
    # T-014: o Rust adota sozinho na subida o Codex sem terminal com cano vivo ou fila pendente; o Python
    # não pede a abertura.
    import os
    codex_birth.codex_sessions.update("cx", cano={"pid":os.getpid(), "escuta":"unix:/tmp/vivo.sock", "token":"t", "ts":1.0, "versao":2})
    owner = _pending_owner(codex_birth)
    ref = [owner]
    transport = LaunchTransport(ref)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.start_sessions({"claude":codex_birth.adapter, "codex":codex_birth.codex})
        owner.loop = asyncio.get_running_loop()
        owner.configure_transport(transport, CODEX_OWNS)
        await owner.adoption_task
    asyncio.run(scenario())
    assert transport.kinds() == [] and owner.mode == "rust"
    assert not owner.managed_queue("cx")


def test_codex_with_terminal_never_waits_for_the_rust_in_pending(codex_birth, tmp_path):
    codex_birth.codex_sessions.save("ct", "thread", "", str(tmp_path), key="t" * 32)
    owner = codex_birth.build_codex(Transport(), mode="pending")
    waited = []
    async def await_mode():
        waited.append(True)
        return owner.mode
    owner.await_mode = await_mode
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session("ct", "codex") is False
        with pytest.raises(RuntimeError, match="sem responsável"):
            await owner.op("ct", {"kind":"drain"}, "op")
        assert waited == [], "Codex com terminal é do Python até a 5C"
        await owner.prepare_session("cx", "codex")
        assert waited, "o sem terminal espera o desfecho do Rust"
    asyncio.run(scenario())


# --- Ciclo de vida do Codex sem terminal no Rust (5B Task 5) ---

class LifecycleTransport(LaunchTransport):
    """Rust falso que também responde aos controles e ao `close` (com ou sem `kill`)."""

    def __init__(self, owner_ref, control_error=None, killed=True, **kwargs):
        super().__init__(owner_ref, **kwargs)
        self.control_error, self.killed = control_error, killed

    async def op(self, descriptor, command, operation_id, clock):
        from app.rust_server import RustOpError
        if command["kind"] in {"close", "control"}:
            self.commands.append(command)
            self.ops.append((command["kind"], descriptor))
        if command["kind"] == "close":
            return {"closed": True, **({"killed": self.killed} if command.get("kill") else {})}
        if command["kind"] == "control":
            if self.control_error:
                code, message = self.control_error
                raise RustOpError(f"IPC recusou a operação (503: {code} {message})", 503, code, message)
            payload = {"current": "Ask for approval"} if command["control"] == "set_permission_mode" else {}
            return {"operation_id": operation_id, "disposition": "accepted", "payload": payload}
        return await super().op(descriptor, command, operation_id, clock)


def _lifecycle_owner(codex_birth, monkeypatch, **kwargs):
    ref = []
    transport = LifecycleTransport(ref, **kwargs)
    owner = codex_birth.build_codex(transport)
    ref.append(owner)
    monkeypatch.setattr(api, "get_adapter", lambda provider: codex_birth.codex if provider == "codex" else pytest.fail(provider))
    return owner, transport


def test_codex_close_with_rust_owner_kills_in_rust(codex_birth, monkeypatch):
    from app.adapters.codex import sem_terminal
    monkeypatch.setattr(sem_terminal, "matar", lambda meta: pytest.fail("o Python matou o processo de uma sessão do Rust"))
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        codex_birth.codex._problemas["cx"] = ("codex_headless_nao_subiu", "velho")
        await asyncio.to_thread(codex_birth.codex.close_sync, "cx")
    asyncio.run(scenario())
    closes = [command for command in transport.commands if command["kind"] == "close"]
    assert closes[0].get("kill") is True, "o Rust encerra o processo que ele subiu"
    assert "cx" not in codex_birth.codex._problemas, "a memória do adapter é esquecida mesmo sem matar nada no Python"


def test_codex_reopen_after_change_reads_the_cano_the_rust_recorded(codex_birth, monkeypatch):
    import os
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        # O Rust religou: o arquivo aponta o processo novo, o registro em memória ainda o antigo (morto).
        codex_birth.codex_sessions.update("cx", cano={"pid":os.getpid(), "escuta":"unix:/tmp/vivo.sock", "token":"t", "ts":1.0, "versao":2})
        owner.slot("cx").binding.meta["cano"] = {"pid":999_999_999, "escuta":"unix:/tmp/morto.sock", "token":"t", "ts":1.0, "versao":2}
        async def failing():
            raise RuntimeError("ação da administração falhou")
        # A ação que falha devolve a sessão ao Rust na vida de antes, sem regravar o registro.
        with pytest.raises(RuntimeError, match="administração falhou"):
            await owner.change("cx", failing)
    asyncio.run(scenario())
    assert transport.kinds().count("open") == 2, "processo vivo no arquivo: a sessão volta ao Rust, não fica parada"


def test_codex_close_falls_back_to_python_kill_when_rust_had_no_process(codex_birth, monkeypatch):
    from app.adapters.codex import sem_terminal
    killed = []
    monkeypatch.setattr(sem_terminal, "matar", lambda meta: killed.append(meta["name"]))
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch, killed=False)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        await asyncio.to_thread(codex_birth.codex.close_sync, "cx")
    asyncio.run(scenario())
    assert killed == ["cx"], "sem a sessão aberta no Rust, o processo não fica vivo sem dono"


def test_codex_sandbox_switch_of_a_stopped_session_goes_to_rust(codex_birth, monkeypatch):
    # Cano gravado mas morto e a sessão fora do Rust: a troca não pode virar 503 de cliente Python.
    codex_birth.codex_sessions.update("cx", cano={"pid":999_999_999, "escuta":"unix:/tmp/morto.sock", "token":"t", "ts":1.0})
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        return await api.trocar_permissao_do_codex("cx", api.CodexPermissionBody(mode="Ask for approval"))
    result = asyncio.run(scenario())
    controls = [(command["control"], command["payload"]) for command in transport.commands if command["kind"] == "control"]
    assert controls == [("set_permission_mode", {"mode": "Ask for approval"})]
    assert result == {"current": "Ask for approval"}


def test_codex_switch_to_terminal_has_no_python_hand_over(codex_birth, monkeypatch):
    # Relançar é do Rust: sem passagem, nenhum cliente Python religa no cano, e a sessão volta ao Rust.
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch)
    async def open_terminal(adapter, name):
        runtime_coordinator.refuse_python_client(name, "codex")
    monkeypatch.setattr(codex_birth.codex, "open_terminal", open_terminal, raising=False)
    async def scenario():
        import os
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        # Processo vivo no arquivo: a recusa devolve a sessão ao Rust (cano parado ficaria só registrado).
        codex_birth.codex_sessions.update("cx", cano={"pid":os.getpid(), "escuta":"unix:/tmp/vivo.sock", "token":"t", "ts":1.0, "versao":2})
        with pytest.raises(RuntimeError, match="bloqueado"):
            await owner.lifecycle_call("cx", "open_terminal", {})
    asyncio.run(scenario())
    assert not hasattr(owner, "python_client_released")
    assert "cx" not in codex_birth.codex._sessions, "nenhum cliente Python fica no cano do Rust"
    assert owner.slot("cx").phase == runtime_coordinator.Phase.Rust


@pytest.mark.parametrize("code,status,api_code", [
    ("erro_permissao_ocupada", 409, "erro_permissao_ocupada"),
    ("erro_modo_desconhecido", 400, "erro_permissao_picker"),
])
def test_codex_permission_refusal_from_rust_keeps_the_python_codes(codex_birth, monkeypatch, code, status, api_code):
    from fastapi import HTTPException
    message = "a sessão está trabalhando; mudar o sandbox reiniciaria o Codex — espere ela terminar"
    owner, transport = _lifecycle_owner(codex_birth, monkeypatch, control_error=(code, message))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        await owner.ensure_open("cx")
        await api.trocar_permissao_do_codex("cx", api.CodexPermissionBody(mode="Ask for approval"))
    with pytest.raises(HTTPException) as caught:
        asyncio.run(scenario())
    assert caught.value.status_code == status
    assert caught.value.detail["code"] == api_code and caught.value.detail["msg"] == message


@pytest.mark.parametrize("mode,swept", [("pending", False), ("rust", False), ("python", True)])
def test_boot_sweeps_orphans_only_when_python_owns(monkeypatch, mode, swept):
    from app.adapters.claude_headless import adapter as A
    calls = []
    monkeypatch.setattr(A, "matar_orfaos", lambda: calls.append(mode) or 0)
    async def nothing():
        return None
    monkeypatch.setattr(api, "_python_owns_headless", nothing)
    runtime = SimpleNamespace(mode=mode, mode_hooks={})
    asyncio.run(api._boot_sessions(runtime))
    assert bool(calls) is swept, "com o Rust de pé a varredura de órfãos é dele"


def test_python_never_drains_a_terminal_claude_queue_the_rust_owns(monkeypatch):
    # A trava da fila é do Rust: o drain do Python bateria nela (BlockingIOError) e a cadeia de
    # reavaliação desistiria com as mensagens paradas.
    from app.runtime_coordinator import RuntimeCoordinator
    owner = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    monkeypatch.setattr(api, "_headless", lambda name: False)
    def locked(*args):
        raise BlockingIOError(11, "Resource temporarily unavailable")
    monkeypatch.setattr(api, "drain", locked)
    for mode in ("rust", "pending"):
        owner.mode = mode
        assert api._drenar("terminal", "chat", "claude") == 0, mode
    owner.mode = "python"
    monkeypatch.setattr(api, "drain", lambda *args: 2)
    assert api._drenar("terminal", "chat", "claude") == 2, "no modo python o Python segue drenando"


def test_a_terminal_claude_the_rust_already_adopted_opens_without_taking_its_queue_lock(tmp_path, monkeypatch):
    # O Rust adota o Claude com terminal sozinho (drenagem, limite, lista) e segura a trava da fila: a
    # rota do Python que chega primeiro depois (esforço, modo, tecla) não pode cair no BlockingIOError.
    import fcntl
    from app import runtime_terminal
    from app.runtime_coordinator import Binding, Phase, RuntimeCoordinator
    owner = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    monkeypatch.setattr(runtime_terminal, "validate_binding", lambda descriptor: None)
    monkeypatch.setattr("app.runtime_process.reconcile_startup", lambda allow_current=True: None)
    binding = Binding("s1", "k" * 32, "claude", False, {"terminal": {"pane": "%1"}}, str(tmp_path / "c.jsonl"),
                      tmp_path / "projection", tmp_path / "state.json", tmp_path / "state.lock", 1)
    rust = open(binding.lock_path, "w")
    fcntl.flock(rust, fcntl.LOCK_EX | fcntl.LOCK_NB)
    async def rpc(descriptor, command, operation_id):
        return {"state": {"phase": "idle"}}
    owner._rpc = rpc
    owner._check_opened = lambda ready, descriptor: None
    try:
        slot = asyncio.run(owner._open_terminal("s1", binding))
        assert slot.phase == Phase.Rust
        assert not binding.state_path.exists(), "a fila é do Rust: o Python não a reescreve"
    finally:
        rust.close()
        owner.close_python_leases()
