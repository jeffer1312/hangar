import asyncio
import subprocess
import sys
from contextlib import contextmanager

import pytest

from app.runtime_coordinator import Binding, RuntimeCoordinator, WriterLease


def binding(tmp_path, headless=True, provider="claude"):
    return Binding(name="session", key="key", provider=provider, headless=headless,
        meta={"key": "key", "cano": {"pid": 42, "escuta": "tcp:127.0.0.1:1", "token": "test", "versao": 2}},
        jsonl=str(tmp_path / "chat.jsonl"), projection_dir=tmp_path / "projection",
        state_path=tmp_path / "key.queue-state.json", lock_path=tmp_path / "key.lock", generation=1)


class Legacy:
    def __init__(self):
        self.events = []

    async def quiesce(self, target):
        self.events.append("quiesce")
        return {"runtime_state": {}, "pending": []}

    async def reconnect(self, target, carry):
        self.events.append("reconnect")
        return {"hydrated": True}


class Gateway:
    instance = "instance-1"

    def __init__(self):
        self.lease = None
        self.alive = True
        self.fail = False

    async def op(self, target, command, operation_id, clock):
        if command["kind"] == "ingress":
            return {"closed": command["closed"]}
        if self.fail:
            raise TimeoutError("synthetic IPC")
        if command["kind"] == "open":
            self.lease = WriterLease(target["lock_path"])
            return {"opened": True, "instance": self.instance, "key": target["key"],
                    "generation": target["generation"], "state": {"alive": True}}
        if command["kind"] == "close":
            self.lease.close()
            self.lease = None
            return {"closed": True}
        return {"accepted": True}


def _owner(tmp_path, gateway):
    target = binding(tmp_path)
    legacy = ReopenLegacy(target)
    coordinator = RuntimeCoordinator(gateway, legacy)
    slot = coordinator.register(target)
    return coordinator, slot, legacy


async def _open(coordinator):
    await coordinator._open_slot_in_rust("session", coordinator.slot("session"), launch=False)


def test_two_processes_one_lease(tmp_path):
    path = tmp_path / "lease"
    lease = WriterLease(path)
    try:
        script = "from app.runtime_coordinator import WriterLease\nimport sys\ntry:\n WriterLease(sys.argv[1])\nexcept BlockingIOError:\n sys.exit(0)\nsys.exit(1)"
        result = subprocess.run([sys.executable, "-c", script, str(path)], timeout=5)
        assert result.returncode == 0
    finally:
        lease.close()


def test_open_releases_python_lease_and_close_returns_it(tmp_path):
    async def flow():
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        await _open(coordinator)
        assert gateway.lease is not None and not coordinator.legacy_allowed("key", 1)
        await coordinator.detach("session", restore=False)
        assert gateway.lease is None and coordinator.legacy_allowed("key", 1)
        assert legacy.events == [], "nem quiesce nem cliente religado"
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_timeout_is_not_fallback(tmp_path):
    async def flow():
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        await _open(coordinator)
        gateway.fail = True
        # Sem resposta do Rust o envio fica incerto (pode estar na fila), nunca passa ao Python.
        result = await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1")
        assert result["disposition"] == "unknown"
        with pytest.raises(RuntimeError):
            await coordinator.recover("session", confirmed_dead=True)
        assert legacy.events == [] and not coordinator.legacy_allowed("key", 1)
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_managed_queue_survives_terminal_mode(tmp_path):
    async def flow():
        coordinator = RuntimeCoordinator()
        target = binding(tmp_path)
        coordinator.register(target)
        async with coordinator.freeze("session"):
            target.headless = False
            coordinator.register(target)
        assert coordinator.managed_queue("session")
        assert not coordinator.managed_runtime("session")
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_late_ready_rejected(tmp_path):
    async def flow():
        gateway = Gateway()
        original = gateway.op
        async def stale(target, command, operation_id, clock):
            result = await original(target, command, operation_id, clock)
            if command["kind"] == "open":
                result["generation"] = 0
            return result
        gateway.op = stale
        coordinator, slot, _ = _owner(tmp_path, gateway)
        with pytest.raises(RuntimeError, match="vida atual"):
            await _open(coordinator)
        assert gateway.lease is None, "a abertura de outra vida é fechada no Rust"
        assert coordinator.legacy_allowed("key", 1) and slot.phase.name == "Python"
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_unconfirmed_close_never_makes_two_owners(tmp_path):
    async def flow():
        gateway = Gateway()
        original = gateway.op
        async def dead_actor(target, command, operation_id, clock):
            if command["kind"] == "open":
                await original(target, command, operation_id, clock)
                raise RuntimeError("IPC recusou a operação (503: runtime_initialize)")
            if command["kind"] == "close":
                raise RuntimeError("IPC recusou a operação (503: runtime_closed)")
            return await original(target, command, operation_id, clock)
        gateway.op = dead_actor
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        # O Rust ainda segura a trava: o Python não assume; a sessão fica com o Rust, vista inválida.
        with pytest.raises(RuntimeError, match="runtime_initialize"):
            await _open(coordinator)
        assert not coordinator.legacy_allowed("key", 1) and slot.phase.name == "Rust" and not slot.cache_valid
        assert legacy.events == []
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_detach_and_lock_before_reserve(tmp_path):
    async def flow():
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        await _open(coordinator)
        original = gateway.op
        interloper = []
        async def detach_then_other_owner(target, command, operation_id, clock):
            result = await original(target, command, operation_id, clock)
            if command["kind"] == "close":
                interloper.append(WriterLease(target["lock_path"]))
            return result
        gateway.op = detach_then_other_owner
        try:
            with pytest.raises(BlockingIOError):
                await coordinator.detach("session")
            assert not coordinator.legacy_allowed("key", 1)
            assert legacy.events == []
        finally:
            for lease in interloper:
                lease.close()
    asyncio.run(flow())


