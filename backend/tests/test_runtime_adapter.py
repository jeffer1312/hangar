import asyncio
from types import SimpleNamespace

import pytest

from app import runtime_coordinator
from app.runtime_adapter import RuntimeAdapter, RuntimeView, install_adapter


class Coordinator:
    instance = "instance-test"

    def __init__(self):
        self.calls = []
        self.target = SimpleNamespace(phase=runtime_coordinator.Phase.Rust,
            binding=SimpleNamespace(key="key", generation=2, provider="codex", headless=True, meta={}),
            view={"key":"key","generation":2,"revision":7,"channels":{},"view":{"alive":True,"ready":True,
                "thread_id":"thread-2","model":"test-model","effort":"high","deliverable":True,
                "public_state":{"session":"session","state":"idle","headless":True}}},
            cache_valid=True, changed=asyncio.Event())

    def managed_runtime(self, name):
        return name == "session"

    def slot(self, name):
        return self.target

    async def op(self, name, command, operation_id):
        self.calls.append((name, command, operation_id))
        return {"disposition":"accepted","payload":[{"model":"test-model"}]}


@pytest.fixture
def owner(monkeypatch):
    coordinator = Coordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    return coordinator


def test_owned_runtime_never_opens_legacy_reader(owner):
    class Adapter:
        async def ensure_running(self, name):
            raise AssertionError("leitor Legacy")

        async def list_models(self, name):
            raise AssertionError("RPC Legacy")

    install_adapter(Adapter, "codex")
    adapter = Adapter()
    assert isinstance(asyncio.run(adapter.ensure_running("session")), RuntimeView)
    assert asyncio.run(adapter.list_models("session")) == [{"model":"test-model"}]
    assert len(owner.calls) == 1


def test_getter_uses_same_generation(owner):
    facade = RuntimeAdapter("codex")
    assert facade.current_model("session")["model"] == "test-model"
    owner.target.view["generation"] = 1
    with pytest.raises(RuntimeError):
        facade.current_model("session")


@pytest.mark.parametrize("tier", ["priority", "default"])
def test_service_tier_uses_owner_without_legacy_session(owner, tier):
    class Adapter:
        async def set_service_tier(self, name, service_tier):
            raise AssertionError("RPC Legacy")

    async def op(name, command, operation_id):
        owner.calls.append(command)
        return {"disposition": "accepted", "payload": {"service_tier": tier}}

    owner.op = op
    owner.target.view["view"]["service_tier"] = tier
    install_adapter(Adapter, "codex")
    assert asyncio.run(Adapter().set_service_tier("session", tier)) == tier
    assert owner.calls == [{"kind": "control", "control": "set_service_tier", "payload": {"service_tier": tier}}]
    assert RuntimeAdapter("codex").current_model("session")["service_tier"] == tier
    assert "service_tier" not in RuntimeAdapter("claude").current_model("session")


def test_service_tier_owner_must_return_confirmed_value(owner):
    async def op(name, command, operation_id):
        return {"disposition": "accepted", "payload": {}}
    owner.op = op
    with pytest.raises(RuntimeError):
        asyncio.run(RuntimeAdapter("codex").dispatch("set_service_tier", "session", {"service_tier": "priority"}))


