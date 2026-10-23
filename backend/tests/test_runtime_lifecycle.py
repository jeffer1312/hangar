import asyncio
import copy
from types import SimpleNamespace

import pytest
from fastapi import HTTPException, Response

from app import api, runtime_coordinator
from app.runtime_coordinator import Binding, Phase, RuntimeCoordinator, WriterLease


def owner(tmp_path, monkeypatch):
    target = Binding("session", "key", "claude", True, {"key":"key", "session_id":"before"},
        str(tmp_path / "before.jsonl"), tmp_path / "projection", tmp_path / "state", tmp_path / "lease", 1)
    class Legacy:
        def binding(self, name, provider):
            return copy.deepcopy(target)
        async def reconnect(self, descriptor, carry):
            return {"hydrated":True}
        async def quiesce(self, descriptor):
            return {"runtime_state":{}}
    coordinator = RuntimeCoordinator(legacy=Legacy())
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    slot = coordinator.register(target)
    return coordinator, slot, target


def test_clear_changes_conversation(tmp_path, monkeypatch):
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    async def change():
        target.meta["session_id"] = "after"
        target.jsonl = str(tmp_path / "after.jsonl")
    try:
        asyncio.run(coordinator.change("session", change))
        assert slot.binding.generation == 2
        assert slot.binding.meta["session_id"] == "after"
        assert slot.store.state["generation"] == 2
    finally:
        coordinator.close_python_leases()


def test_terminal_switch_keeps_managed_queue(tmp_path, monkeypatch):
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    async def change():
        target.headless = False
        target.meta["headless"] = False
    try:
        asyncio.run(coordinator.change("session", change))
        assert coordinator.managed_queue("session")
        assert not coordinator.managed_runtime("session")
        assert slot.lease is not None and not slot.lease.closed
        saved = slot.store.state["runtime_state"]["_binding"]
        assert saved["headless"] is False
        assert saved["key"] == "key"
    finally:
        coordinator.close_python_leases()


def test_stop_failure_keeps_files(tmp_path, monkeypatch):
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    before = slot.binding.state_path.read_bytes()
    async def stop():
        raise OSError("processo ainda vivo")
    try:
        with pytest.raises(OSError):
            asyncio.run(coordinator.change("session", stop, remove=True))
        assert coordinator.managed_queue("session")
        assert slot.binding.state_path.read_bytes() == before
        assert slot.lease is not None and not slot.lease.closed
    finally:
        coordinator.close_python_leases()


def test_history_projection_error_not_304(monkeypatch):
    class Coordinator:
        def managed_queue(self, name):
            return True
        async def op(self, *args):
            raise OSError("projection")
    monkeypatch.setattr(runtime_coordinator, "_current", Coordinator())
    async def info(name):
        return SimpleNamespace(jsonl="chat.jsonl", provider="claude")
    monkeypatch.setattr(api, "_cached_info", info)
    request = SimpleNamespace(headers={"if-none-match":"old"})
    with pytest.raises(HTTPException) as error:
        asyncio.run(api.history(request, Response(), "session"))
    assert error.value.status_code == 503


def test_lifecycle_cancellation_joins_change(tmp_path, monkeypatch):
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    async def scenario():
        started, release = asyncio.Event(), asyncio.Event()
        async def change():
            started.set()
            await release.wait()
        task = asyncio.create_task(coordinator.change("session", change))
        await started.wait()
        task.cancel()
        await asyncio.sleep(0)
        assert slot.frozen
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert slot.binding.generation == 2
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def test_mode_keeps_original_key_when_returning_to_headless(tmp_path, monkeypatch):
    from app.adapters.claude_headless import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    meta = sessions.save("session", str(tmp_path), "sid", key="same-key")
    assert meta["key"] == "same-key"
    assert sessions.load("session")["key"] == "same-key"


def test_combined_supervisor_stop_deactivates_without_recover(monkeypatch):
    # A parada desliga as pontes e deixa as sessões sem dono; nada volta ao Python aqui.
    from app import rust_server
    events = []
    class Proc:
        stdin = None
        runtime_containment = SimpleNamespace(cleaned=True)
        def poll(self):
            return 0
    class Coordinator:
        transport = None
        slots = {"key":SimpleNamespace(binding=SimpleNamespace(name="session"), phase=Phase.Rust)}
        async def enter_pending(self):
            events.append("pending")
        async def recover(self, name, confirmed_dead, containment=None):
            events.append("recover")
    monkeypatch.setattr(runtime_coordinator, "_current", Coordinator())
    monkeypatch.setattr(rust_server.terminal_observer, "configure", lambda *args: events.append("terminal"))
    supervisor = rust_server.Supervisor("fake", "127.0.0.1", 1, 2, "token", "127.0.0.1", lambda: False)
    supervisor.proc = Proc()
    asyncio.run(supervisor.stop())
    assert events == ["terminal", "pending"]


def test_stop_error_is_raised_before_cano_cleanup(monkeypatch):
    from app.adapters.claude_headless import adapter
    if adapter.os.name == "nt":
        pytest.skip("caminho POSIX")
    monkeypatch.setattr(adapter, "_e_cano", lambda pid: True)
    monkeypatch.setattr(adapter.os, "getpgid", lambda pid: pid)
    monkeypatch.setattr(adapter.os, "killpg", lambda *args: (_ for _ in ()).throw(PermissionError("synthetic")))
    with pytest.raises(RuntimeError):
        adapter._matar_grupo(42, "session")


def test_dead_runtime_does_not_re_adopt(tmp_path, monkeypatch):
    from app import rust_server
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    target.meta["cano"] = {"versao":2}
    class Transport:
        instance = "old"
        alive = False
        async def close(self):
            pass
    transport = Transport()
    coordinator.transport, coordinator.instance = transport, "old"
    supervisor = rust_server.Supervisor("fake", "127.0.0.1", 1, 2, "token", "127.0.0.1", lambda: False)
    supervisor.runtime_transport = transport
    async def scenario():
        await supervisor.deactivate_runtime(confirmed_dead=True)
        assert coordinator.transport is None and coordinator.instance is None and coordinator.mode == "pending"
        await supervisor.hand_to_python()
        assert await coordinator.prepare_session("session", "claude")
        assert slot.phase == Phase.Python and not slot.lease.closed
    try:
        asyncio.run(scenario())
    finally:
        coordinator.close_python_leases()


def rust_owner(tmp_path, monkeypatch, state):
    coordinator, slot, target = owner(tmp_path, monkeypatch)
    slot.lease.close()
    slot.lease = None
    slot.phase = Phase.Rust
    slot.view, slot.cache_valid = {}, False
    coordinator.instance, coordinator.mode = "test-instance", "rust"
    calls = []
    snapshot = {"key": target.key, "generation": target.generation, "revision": 1,
                "channels": {}, "error": None,
                "view": {"alive": True, "initialized": True, "in_progress": state == "working",
                         "public_state": {"session": target.name, "state": state, "headless": True}}}
    async def op(descriptor, command, operation_id, clock):
        if command["kind"] == "ingress":
            return {"closed": command["closed"]}
        calls.append(command["kind"])
        assert command["kind"] == "snapshot"
        assert slot.phase == Phase.Rust and slot.lease is None
        assert descriptor["generation"] == target.generation
        return copy.deepcopy(snapshot)
    coordinator.transport = SimpleNamespace(instance=coordinator.instance, op=op)
    return coordinator, slot, calls, snapshot