def test_recover_dispatch_as_unknown(tmp_path):
    async def flow():
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        slot.store.exec(1, "prepare", {"monotonic_s": 1, "epoch_s": 1},
                        {"kind": "prepare", "id": "op", "payload": {}, "entry_id": None})
        await _open(coordinator)
        from app.runtime_queue import QueueStore, initial_state
        native_store = QueueStore(slot.binding.state_path, slot.binding.projection_dir,
                                  initial_state("key", 1, "session", []))
        native_store.exec(1, "dispatch", {"monotonic_s": 1, "epoch_s": 1},
                          {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:1"})
        gateway.lease.close()
        gateway.alive = False
        await coordinator.recover("session", confirmed_dead=True)
        assert slot.store.state["operations"]["op"]["status"] == "unknown"
        assert coordinator.legacy_allowed("key", 1)
        coordinator.close_python_leases()
    asyncio.run(flow())


def _python_ops(legacy):
    sent = []
    async def legacy_op(target, command, operation_id):
        sent.append(command["kind"])
        return {"accepted": True, "via": "python"}
    legacy.op = legacy_op
    return sent


def _refusing(gateway, code, fail_times):
    from app.rust_server import RustOpError
    original, attempts = gateway.op, []
    async def op(target, command, operation_id, clock):
        if command["kind"] == "submit":
            attempts.append(operation_id)
            if len(attempts) <= fail_times:
                raise RustOpError(f"IPC recusou a operação (503: {code})", 503, code)
        return await original(target, command, operation_id, clock)
    gateway.op = op
    return attempts


def test_failure_after_possible_effect_is_never_repeated(tmp_path):
    async def flow():
        from app.rust_server import RustOpError
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        sent = _python_ops(legacy)
        attempts = _refusing(gateway, "queue_io", fail_times=99)
        await _open(coordinator)
        with pytest.raises(RustOpError):
            await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1")
        # Uma tentativa só, nada reenviado pelo Python; a sessão segue no Rust.
        assert attempts == ["op-1"] and sent == []
        assert slot.phase.name == "Rust" and not coordinator.legacy_allowed("key", 1)
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_normal_answer_from_rust_is_not_a_failure(tmp_path):
    async def flow():
        from app.rust_server import RustOpError
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        sent = _python_ops(legacy)
        attempts = _refusing(gateway, "claude_command", fail_times=99)
        await _open(coordinator)
        # Ex.: "nenhuma permissão pendente" de um botão velho: sobe como erro, a sessão fica no Rust.
        with pytest.raises(RustOpError):
            await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1")
        assert attempts == ["op-1"] and sent == []
        assert slot.phase.name == "Rust" and not coordinator.legacy_allowed("key", 1)
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_slow_disk_write_does_not_block_the_event_loop(tmp_path, monkeypatch):
    import time
    from app import runtime_queue
    from app.runtime_adapter import LegacyIO

    async def flow():
        coordinator = RuntimeCoordinator(Gateway(), Legacy())
        slot = coordinator.register(binding(tmp_path))
        original = runtime_queue.QueueStore._atomic
        def slow(path, data):
            time.sleep(0.5)
            original(path, data)
        monkeypatch.setattr(runtime_queue.QueueStore, "_atomic", staticmethod(slow))
        write = asyncio.create_task(LegacyIO(coordinator)._exec("session", {"kind": "set_runtime_state", "state": {"a": 1}}))
        await asyncio.sleep(0.1)          # a gravação está no disco, numa thread
        started = time.monotonic()
        with slot.guard:                  # é o que o leitor de eventos faz em cada evento
            pass
        assert time.monotonic() - started < 0.1
        await write
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_dead_cano_after_rust_lets_go_parks_the_session_in_python(tmp_path):
    # O Rust soltou e a trava voltou: sem cano para religar, a sessão não pode ficar presa em
    # "recuperando" recusando tudo; ela fica no Python e sobe de novo no próximo envio.
    async def flow():
        gateway = Gateway()
        coordinator, slot, legacy = _owner(tmp_path, gateway)
        await _open(coordinator)
        async def dead(target, carry):
            raise RuntimeError("reserva não reconectou ao cano existente")
        legacy.reconnect = dead
        await coordinator.detach("session")
        assert coordinator.legacy_allowed("key", 1)
        coordinator.close_python_leases()
    asyncio.run(flow())


# --- Falha vira erro visível, com reabertura única no Rust (dono único, Task 3) ---

class Reopenable(Gateway):
    """Rust falso com snapshot e reabertura: `broken` faz o ator responder em erro até reabrir."""

    def __init__(self, broken=None, reopen_fails=False):
        super().__init__()
        self.kinds, self.broken, self.reopen_fails, self.revision = [], broken, reopen_fails, 6
        self.launches = []

    async def op(self, target, command, operation_id, clock):
        from app.rust_server import RustOpError
        kind = command["kind"]
        if kind == "ingress":
            return {"closed": command["closed"]}
        self.kinds.append(kind)
        if kind == "open" and command.get("launch"):
            self.launches.append(target["name"])
        if kind == "snapshot":
            if self.broken == "gone":
                raise RustOpError("IPC recusou a operação (503: runtime_binding)", 503, "runtime_binding")
            self.revision += 1
            return {"key": "key", "generation": 1, "revision": self.revision, "channels": {}, "error": self.broken,
                    "view": {"alive": self.broken is None, "public_state": {"session": "session", "state": "idle", "headless": True}}}
        if kind == "open" and self.lease is not None:
            if self.reopen_fails:
                raise RustOpError("IPC recusou a operação (503: cano_connect)", 503, "cano_connect")
            self.broken = None
            self.revision += 1
            return {"opened": True, "instance": self.instance, "key": "key", "generation": 1,
                    "state": {"key": "key", "generation": 1, "revision": self.revision, "channels": {}, "error": None,
                              "view": {"alive": True, "public_state": {"session": "session", "state": "idle", "headless": True}}}}
        if kind == "close" and self.lease is not None:
            return {"closed": True}      # a trava do falso fica: o Rust real a reabre na mesma chave
        if kind == "ensure_projection" and self.broken == "gone":
            raise RustOpError("IPC recusou a operação (503: runtime_binding)", 503, "runtime_binding")
        if kind == "submit":
            return {"operation_id": operation_id, "disposition": "accepted", "payload": {}}
        return await super().op(target, command, operation_id, clock)


class ReopenLegacy(Legacy):
    def __init__(self, target):
        super().__init__()
        self.target = target
        self.adapters = {}

    def binding(self, name, provider):
        return self.target


def _rust_session(tmp_path, gateway, provider="claude"):
    target = binding(tmp_path, provider=provider)
    legacy = ReopenLegacy(target)
    sent = _python_ops(legacy)
    coordinator = RuntimeCoordinator(gateway, legacy)
    slot = coordinator.register(target)
    return coordinator, slot, legacy, sent


def test_rust_failure_raises_with_code_and_session_stays_rust(tmp_path):
    async def flow():
        from app.rust_server import RustOpError
        gateway = Gateway()
        coordinator, slot, _, sent = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        attempts = _refusing(gateway, "cano_connect", fail_times=99)
        for n in range(4):
            with pytest.raises(RustOpError) as caught:
                await coordinator.op("session", {"kind": "submit", "text": "Olá"}, f"op-{n}")
            assert caught.value.code == "cano_connect"
        assert attempts == ["op-0", "op-1", "op-2", "op-3"], "uma tentativa por operação"
        assert slot.phase.name == "Rust" and sent == []
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_headless_in_error_reopens_in_rust_once(tmp_path):
    # Codex: o Claude sem terminal o próprio Rust reabre (`request_reopen`), nunca o coordenador.
    async def flow():
        from app.rust_server import RustOpError
        gateway = Reopenable()
        coordinator, slot, legacy, sent = _rust_session(tmp_path, gateway, "codex")
        await _open(coordinator)
        gateway.launches.clear()
        gateway.broken, slot.cache_valid = "cano_closed", False
        assert (await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1"))["disposition"] == "accepted"
        assert gateway.kinds[-4:] == ["snapshot", "close", "open", "submit"]
        assert gateway.launches == ["session"], "o Rust sobe o processo na reabertura"
        assert sent == [] and slot.phase.name == "Rust"
        gateway.kinds.clear()
        gateway.broken, gateway.reopen_fails, slot.cache_valid = "cano_closed", True, False
        with pytest.raises(RustOpError):
            await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-2")
        assert gateway.kinds == ["snapshot", "close", "open"], "uma reabertura; a falha sobe sem enviar"
        assert slot.phase.name == "Rust" and sent == []
        assert slot.view.get("error") == "cano_closed", "o problema fica na tela"
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_history_reopens_dead_session_once(tmp_path):
    async def flow():
        gateway = Reopenable()
        coordinator, slot, legacy, sent = _rust_session(tmp_path, gateway, "codex")
        await _open(coordinator)
        gateway.broken, slot.cache_valid = "gone", False
        await coordinator.op("session", {"kind": "ensure_projection"}, "projection")
        assert gateway.kinds[-4:] == ["snapshot", "close", "open", "ensure_projection"]
        assert sent == []
        from app.rust_server import RustOpError
        gateway.kinds.clear()
        gateway.broken, gateway.reopen_fails, slot.cache_valid = "gone", True, False
        with pytest.raises(RustOpError):
            await coordinator.op("session", {"kind": "ensure_projection"}, "projection-2")
        from app.runtime_adapter import runtime_problem
        assert runtime_problem("session") == ("runtime_falhou", "runtime_binding: ator do runtime ausente")
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_invalid_cache_waits_for_resync(tmp_path):
    async def flow():
        gateway = Reopenable()
        coordinator, slot, _, sent = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        original = gateway.op
        async def silent(target, command, operation_id, clock):
            if command["kind"] == "snapshot":
                raise TimeoutError("canal oscilando")
            return await original(target, command, operation_id, clock)
        gateway.op = silent
        slot.cache_valid = False
        async def resync():
            await asyncio.sleep(0.3)
            slot.cache_valid = True
            coordinator._signal(slot)
        asyncio.create_task(resync())
        result = await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1")
        assert result["disposition"] == "accepted" and sent == []
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_transport_loss_on_send_is_uncertain_not_failed(tmp_path):
    async def flow():
        gateway = Gateway()
        coordinator, slot, _, sent = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        original = gateway.op
        async def dropped(target, command, operation_id, clock):
            if command["kind"] == "submit":
                raise ConnectionResetError("conexão caiu")
            return await original(target, command, operation_id, clock)
        gateway.op = dropped
        result = await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-1")
        assert result["disposition"] == "unknown" and result["payload"]["transport_lost"] is True
        assert slot.phase.name == "Rust" and sent == []
        assert not slot.cache_valid, "a próxima operação relê o estado"
        async def refused(target, command, operation_id, clock):
            raise ConnectionRefusedError("Rust fora do ar")
        gateway.op = refused
        slot.cache_valid = True
        with pytest.raises(ConnectionRefusedError):    # o pedido nem saiu: erro, não incerto
            await coordinator.op("session", {"kind": "submit", "text": "Olá"}, "op-2")
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_problem_event_reaches_session_problem(tmp_path):
    async def flow():
        from app.runtime_adapter import RuntimeAdapter, apply_event
        gateway = Reopenable()
        coordinator, slot, _, _ = _rust_session(tmp_path, gateway)   # `register` o torna o atual
        await _open(coordinator)
        await coordinator.refresh_snapshot("session")
        assert apply_event(slot, {"key": "key", "generation": 1, "revision": slot.view["revision"] + 1, "channel": "problem",
                                  "data": {"error_code": "queue_io", "message": "fila recusou: disco cheio"}})
        state = RuntimeAdapter("claude").snapshot("session")
        assert state.problema == "runtime_falhou"
        assert state.problema_detalhe == "queue_io: fila recusou: disco cheio"
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_snapshot_without_public_state_never_invents_a_state(tmp_path):
    async def flow():
        from app.runtime_adapter import RuntimeAdapter
        gateway = Reopenable()
        coordinator, slot, _, _ = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        await coordinator.refresh_snapshot("session")
        slot.view["view"].pop("public_state", None)     # vista recém-aberta, sem retrato ainda
        try:
            with pytest.raises(RuntimeError, match="snapshot do runtime indisponível"):
                RuntimeAdapter("claude").snapshot("session")
        finally:
            gateway.lease.close()
            coordinator.close_python_leases()
    asyncio.run(flow())


def test_malformed_rate_event_keeps_runtime_state_valid(tmp_path):
    async def flow():
        from app.runtime_adapter import apply_event
        gateway = Reopenable()
        coordinator, slot, _, _ = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        await coordinator.refresh_snapshot("session")
        event = {"key": "key", "generation": 1, "revision": slot.view["revision"] + 1, "channel": "rate",
                 "data": {"tokens": 1000, "seconds": 10.0, "conversation": None}}
        assert apply_event(slot, event)
        assert slot.cache_valid
        gateway.lease.close()
        coordinator.close_python_leases()
    asyncio.run(flow())


def test_background_drain_never_reopens(tmp_path):
    async def flow():
        gateway = Reopenable()
        coordinator, slot, _, sent = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        gateway.broken, slot.cache_valid = "queue_io", False
        gateway.kinds.clear()
        try:
            from app.runtime_coordinator import RustCacheInvalid
            with pytest.raises(RustCacheInvalid):      # recusa na hora, sem esperar nem reabrir
                await coordinator.op("session", {"kind": "drain"}, "drain")
            assert "close" not in gateway.kinds and "open" not in gateway.kinds, "erro persistente não vira laço de reabertura"
            assert sent == []
        finally:
            gateway.lease.close()
            coordinator.close_python_leases()
    asyncio.run(flow())


def test_background_refusal_goes_to_diary_once_per_sequence(tmp_path, monkeypatch):
    from app import diag
    logged = []
    monkeypatch.setattr(diag, "registrar", lambda evento, *a, **k: logged.append((evento, k.get("etapa"))))
    async def flow():
        gateway = Reopenable()
        coordinator, slot, _, _ = _rust_session(tmp_path, gateway)
        await _open(coordinator)
        gateway.broken, slot.cache_valid = "queue_io", False
        try:
            from app.runtime_coordinator import RustCacheInvalid
            for i in range(3):
                with pytest.raises(RustCacheInvalid):
                    await coordinator.op("session", {"kind": "drain"}, f"drain-{i}")
            assert logged.count(("runtime.rust_op_failed", "drain")) == 1, "a cada troca de estado ia mais uma linha"
            coordinator._cache_invalid_logged.discard("session")    # o que a operação que dá certo faz
            with pytest.raises(RustCacheInvalid):
                await coordinator.op("session", {"kind": "drain"}, "drain-next")
            assert logged.count(("runtime.rust_op_failed", "drain")) == 2, "sequência nova volta ao diário"
        finally:
            gateway.lease.close()
            coordinator.close_python_leases()
    asyncio.run(flow())