def test_service_tier_legacy_bridge_preserves_confirmed_result(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key": "key", "thread_id": "thread"},
        str(tmp_path / "chat.jsonl"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    calls = []
    class Adapter:
        async def set_service_tier(self, name, service_tier):
            calls.append((name, service_tier))
            return service_tier
    bridge = LegacyBridge(coordinator, {"codex": Adapter()})
    try:
        result = asyncio.run(bridge.op(slot.binding.descriptor(), {"kind": "control", "control": "set_service_tier",
            "payload": {"service_tier": "default"}}, "tier-op"))
        assert result["disposition"] == "accepted" and result["payload"] == {"service_tier": "default"}
        assert calls == [("session", "default")]
    finally:
        coordinator.close_python_leases()


@pytest.mark.parametrize("reply", [{"models": [{"value": "opus"}]}, [{"value": "opus"}]])
def test_list_models_returns_the_list_not_the_reply_object(monkeypatch, reply):
    """Percorrer o `{"models": [...]}` do Claude dava a chave "models" e quebrava o seletor."""
    async def control(self, name, kind, payload=None, **kwargs):
        return reply
    monkeypatch.setattr(RuntimeAdapter, "control", control)
    assert asyncio.run(RuntimeAdapter("claude").list_models("session")) == [{"value": "opus"}]


@pytest.mark.parametrize("reply", [{}, {"error": "x"}, None, "models", {"models": []}])
def test_list_models_without_a_list_is_an_error_not_an_empty_picker(monkeypatch, reply):
    async def control(self, name, kind, payload=None, **kwargs):
        return reply
    monkeypatch.setattr(RuntimeAdapter, "control", control)
    with pytest.raises(RuntimeError):
        asyncio.run(RuntimeAdapter("claude").list_models("session"))


def test_invalid_event_or_gap_requests_snapshot(owner):
    from app.runtime_adapter import apply_event
    before = dict(owner.target.view)
    event = {"key":"key","generation":2,"revision":9,"channel":"state", "data":{"state":"working"}}
    assert apply_event(owner.target, event) is False
    assert owner.target.cache_valid is False
    assert owner.target.view == before
    with pytest.raises(RuntimeError):
        asyncio.run(RuntimeAdapter("codex").list_models("session"))


def test_old_snapshot_preserves_newer_good_cache(owner):
    from app.runtime_adapter import apply_event
    data = dict(owner.target.view)
    data["revision"] = 6
    before = dict(owner.target.view)
    assert apply_event(owner.target, {"key":"key", "generation":2, "revision":6, "channel":"snapshot", "data":data})
    assert owner.target.view == before
    assert owner.target.cache_valid


def test_sync_lifecycle_does_not_touch_legacy_while_owned(owner):
    class Adapter:
        def close_sync(self, name):
            raise AssertionError("fechamento Legacy sem barreira")
        def rename(self, old, new):
            raise AssertionError("rename Legacy sem barreira")
    install_adapter(Adapter, "codex")
    adapter = Adapter()
    with pytest.raises(RuntimeError):
        adapter.close_sync("session")
    with pytest.raises(RuntimeError):
        adapter.rename("session", "new")


def test_sync_endpoint_uses_server_loop(owner):
    from app.runtime_adapter import run_sync
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        async def value():
            assert asyncio.get_running_loop() is owner.loop
            return 17
        assert await asyncio.to_thread(run_sync, value, owner.loop) == 17
        with pytest.raises(RuntimeError):
            run_sync(value, owner.loop)
    asyncio.run(scenario())


def test_legacy_reserve_journals_controls(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyIO
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True,
        {"key":"key", "session_id":"sid"}, str(tmp_path / "chat.jsonl"), tmp_path / "projection",
        tmp_path / "state.json", tmp_path / "lease", 1))
    class Writer:
        def write(self, raw):
            import json
            envelope = json.loads(raw)
            assert slot.store.state["operations"][envelope["operation_id"]]["status"] == "dispatching"

        async def drain(self):
            raise OSError("partial write")
    async def scenario():
        io = LegacyIO(coordinator)
        endpoint = SimpleNamespace(runtime_acks={})
        with pytest.raises(RuntimeError):
            await io.write("session", endpoint, Writer(), {"type":"control_request", "request_id":"request",
                "request":{"subtype":"set_model", "model":"test-model"}}, 2)
        phases = [operation for operation in slot.store.state["operations"].values()
                  if operation["payload"].get("frame")]
        assert len(phases) == 1
        assert phases[0]["status"] == "unknown"
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def test_steer_queue_uses_one_owner_command(owner):
    async def op(name, command, operation_id):
        owner.calls.append(command)
        return {"disposition":"accepted", "payload":{"ids":["entry"]}}
    owner.op = op
    result = asyncio.run(RuntimeAdapter("codex").dispatch("steer_queue", "session", {"entry_id":"entry"}))
    assert result == ["entry"]
    assert owner.calls == [{"kind":"control", "control":"steer_queue", "payload":{"entry_id":"entry"}}]


def test_codex_reserve_permission_uses_provider_signature(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key":"key", "thread_id":"thread"},
        str(tmp_path / "chat.jsonl"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    calls = []
    class Adapter:
        async def set_permission_mode_sem_terminal(self, name, modo):
            calls.append((name, modo))
            return {"mode":modo}
    bridge = LegacyBridge(coordinator, {"codex":Adapter()})
    try:
        result = asyncio.run(bridge.op(slot.binding.descriptor(), {"kind":"control", "control":"set_permission_mode",
            "payload":{"mode":"Full Access"}}, "permission-op"))
        assert result["disposition"] == "accepted"
        assert calls == [("session", "Full Access")]
    finally:
        coordinator.close_python_leases()


def test_resubmit_after_prune_of_confirmed_row_does_not_send_again(tmp_path, monkeypatch):
    from app import runtime_queue
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key":"key", "thread_id":"thread"},
        str(tmp_path / "chat.jsonl"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    sent = []
    class Adapter:
        async def send_prompt(self, name, text, pre_transcript=False):
            sent.append(text)
            return {"sent":True}
    bridge = LegacyBridge(coordinator, {"codex":Adapter()})
    sample = {"monotonic_s":1, "epoch_s":1800000000}
    try:
        submit = lambda: asyncio.run(bridge.op(slot.binding.descriptor(), {"kind":"submit", "text":"Olá"}, "msg"))
        assert submit()["disposition"] == "accepted"
        # O adaptador falso não passa pelo fio, que é quem marca a entrega.
        slot.store.exec(1, "delivered", sample, {"kind":"set_delivered", "entry_id":"msg", "value":True, "steered":False})
        slot.store.exec(1, "confirm", sample, {"kind":"confirm", "entry_ids":["msg"]})
        for index in range(20):
            slot.store.exec(1, f"fill:{index}", sample, {"kind":"set_runtime_state", "state":{}})
        assert "msg" not in slot.store.state["operations"]
        # A mesma operação chegando depois da poda: a linha confirmada responde, nada sai de novo.
        assert submit()["disposition"] == "accepted"
        assert sent == ["Olá"]
    finally:
        coordinator.close_python_leases()


def test_reserve_composite_waits_for_both_replies(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyIO, _legacy_operation
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    async def scenario():
        context = {"operation_id":"composite", "command":{"kind":"set_model", "payload":{"effort":"high"}}}
        token = _legacy_operation.set(context)
        coordinator.legacy_active.add("composite")
        io = LegacyIO(coordinator)
        try:
            for request_id, subtype in (("model", "set_model"), ("effort", "set_effort")):
                ticket = await io.prepare_wire("session", subtype,
                    {"type":"control_request", "request_id":request_id, "request":{"subtype":subtype}})
                endpoint = SimpleNamespace(runtime_tickets={(str, request_id):ticket})
                await io.reply("session", endpoint, {"type":"control_response", "response":{"request_id":request_id}})
                await io.finish_wire(ticket, "unknown")
                assert slot.store.state["operations"]["composite"]["status"] == "dispatching"
            await io.finish_call("session", context)
            assert slot.store.state["operations"]["composite"]["status"] == "accepted"
        finally:
            coordinator.legacy_active.discard("composite")
            _legacy_operation.reset(token)
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def test_reserve_bootstrap_before_rollout(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyIO
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key":"key"}, "",
        tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    try:
        ticket = asyncio.run(LegacyIO(coordinator).prepare_wire("session", "initialize",
            {"id":"bootstrap", "method":"initialize", "params":{}}))
        assert slot.store.state["operations"][ticket.phase_id]["status"] == "dispatching"
    finally:
        coordinator.close_python_leases()


def test_reply_without_ack_is_final(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyIO, _legacy_operation
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    async def scenario():
        io = LegacyIO(coordinator)
        endpoint = SimpleNamespace(runtime_acks={})
        context = {"operation_id":"control", "command":{"kind":"list_models", "payload":{}}}
        token = _legacy_operation.set(context)
        coordinator.legacy_active.add("control")
        class Writer:
            def write(self, raw):
                pass
            async def drain(self):
                await io.reply("session", endpoint, {"id":"request", "result":{"data":[]}})
        try:
            await asyncio.wait_for(io.write("session", endpoint, Writer(),
                {"id":"request", "method":"model/list", "params":{}}, 2), 2)
            await io.finish_call("session", context)
            assert slot.store.state["operations"]["control"]["status"] == "accepted"
            assert not endpoint.runtime_acks
        finally:
            coordinator.legacy_active.discard("control")
            _legacy_operation.reset(token)
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


@pytest.mark.parametrize("reload", [False, True])
def test_confirmed_prompt_does_not_consume_next_echo(tmp_path, monkeypatch, reload):
    import json
    from app.runtime_adapter import LegacyBridge
    from app.runtime_receipt import ReceiptIndex
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    from app.runtime_queue import QueueStore, initial_state
    path = tmp_path / "chat.jsonl"
    path.touch()
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True, {"key":"key", "session_id":"sid"},
        str(path), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    bridge = LegacyBridge(coordinator, {})
    sample = {"monotonic_s":1, "epoch_s":1800000000}
    async def scenario():
        for identifier in ("first", "second"):
            cursor = ReceiptIndex("claude", "sid").capture(path)
            slot.store.exec(1, identifier + ":append", sample, {"kind":"append", "text":"Olá", "delivered":False,
                "ts":None, "pre_transcript":False, "entry_id":identifier})
            slot.store.exec(1, identifier + ":prepare", sample, {"kind":"prepare", "id":identifier,
                "entry_id":identifier, "payload":{"kind":"input"}})
            slot.store.exec(1, identifier + ":cursor", sample, {"kind":"bind_dispatch", "id":identifier, "cursor":cursor})
            slot.store.exec(1, identifier + ":dispatch", sample, {"kind":"begin_dispatch", "id":identifier, "wire_id":identifier})
            with path.open("a") as stream:
                stream.write(json.dumps({"type":"user", "uuid":identifier, "message":{"role":"user", "content":"Olá"}}) + "\n")
            assert (await bridge.confirm(slot.binding.descriptor()))["confirmed"] == 1
            if reload:
                slot.store = QueueStore(slot.binding.state_path, slot.binding.projection_dir, initial_state("key", 1, "session", []))
        assert all(row["confirmed"] for row in slot.store.state["rows"])
        # As duas confirmadas: não resta operação que possa casar os ecos, e o uso sai da poda.
        assert slot.store.state["used_occurrences"] == {}
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def test_later_rollout_path_rebinds_same_conversation(tmp_path, monkeypatch):
    from app.runtime_coordinator import Binding, RuntimeCoordinator, Phase
    target = Binding("session", "key", "codex", True, {"key":"key", "thread_id":"thread"}, "",
        tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1)
    class Legacy:
        def binding(self, name, provider):
            import copy
            new = copy.deepcopy(target)
            new.jsonl = str(tmp_path / "rollout.jsonl")
            return new
    coordinator = RuntimeCoordinator(legacy=Legacy())
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(target)
    slot.phase = Phase.Rust
    calls = []
    async def change(name, action, **kwargs):
        calls.append(kwargs)
        slot.binding.jsonl = str(tmp_path / "rollout.jsonl")
        slot.phase = Phase.Python
    coordinator.change = change
    try:
        assert asyncio.run(coordinator.prepare_session("session", "codex"))
        assert calls == [{"advance":False, "reopen":False}]
        assert slot.binding.generation == 1
    finally:
        coordinator.close_python_leases()


def test_v1_control_waits_for_cli_reply_without_inventing_ack(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyIO, _legacy_operation
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    async def scenario():
        io = LegacyIO(coordinator)
        endpoint = SimpleNamespace(runtime_acks={})
        context = {"operation_id":"v1-control", "command":{"kind":"list_models", "payload":{}}}
        token = _legacy_operation.set(context)
        coordinator.legacy_active.add("v1-control")
        writes = []
        class Writer:
            def write(self, raw):
                writes.append(raw)
            async def drain(self):
                pass
        try:
            ticket = await io.write("session", endpoint, Writer(), {"id":"request", "method":"model/list", "params":{}}, 1)
            assert slot.store.state["operations"][ticket.phase_id]["status"] == "unknown"
            await io.reply("session", endpoint, {"id":"request", "result":{"data":[]}})
            await io.finish_call("session", context)
            assert slot.store.state["operations"]["v1-control"]["status"] == "accepted"
            assert len(writes) == 1 and b"cano_input" not in writes[0]
        finally:
            coordinator.legacy_active.discard("v1-control")
            _legacy_operation.reset(token)
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def test_v1_prompt_stays_unknown_until_transcript_proof(tmp_path, monkeypatch):
    import json
    from app.runtime_adapter import LegacyIO, LegacyBridge, _legacy_operation
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    path = tmp_path / "chat.jsonl"
    path.touch()
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True, {"key":"key", "session_id":"sid"},
        str(path), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    async def scenario():
        io = LegacyIO(coordinator)
        await io._exec("session", {"kind":"append", "text":"Olá", "delivered":False, "ts":None,
            "pre_transcript":False, "entry_id":"entry"})
        context = {"operation_id":"entry", "entry_id":"entry", "command":{"kind":"input", "payload":{"text":"Olá"}}}
        token = _legacy_operation.set(context)
        coordinator.legacy_active.add("entry")
        writes = []
        class Writer:
            def write(self, raw):
                writes.append(raw)
            async def drain(self):
                pass
        try:
            await io.write("session", SimpleNamespace(runtime_acks={}), Writer(),
                {"type":"user", "message":{"role":"user", "content":"Olá"}}, 1)
            with pytest.raises(RuntimeError):
                await io.finish_call("session", context)
            assert slot.store.state["operations"]["entry"]["status"] == "unknown"
            with pytest.raises(ValueError):
                await io._exec("session", {"kind":"set_delivered", "entry_id":"entry", "value":False, "steered":False})
            path.write_text(json.dumps({"type":"user", "uuid":"echo", "message":{"role":"user", "content":"Olá"}}) + "\n")
            assert (await LegacyBridge(coordinator, {}).confirm(slot.binding.descriptor()))["confirmed"] == 1
            assert slot.store.state["rows"][0]["confirmed"]
            assert len(writes) == 1
        finally:
            coordinator.legacy_active.discard("entry")
            _legacy_operation.reset(token)
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


@pytest.mark.parametrize("stage", ["before_write", "claiming", "acked", "no_ack"])
def test_quiesce_settles_entry_claimed_by_cancelled_drain(tmp_path, monkeypatch, stage):
    # Sessão sem terminal nascendo: a passagem ao Rust cancela o drain da reserva Python no meio.
    # Antes da escrita a entrada volta à fila; com a escrita em voo o quiesce espera o ack; sem ack
    # ela fica desistida (visível), nunca "entregue" sem ter saído.
    from app import runtime_adapter
    from app import runtime_queue
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
    from app.pqueue import PromptQueue
    from app.runtime_adapter import LegacyBridge, _legacy_operation
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    monkeypatch.setattr(runtime_queue, "_coordinator", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    adapter = ClaudeHeadlessAdapter()
    coordinator.legacy = LegacyBridge(coordinator, {"claude": adapter})
    async def discard():
        return None
    buffers = SimpleNamespace(discard=discard)
    sess = SimpleNamespace(name="session", initialized=asyncio.Event(), model=None, effort=None, permission_mode=None,
        modo_nao_plan=None, comandos=[], comandos_terminal=[], usage=None, context_window=None, cost=None,
        desligando=False, live_active=None, drenador=None, proc=None, leitor=None,
        preview_buffer=buffers, thinking_buffer=buffers, tool_buffer=buffers)
    adapter._sessions["session"] = sess
    claimed = asyncio.Event()
    written = []
    async def write(sess, frame):
        claimed.set()
        if stage == "no_ack":
            await asyncio.Event().wait()
        await asyncio.sleep(0.2)              # o ack do cano chega depois de o quiesce começar
        written.append(frame["message"]["content"])
    adapter._write = write
    monkeypatch.setattr(runtime_adapter, "_WRITE_WAIT_S", 0.5)
    async def send_prompt(name, text):
        if stage in {"acked", "no_ack"}:
            await adapter._escrever_prompt(sess, text)
        claimed.set()
        await asyncio.Event().wait()
    adapter.send_prompt = send_prompt
    if stage == "claiming":
        import threading
        from app import pqueue
        release = threading.Event()
        original_claim = pqueue.PromptQueue.claim_undelivered
        def slow_claim(self, *args, **kwargs):
            loop.call_soon_threadsafe(claimed.set)
            release.wait(5)
            return original_claim(self, *args, **kwargs)
        monkeypatch.setattr(pqueue.PromptQueue, "claim_undelivered", slow_claim)
    def delivered():
        return [row["delivered"] for row in slot.store.state["rows"]]
    async def scenario():
        nonlocal loop
        loop = asyncio.get_running_loop()
        await asyncio.to_thread(PromptQueue("session").append, "texto", delivered=False)
        # O drain da reserva roda como operação do coordenador (`acordar` → op drain).
        token = _legacy_operation.set({"operation_id":"drain-op"})
        coordinator.legacy_active.add("drain-op")
        sess.drenador = asyncio.create_task(adapter.drain("session", ""))
        _legacy_operation.reset(token)
        await asyncio.wait_for(claimed.wait(), 3)
        if stage == "claiming":
            sess.drenador.cancel()
            release.set()
        else:
            assert delivered() == [True]
        await coordinator.legacy.quiesce(slot.binding.descriptor())
        assert sess.drenador.cancelled()
    loop = None
    asyncio.run(scenario())
    rows = slot.store.state["rows"]
    if stage == "acked":
        assert written and rows[0]["delivered"] is True and not rows[0].get("desistiu")
    elif stage == "no_ack":
        assert not written and rows[0]["delivered"] is True and rows[0]["desistiu"] is True
    else:
        assert delivered() == [False]
    assert adapter.drain_claims == {}


def test_quiesce_stops_drain_outside_the_cancelled_tasks(tmp_path, monkeypatch):
    # Drain dono de uma reivindicação, fora das tarefas da sessão, não sobrevive à passagem.
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator
    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "claude", True, {"key":"key"},
        str(tmp_path / "chat"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    adapter = ClaudeHeadlessAdapter()
    coordinator.legacy = LegacyBridge(coordinator, {"claude": adapter})
    calls = []
    slot.store.exec = lambda *args: calls.append(args)
    async def scenario():
        live = asyncio.create_task(asyncio.Event().wait())
        claim = adapter.drain_claims["session"] = {"id":"entry", "task":live}
        await coordinator.legacy.quiesce(slot.binding.descriptor())
        assert live.cancelled() and adapter.drain_claims == {}
    asyncio.run(scenario())
    assert calls == []          # sem linha na fila: nada a devolver


class _Owner:
    def __init__(self, phase):
        from app.runtime_coordinator import Phase
        self.instance = "instance-test"
        self.target = SimpleNamespace(phase=phase, lease=None,
            binding=SimpleNamespace(key="key", generation=1, provider="claude", headless=True, meta={}))
        self.Phase = Phase

    def managed_runtime(self, name):
        return True

    def slot(self, name):
        return self.target


def test_state_stream_never_waits_for_owner(monkeypatch):
    # Sem passagem de dono: a sessão no meio de um fechamento da administração segue pela vista do
    # Python, sem espera nem diário de dono preso, e volta à do Rust quando reabre.
    from app import runtime_adapter
    from app.runtime_coordinator import Phase
    monkeypatch.setattr(runtime_adapter, "_OWNER_POLL_S", 0.01)
    owner = _Owner(Phase.RecoveringPython)
    monkeypatch.setattr(runtime_coordinator, "_current", owner)

    class Adapter:
        async def state_monitor(self, name, sid_get):
            yield "python"
            await asyncio.Event().wait()

    async def native(self, name, sid_get):
        yield "rust"
        await asyncio.Event().wait()
    monkeypatch.setattr(RuntimeAdapter, "state_stream", native)
    install_adapter(Adapter, "claude")

    async def scenario():
        stream = Adapter().state_monitor("session", lambda: None)
        assert await asyncio.wait_for(anext(stream), 0.5) == "python"
        owner.target.phase = Phase.Rust
        assert await asyncio.wait_for(anext(stream), 1) == "rust"
        await stream.aclose()
    asyncio.run(scenario())


def test_state_monitor_error_without_owner_change_still_surfaces(monkeypatch):
    from app import runtime_adapter
    from app.runtime_coordinator import Phase
    monkeypatch.setattr(runtime_adapter, "_OWNER_POLL_S", 0.01)
    monkeypatch.setattr(runtime_coordinator, "_current", _Owner(Phase.Python))

    class Adapter:
        async def state_monitor(self, name, sid_get):
            raise ValueError("falha real")
            yield
    install_adapter(Adapter, "claude")

    async def scenario():
        with pytest.raises(ValueError):
            await anext(Adapter().state_monitor("session", lambda: None))
    asyncio.run(scenario())


def test_reads_during_hand_over_use_python_view(monkeypatch):
    # A lista de sessões lia o estado na passagem e dava 500 para todas as sessões.
    from app.runtime_coordinator import Phase, TransferInProgress
    owner = _Owner(Phase.RecoveringPython)
    owner.legacy_active = set()
    monkeypatch.setattr(runtime_coordinator, "_current", owner)

    class Adapter:
        def snapshot(self, name):
            return "vista python"

        def rename(self, old, new):
            raise AssertionError("rename sem barreira")
    install_adapter(Adapter, "claude")
    assert Adapter().snapshot("session") == "vista python"
    with pytest.raises(TransferInProgress):
        Adapter().rename("session", "new")


@pytest.mark.parametrize("reply,expected", [
    ({"rateLimits": {"limitId": "codex", "primary": {"usedPercent": 3}}}, {"limitId": "codex", "primary": {"usedPercent": 3}}),
    ({}, None), (ValueError("recusado"), None), (RuntimeError("incerto"), None),
])
def test_read_rate_limits_returns_the_snapshot_like_the_python_adapter(monkeypatch, reply, expected):
    """O `/limits` do convidado passa por aqui: com o retrato cru (`{"rateLimits": ...}`) a rota saía toda null."""
    async def control(self, name, kind, payload=None, **kwargs):
        if isinstance(reply, Exception):
            raise reply
        return reply
    monkeypatch.setattr(RuntimeAdapter, "control", control)
    assert asyncio.run(RuntimeAdapter("codex").dispatch("read_rate_limits", "session", {})) == expected


def test_codex_unknown_send_stays_unknown_and_cannot_be_replayed(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator

    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True,
        {"key": "key", "thread_id": "thread"}, str(tmp_path / "chat.jsonl"),
        tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    calls = []

    class Adapter:
        async def send_prompt(self, name, text):
            calls.append(text)
            return "unknown"

    bridge = LegacyBridge(coordinator, {"codex": Adapter()})
    try:
        command = {"kind": "submit", "text": "sem repetir"}
        result = asyncio.run(bridge.op(slot.binding.descriptor(), command, "entry"))
        assert result["disposition"] == "unknown"
        assert slot.store.state["operations"]["entry"]["status"] == "unknown"
        assert slot.store.state["rows"][0]["delivered"] is True
        assert asyncio.run(bridge.op(slot.binding.descriptor(), command, "entry"))["disposition"] == "unknown"
        assert asyncio.run(bridge.op(slot.binding.descriptor(), {"kind": "drain"}, "drain"))["sent"] == 0
        assert calls == ["sem repetir"]
    finally:
        coordinator.close_python_leases()


async def test_cancelled_codex_runtime_send_preserves_uncertainty(tmp_path, monkeypatch):
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator

    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True,
        {"key": "key", "thread_id": "thread"}, str(tmp_path / "chat.jsonl"),
        tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    written = asyncio.Event()

    class Adapter:
        async def send_prompt(self, name, text):
            assert slot.store.state["rows"][0]["delivered"] is True
            written.set()
            await asyncio.Event().wait()

    bridge = LegacyBridge(coordinator, {"codex": Adapter()})
    try:
        command = {"kind": "submit", "text": "sem repetir"}
        task = asyncio.create_task(bridge.op(slot.binding.descriptor(), command, "entry"))
        await written.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert slot.store.state["operations"]["entry"]["status"] == "unknown"
        assert (await bridge.op(slot.binding.descriptor(), command, "entry"))["disposition"] == "unknown"
        assert (await bridge.op(slot.binding.descriptor(), {"kind": "drain"}, "drain"))["sent"] == 0
    finally:
        coordinator.close_python_leases()


@pytest.mark.parametrize("unknown", [False, True])
async def test_codex_runtime_steer_preserves_only_uncertain_attempts(tmp_path, monkeypatch, unknown):
    from app.adapters.codex.appserver import RequestNotSent, RequestOutcomeUnknown
    from app.runtime_adapter import LegacyBridge
    from app.runtime_coordinator import Binding, RuntimeCoordinator

    coordinator = RuntimeCoordinator()
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(Binding("session", "key", "codex", True,
        {"key": "key", "thread_id": "thread"}, str(tmp_path / "chat.jsonl"),
        tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1))
    failure = RequestOutcomeUnknown if unknown else RequestNotSent

    class Adapter:
        async def steer(self, name, text):
            raise failure("não houve confirmação" if unknown else "nenhuma escrita")

    bridge = LegacyBridge(coordinator, {"codex": Adapter()})
    try:
        with pytest.raises(failure):
            await bridge.op(slot.binding.descriptor(),
                {"kind": "submit", "text": "orientação", "steer": True}, "entry")
        assert slot.store.state["operations"]["entry"]["status"] == ("unknown" if unknown else "rejected")
        assert slot.store.state["rows"][0]["delivered"] is unknown
    finally:
        coordinator.close_python_leases()
