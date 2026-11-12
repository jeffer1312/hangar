import asyncio
from types import SimpleNamespace

import pytest

from app import api, runtime_coordinator
from app.runtime_coordinator import Binding, RuntimeCoordinator
from app.rust_server import RustOpError


class Transport:
    instance = "instance-1"

    def __init__(self, log=None, busy=False):
        self.log = [] if log is None else log
        self.busy = busy
        self.held = []

    async def op(self, descriptor, command, operation_id, clock):
        assert descriptor["key"] and operation_id
        self.log.append((command["kind"], command.get("name"), command.get("closed")))
        if command["kind"] == "ingress":
            self.held.append(command.get("held"))
        if command["kind"] == "ingress" and command["closed"] and self.busy:
            raise RustOpError("ingress_busy", 503, "ingress_busy")
        return {"closed": command.get("closed")}

    def ingress(self):
        return [(n, c) for k, n, c in self.log if k == "ingress"]

    def holds(self):
        return [(n, c, h) for (n, c), h in zip(self.ingress(), self.held)]


def binding(tmp_path, name="session"):
    return Binding(name=name, key="key", provider="claude", headless=True,
        meta={"key": "key", "cano": {"pid": 42, "escuta": "tcp:127.0.0.1:1", "token": "t", "versao": 2}},
        jsonl=str(tmp_path / "chat.jsonl"), projection_dir=tmp_path / "projection",
        state_path=tmp_path / "key.queue-state.json", lock_path=tmp_path / "key.lock", generation=1)


def coordinator_with(tmp_path, transport):
    coordinator = RuntimeCoordinator(transport, None)
    slot = coordinator.register(binding(tmp_path))
    return coordinator, slot


def test_freeze_closes_before_frozen_and_opens_on_exit(tmp_path):
    transport = Transport()
    coordinator, slot = coordinator_with(tmp_path, transport)
    async def flow():
        async with coordinator.freeze("session"):
            assert transport.ingress() == [("session", True)] and slot.frozen
        assert transport.ingress() == [("session", True), ("session", False)] and not slot.frozen
        # Congelamento curto: fechamento comum, o Rust espera reabrir em vez de recusar.
        assert transport.holds() == [("session", True, None), ("session", False, None)]
    asyncio.run(flow())
    coordinator.close_python_leases()


def test_freeze_reopens_when_block_raises(tmp_path):
    transport = Transport()
    coordinator, _ = coordinator_with(tmp_path, transport)
    async def flow():
        with pytest.raises(ValueError):
            async with coordinator.freeze("session"):
                raise ValueError("boom")
    asyncio.run(flow())
    assert transport.ingress() == [("session", True), ("session", False)]
    coordinator.close_python_leases()


def test_ingress_busy_propagates_and_freeze_does_not_proceed(tmp_path):
    transport = Transport(busy=True)
    coordinator, slot = coordinator_with(tmp_path, transport)
    async def flow():
        with pytest.raises(RustOpError) as caught:
            async with coordinator.freeze("session"):
                pytest.fail("não pode congelar com a porta aberta")
        assert caught.value.code == "ingress_busy"
        assert not slot.frozen
    asyncio.run(flow())
    assert transport.ingress() == [("session", True)]     # nada fechado: nada a abrir
    coordinator.close_python_leases()


def session_file(monkeypatch, record):
    """Arquivo da sessão falso que `_rebind` relê antes de religar."""
    from app.adapters.claude_headless import sessions
    monkeypatch.setattr(sessions, "load", lambda name: dict(record))


def test_rebind_closes_gate_before_marking_frozen(tmp_path, monkeypatch):
    session_file(monkeypatch, {"session_id": "sid-new"})
    transport = Transport()
    coordinator, slot = coordinator_with(tmp_path, transport)
    seen = {}
    async def change(name, action, **kw):
        seen["gate"], seen["frozen"] = list(transport.ingress()), slot.frozen
    coordinator.change = change
    async def flow():
        coordinator._rebind(slot, "sid-new")
        assert not slot.frozen                      # a marca só vem depois da porta fechada
        await coordinator.rebindings[slot.binding.key]
    asyncio.run(flow())
    assert seen == {"gate": [("session", True)], "frozen": True}
    assert transport.ingress() == [("session", True), ("session", False)]
    coordinator.close_python_leases()


def test_rebind_reads_the_real_claude_session_file(tmp_path, monkeypatch):
    # Sem fake de `_sessions`: a chamada tem de casar com a assinatura real (antes, `TypeError` calado na tarefa).
    from app.adapters.claude_headless import sessions
    monkeypatch.setattr(sessions, "load", lambda name: {"session_id": "sid-new"})
    coordinator, slot = coordinator_with(tmp_path, Transport())
    changes = []
    async def change(name, action, **kw):
        changes.append(name)
    coordinator.change = change
    async def flow():
        coordinator._rebind(slot, "sid-new")
        await coordinator.rebindings[slot.binding.key]
    asyncio.run(flow())
    assert changes == ["session"]
    coordinator.close_python_leases()


def test_codex_conversation_change_is_not_rebound_here(tmp_path, monkeypatch):
    # Thread nova do Codex é do Rust: o Python não relê sidecar nem religa.
    coordinator, slot = coordinator_with(tmp_path, Transport())
    slot.binding.provider = "codex"
    coordinator._rebind(slot, "thread-new")
    assert slot.binding.key not in coordinator.rebindings
    coordinator.close_python_leases()


def test_rebind_waits_for_the_conversation_in_the_session_file(tmp_path, monkeypatch):
    # A vista com a thread nova chega antes do patch: religar agora relia o arquivo sem ela, matava o
    # patch em voo e a vida nova abria outra thread. Só religa quando o arquivo já tem a conversa.
    record = {"session_id": None}
    session_file(monkeypatch, record)
    coordinator, slot = coordinator_with(tmp_path, Transport())
    changes = []
    async def change(name, action, **kw):
        changes.append(name)
    coordinator.change = change
    async def flow():
        coordinator._rebind(slot, "sid-new")
        await coordinator.rebindings[slot.binding.key]
        assert changes == []
        record["session_id"] = "sid-new"
        coordinator._rebind(slot, "sid-new")
        await coordinator.rebindings[slot.binding.key]
    asyncio.run(flow())
    assert changes == ["session"]
    coordinator.close_python_leases()


def test_rebind_loop_is_capped_reported_once_and_retries_after_the_window(tmp_path, monkeypatch):
    from app import diag
    session_file(monkeypatch, {"session_id": "sid-new"})
    coordinator, slot = coordinator_with(tmp_path, Transport())
    changes, reported, now = [], [], [1000.0]
    async def change(name, action, **kw):
        changes.append(name)
    coordinator.change = change
    monkeypatch.setattr(diag, "registrar", lambda event, level, **kw: reported.append((event, kw)))
    monkeypatch.setattr(runtime_coordinator.time, "monotonic", lambda: now[0])
    async def rebind(times):
        for _ in range(times):
            coordinator._rebind(slot, "sid-new")
            await coordinator.rebindings[slot.binding.key]
            now[0] += 1
    asyncio.run(rebind(10))
    assert len(changes) == 3
    assert reported == [("runtime.rebind_loop", {"sessao": "session", "codigo": "rebind_loop"})]
    now[0] += 60
    asyncio.run(rebind(1))
    assert len(changes) == 4                        # tentativas recusadas não prendem o teto
    coordinator.close_python_leases()


def test_no_rust_sends_nothing(tmp_path):
    coordinator = RuntimeCoordinator(None, None)
    coordinator.register(binding(tmp_path))
    async def flow():
        await coordinator.ingress("session", True)
        async with coordinator.freeze("session"):
            pass
    asyncio.run(flow())
    coordinator.ingress_sync("session", True)       # sem Rust nem laço: não faz nada
    coordinator.close_python_leases()


def test_ingress_sync_from_the_loop_raises(tmp_path):
    coordinator, _ = coordinator_with(tmp_path, Transport())
    async def flow():
        coordinator.loop = asyncio.get_running_loop()
        with pytest.raises(RuntimeError):
            coordinator.ingress_sync("session", True)
    asyncio.run(flow())
    coordinator.close_python_leases()


def test_ingress_sync_from_thread_goes_through_the_loop(tmp_path):
    transport = Transport()
    coordinator, _ = coordinator_with(tmp_path, transport)
    async def flow():
        coordinator.loop = asyncio.get_running_loop()
        await asyncio.to_thread(coordinator.ingress_sync, "session", True)
    asyncio.run(flow())
    assert transport.ingress() == [("session", True)]
    coordinator.close_python_leases()


# --- troca de conversa -------------------------------------------------------------------



# --- sessão Claude sem vínculo no Rust ---------------------------------------------------

class RustModeOwner:
    mode = "rust"
    legacy = object()
    loop = None

    def managed_runtime(self, name):
        return False

    def managed_queue(self, name):
        return False

    async def prepare_session(self, name, provider, *, launch=False, engine_models=None):
        return False


def test_headless_without_rust_binding_is_an_error(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "_current", RustModeOwner())
    monkeypatch.setattr(api, "_enviar_nativo", lambda *a: pytest.fail("socket nativo"))
    result = asyncio.run(api._send_one_headless("s", "oi"))
    assert result["ok"] is False and result["delivered"] is False
    assert "vínculo no Rust" in str(result["error"])


def test_available_without_rust_binding_is_an_error(monkeypatch):
    owner = RustModeOwner()
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        monkeypatch.setattr(runtime_coordinator, "_current", owner)
        monkeypatch.setattr(api, "_pane_info", lambda name: ("claude", "%1"))
        from app import terminal_input
        monkeypatch.setattr(terminal_input, "_send_lock", lambda *a, **k: pytest.fail("tmux"))
        monkeypatch.setattr(api, "_enviar_nativo", lambda *a: pytest.fail("socket nativo"))
        return await asyncio.to_thread(api._send_one, "s", "oi")
    result = asyncio.run(scenario())
    # O texto vai ao Rust pela ponte; sem ela de pé o erro tem o código dela, e nada é digitado aqui.
    assert result["ok"] is False and result["delivered"] is False
    assert result["error"]["code"] == "claude_bridge_failed"


# --- rodada de correção --------------------------------------------------------------------

def test_cancelled_close_is_undone_once_the_request_lands(tmp_path):
    release = asyncio.Event()
    transport = Transport()
    original = transport.op
    async def slow(descriptor, command, operation_id, clock):
        if command["closed"]:
            await release.wait()
        return await original(descriptor, command, operation_id, clock)
    transport.op = slow
    coordinator, _ = coordinator_with(tmp_path, transport)
    async def flow():
        task = asyncio.create_task(coordinator.close_ingress("session"))
        await asyncio.sleep(0.01)
        task.cancel()
        await asyncio.sleep(0.01)
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
    asyncio.run(flow())
    assert transport.ingress() == [("session", True), ("session", False)]
    coordinator.close_python_leases()


def test_failed_reopen_does_not_stop_the_others_and_never_fails_a_done_block(tmp_path, monkeypatch):
    transport = Transport()
    coordinator, _ = coordinator_with(tmp_path, transport)
    original = transport.op
    async def op(descriptor, command, operation_id, clock):
        if command["name"] == "session" and not command["closed"]:
            raise RustOpError("x", 503, "queue_io")
        return await original(descriptor, command, operation_id, clock)
    transport.op = op
    async def ok_block():
        async with coordinator._ingress_closed("session", "novo"):
            pass
    async def bad_block():
        async with coordinator._ingress_closed("session", "novo"):
            raise ValueError("corpo")
    from app import diag
    logged = []
    monkeypatch.setattr(diag, "registrar", lambda event, level, **fields: logged.append((event, fields["codigo"])))
    asyncio.run(ok_block())         # o bloco deu certo: a reabertura que falha só vai ao diário
    assert ("novo", False) in transport.ingress()
    assert logged == [("runtime.ingress_reopen_failed", "queue_io")]
    with pytest.raises(ValueError):                 # a falha do corpo não é mascarada
        asyncio.run(bad_block())
    coordinator.close_python_leases()
