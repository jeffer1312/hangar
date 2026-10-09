"""Fachada dos adapters e leitura do estado da mesma chave e geração."""
from __future__ import annotations

import asyncio
import copy
import functools
import annotationlib
import inspect
import uuid
import contextvars
import json
import time
from dataclasses import dataclass

from app import runtime_coordinator
from app.models import StateEvent

_legacy_operation = contextvars.ContextVar("runtime_legacy_operation", default=None)


def assert_legacy(name):
    coordinator = runtime_coordinator.current()
    if coordinator is None or not coordinator.managed_queue(name):
        return
    slot = coordinator.slot(name)
    with slot.guard:
        restoring = coordinator.in_lifecycle(slot) and slot.phase == runtime_coordinator.Phase.RecoveringPython
        normal = slot.phase == runtime_coordinator.Phase.Python and (not slot.frozen or coordinator.in_lifecycle(slot))
        if slot.lease is None or slot.lease.closed or not (normal or restoring):
            raise RuntimeError("Python não possui a sessão; cliente Legacy bloqueado")


@dataclass
class WireTicket:
    name: str
    key: str
    generation: int
    operation_id: str
    phase_id: str
    frame: dict
    aggregate: bool = False
    incomplete: bool = False


class LegacyIO:
    def __init__(self, coordinator):
        self.coordinator = coordinator

    async def _exec(self, name, action, call_id=None):
        coordinator = self.coordinator
        slot = coordinator.slot(name)
        assert_legacy(name)
        generation = slot.binding.generation
        with slot.guard:
            slot.active += 1
        def execute():
            # A trava do slot só cobre a conferência de posse: o laço de eventos também a usa e não
            # pode esperar o disco. A posse não muda no meio: quem a solta (adopt) espera
            # `slot.active` zerar, e a gravação em si é serializada dentro do QueueStore.
            with slot.guard:
                if slot.lease is None or slot.lease.closed or slot.binding.generation != generation:
                    raise RuntimeError("reserva perdeu a posse antes da gravação")
                store = slot.store
            return store.exec(generation, call_id or uuid.uuid4().hex,
                {"monotonic_s":time.monotonic(), "epoch_s":time.time()}, action)
        task = asyncio.create_task(asyncio.to_thread(execute))
        def finished(done):
            with slot.guard:
                slot.active -= 1
            coordinator._signal(slot)
            if not done.cancelled():
                done.exception()
        task.add_done_callback(finished)
        return await asyncio.shield(task)

    async def prepare_wire(self, name, phase, payload, operation_id=None):
        assert_legacy(name)
        slot = self.coordinator.slot(name)
        binding = slot.binding.descriptor()
        context = _legacy_operation.get() or {}
        if context.get("operation_id") not in self.coordinator.legacy_active:
            context = {}
        operation_id = operation_id or context.get("operation_id") or uuid.uuid4().hex
        phase_id = "reserve-wire:" + binding["key"] + ":" + str(binding["generation"]) + ":" + uuid.uuid4().hex
        with slot.guard:
            saved = copy.deepcopy(slot.store.state["operations"].get(operation_id))
        if saved is None:
            intent = context.get("command") or {"operation_id":operation_id, "kind":"legacy", "payload":{"phase":phase}}
            saved = await self._exec(name, {"kind":"prepare", "id":operation_id, "payload":intent, "entry_id":context.get("entry_id")})
        await self._exec(name, {"kind":"prepare", "id":phase_id, "payload":{"logical_id":operation_id,
            "generation":binding["generation"], "frame":payload,
            "request_id":payload.get("id", payload.get("request_id"))}, "entry_id":saved.get("entry_id")})
        from app.runtime_receipt import ReceiptIndex
        conversation = binding["meta"].get("session_id" if binding["provider"] == "claude" else "thread_id") or ""
        is_input = payload.get("type") == "user" or payload.get("method") in {"turn/start", "turn/steer"}
        cursor = (await asyncio.to_thread(ReceiptIndex(binding["provider"], conversation).capture, binding["jsonl"])
            if is_input and binding["jsonl"] else None)
        await self._exec(name, {"kind":"bind_dispatch", "id":phase_id, "cursor":cursor})
        if saved["status"] == "prepared":
            await self._exec(name, {"kind":"bind_dispatch", "id":operation_id, "cursor":cursor})
        await self._exec(name, {"kind":"begin_dispatch", "id":phase_id, "wire_id":phase_id})
        await self._exec(name, {"kind":"begin_dispatch", "id":operation_id, "wire_id":phase_id})
        return WireTicket(name, binding["key"], binding["generation"], operation_id, phase_id, copy.deepcopy(payload), bool(context))

    async def finish_wire(self, ticket, outcome, result=None, *, definitive=False):
        slot = self.coordinator.slot(ticket.name)
        if slot.binding.key != ticket.key or slot.binding.generation != ticket.generation:
            raise RuntimeError("recibo de outra geração")
        status = {"written":"accepted", "not_written":"rejected", "unknown":"unknown"}[outcome]
        record = result if definitive else {"write_outcome":outcome}
        await self._exec(ticket.name, {"kind":"finish", "id":ticket.phase_id, "status":status, "result":record})
        is_request = ticket.frame.get("type") == "control_request" or ticket.frame.get("method") is not None and ticket.frame.get("id") is not None
        if ticket.aggregate and ticket.operation_id in getattr(self.coordinator, "legacy_active", set()):
            return
        if ticket.incomplete and outcome == "written":
            await self._exec(ticket.name, {"kind":"finish", "id":ticket.operation_id, "status":"unknown",
                "result":{"operation_id":ticket.operation_id, "disposition":"unknown", "payload":{"remaining_phase":"effort"}}})
            return
        parent_status = status if definitive or not is_request or outcome != "written" else "unknown"
        await self._exec(ticket.name, {"kind":"finish", "id":ticket.operation_id, "status":parent_status,
            "result":{"operation_id":ticket.operation_id, "disposition":parent_status, "payload":record}})

    async def reply(self, name, endpoint, frame):
        request_id = frame.get("id") if frame.get("type") != "control_response" else (frame.get("response") or {}).get("request_id")
        if type(request_id) not in (int, str):
            raise ValueError("ID da resposta inválido")
        ticket = getattr(endpoint, "runtime_tickets", {}).get((type(request_id), request_id))
        if ticket is None:
            slot = self.coordinator.slot(name)
            with slot.guard:
                phases = tuple(copy.deepcopy(slot.store.state["operations"]).values())
            candidates = [phase for phase in phases if phase["payload"].get("generation") == slot.binding.generation
                and type(phase["payload"].get("request_id")) is type(request_id) and phase["payload"].get("request_id") == request_id
                and phase["payload"].get("frame")]
            if len(candidates) != 1:
                return
            phase = candidates[0]
            ticket = WireTicket(name, slot.binding.key, slot.binding.generation, phase["payload"]["logical_id"], phase["id"], phase["payload"]["frame"])
        with self.coordinator.slot(name).guard:
            parent = copy.deepcopy(self.coordinator.slot(name).store.state["operations"].get(ticket.operation_id))
        incomplete_effort = bool(parent and parent["payload"].get("kind") == "set_model"
            and isinstance(parent["payload"].get("payload", {}).get("effort"), str)
            and ticket.frame.get("request", {}).get("subtype") == "set_model")
        if incomplete_effort and ticket.operation_id not in getattr(self.coordinator, "legacy_active", set()):
            ticket.incomplete = True
        failure = frame.get("error") is not None or (frame.get("response") or {}).get("subtype") == "error"
        result = {"operation_id":ticket.operation_id, "disposition":"rejected" if failure else "accepted", "payload":frame}
        await self.finish_wire(ticket, "not_written" if failure else "written", result, definitive=True)
        future = getattr(endpoint, "runtime_acks", {}).get(ticket.phase_id)
        if future is not None and not future.done():
            future.set_result("written")

    async def finish_call(self, name, context, *, failed=False, deferred=False):
        slot = self.coordinator.slot(name)
        with slot.guard:
            parent = slot.store.state["operations"].get(context["operation_id"])
            phases = [phase for phase in slot.store.state["operations"].values()
                if phase["payload"].get("logical_id") == context["operation_id"]]
        if parent is None:
            return
        uncertain = any(phase["status"] in {"unknown", "dispatching"}
            or phase["result"] == {"write_outcome":"written"} and (phase["payload"].get("frame", {}).get("method") is not None
                or phase["payload"].get("frame", {}).get("type") == "control_request") for phase in phases)
        status = "unknown" if uncertain else "rejected" if failed else "deferred" if deferred else "accepted"
        await self._exec(name, {"kind":"finish", "id":context["operation_id"], "status":status,
            "result":{"operation_id":context["operation_id"], "disposition":status, "payload":{}}})
        if uncertain:
            raise RuntimeError("resultado incerto; diário e entrada foram conservados")

    async def write(self, name, endpoint, writer, frame, version):
        assert_legacy(name)
        ticket = await self.prepare_wire(name, "write", frame)
        future = asyncio.get_running_loop().create_future()
        endpoint.runtime_acks[ticket.phase_id] = future
        endpoint.runtime_tickets = getattr(endpoint, "runtime_tickets", {})
        request_id = frame.get("id", frame.get("request_id"))
        if request_id is not None:
            endpoint.runtime_tickets[(type(request_id), request_id)] = ticket
        try:
            assert_legacy(name)
            envelope = {"type":"cano_input", "operation_id":ticket.phase_id, "frame":json.dumps(frame)} if version == 2 else frame
            writer.write((json.dumps(envelope) + "\n").encode())
            await writer.drain()
            outcome = await asyncio.wait_for(asyncio.shield(future), 30) if version == 2 else "unknown"
        except asyncio.CancelledError:
            await self.finish_wire(ticket, "unknown")
            raise
        except Exception:
            with self.coordinator.slot(name).guard:
                phase = copy.deepcopy(self.coordinator.slot(name).store.state["operations"].get(ticket.phase_id))
            await self.finish_wire(ticket, "unknown")
            if not (phase and isinstance(phase.get("result"), dict) and phase["result"].get("disposition") in {"accepted", "rejected"}):
                raise RuntimeError("escrita incerta; operação conservada sem reenvio") from None
            outcome = "written"
        finally:
            endpoint.runtime_acks.pop(ticket.phase_id, None)
        await self.finish_wire(ticket, outcome)
        if version != 2:
            return ticket
        if outcome != "written":
            raise RuntimeError("entrada não confirmada pelo cano; diário conservado")
        return ticket


# Teto da espera pela escrita em voo do drain na passagem ao Rust; o ack chega em milissegundos.
_WRITE_WAIT_S = 10.0


class LegacyBridge:
    def __init__(self, coordinator, adapters):
        self.coordinator, self.adapters = coordinator, adapters
        self.receipts = {}

    def binding(self, name, provider):
        from app.pqueue import _queue_dir
        if provider == "claude":
            from app.adapters.claude_headless import sessions
        elif provider == "codex":
            from app.adapters.codex import sessions
        else:
            return None
        meta = sessions.load(name)
        if not meta or not meta.get("key"):
            if provider == "claude":
                from app.runtime_terminal import resolve_binding
                previous = (self.coordinator.slot(name).binding if self.coordinator.managed_queue(name) else
                    next((slot.binding for slot in self.coordinator.slots.values()
                        if self.coordinator.in_lifecycle(slot) and slot.binding.provider == provider), None))
                return resolve_binding(name, previous)
            if self.coordinator.managed_queue(name) and not self.coordinator.slot(name).binding.headless:
                return copy.deepcopy(self.coordinator.slot(name).binding)
            return None
        directory = _queue_dir()
        state_path = directory / "runtime" / (meta["key"] + ".json")
        headless = bool(meta.get("headless"))
        if provider == "claude" and not headless:
            from app.runtime_terminal import resolve_binding
            previous = self.coordinator.slots.get(meta["key"])
            return resolve_binding(name, previous.binding if previous else None)
        if not headless and not state_path.exists():
            return None
        path = self.adapters[provider].transcript_path_de(meta) if provider == "claude" else meta.get("rollout_path") or ""
        if provider == "codex" and not path and meta.get("thread_id"):
            from app.adapters.codex.sem_terminal import rollout_de
            path = rollout_de(meta["thread_id"], meta.get("codex_home"))
            current = sessions.load(name)
            if path and current and current.get("key") == meta["key"] and current.get("thread_id") == meta["thread_id"]:
                meta = sessions.update(name, rollout_path=path) or meta
        generation = (self.coordinator.slots[meta["key"]].binding.generation
            if meta["key"] in self.coordinator.slots else
            json.loads(state_path.read_bytes())["generation"] if state_path.exists() else 1)
        return runtime_coordinator.Binding(name, meta["key"], provider, headless, meta, path,
            directory, state_path, directory / "runtime" / (meta["key"] + ".lock"), generation)

    async def quiesce(self, descriptor):
        if descriptor["meta"].get("terminal") or descriptor["meta"].get("pending_terminal"):
            from app.runtime_terminal import quiesce
            return await quiesce(self.coordinator, descriptor)
        name, provider = descriptor["name"], descriptor["provider"]
        adapter = self.adapters[provider]
        slot = self.coordinator.slots[descriptor["key"]]
        sess = adapter._sessions.get(name)
        if provider == "codex" and sess is not None and sess["client"].tem_processo_proprio:
            raise RuntimeError("reserva headless encontrou processo próprio; transferência recusada")
        adapter._sessions.pop(name, None)
        tasks = []
        if provider == "claude" and (claim := adapter.drain_claims.get(name)) is not None:
            # O drain para antes (a escrita dele é protegida): não pega a próxima nem reverte esta.
            claim["task"].cancel()
            await asyncio.gather(claim["task"], return_exceptions=True)
            # Com o stdin e o leitor vivos, a escrita em voo recebe o ack do cano.
            if (write := claim.get("write")) is not None and not write.done():
                await asyncio.wait({write}, timeout=_WRITE_WAIT_S)
                if not write.done():
                    write.cancel()
                    await asyncio.gather(write, return_exceptions=True)
        if provider == "claude" and sess is not None:
            sess.desligando = True
            sess.live_active = lambda: False
            if sess.drenador is not None:
                tasks.append(sess.drenador)
            for task in tuple(adapter._tarefas):
                frame = getattr(task.get_coro(), "cr_frame", None)
                if frame is not None and (frame.f_locals.get("sess") is sess or frame.f_locals.get("name") == name):
                    tasks.append(task)
            if sess.proc is not None:
                sess.proc.saiu(None)
                sess.proc.stdin.close()
                await sess.proc.stdin.wait_closed()
            if sess.leitor is not None:
                tasks.append(sess.leitor)
        elif provider == "codex" and sess is not None:
            for collection in (adapter._subscribers, adapter._tmux_watchers):
                if task := collection.pop(name, None):
                    tasks.append(task)
            if task := sess.get("bomba"):
                tasks.append(task)
            await sess["client"].close(strict=True)
        for task in tasks:
            if task is not asyncio.current_task():
                task.cancel()
        results = await asyncio.gather(*(task for task in tasks if task is not asyncio.current_task()), return_exceptions=True)
        for result in results:
            if isinstance(result, Exception):
                raise RuntimeError("cliente antigo não encerrou normalmente") from result
        claim = adapter.drain_claims.get(name) if provider == "claude" else None
        if claim is not None and claim["task"].done():
            adapter.drain_claims.pop(name)
            write = claim.get("write")
            if write is None:
                # Drain cancelado entre reivindicar e escrever: sem devolver, o Rust adota a fila
                # com a entrada marcada entregue e ela nunca sai.
                action, code = {"kind":"set_delivered", "entry_id":claim["id"], "value":False}, None
            elif write.done() and not write.cancelled() and write.exception() is None:
                action, code = None, None
            else:
                # Sem ack não se sabe se saiu: repetir pode duplicar, "entregue" pode ser perda.
                # Desistida, a bolha mostra que não chegou e a reconciliação desfaz se ela aparecer.
                action, code = {"kind":"abandon", "entry_id":claim["id"]}, "teto" if write.cancelled() else "falhou"
            try:
                if action is not None:
                    with slot.guard:
                        row = next((r for r in slot.store.state["rows"] if r.get("id") == claim["id"]), None)
                        # Confirmada, desistida ou já devolvida: outro caminho tratou a entrada.
                        pending = (row is not None and row.get("delivered") is True
                            and not row.get("confirmed") and not row.get("desistiu"))
                        if pending:
                            slot.store.exec(descriptor["generation"], "quiesce-claim:" + uuid.uuid4().hex,
                                runtime_coordinator._clock(), action)
                    from app import diag
                    if not pending:
                        diag.registrar("runtime.unclaim_skipped", "aviso", sessao=name,
                            codigo="sem_linha" if row is None else "ja_tratada")
                    elif code is not None:
                        diag.registrar("runtime.write_uncertain", "erro", sessao=name, codigo=code)
            except Exception as exc:
                from app import diag
                diag.registrar("runtime.unclaim_failed", "erro", sessao=name, **runtime_coordinator.failure_reason(exc))
        if provider == "claude" and sess is not None:
            await asyncio.gather(sess.preview_buffer.discard(), sess.thinking_buffer.discard(), sess.tool_buffer.discard())

    async def reconnect(self, descriptor, carry):
        if descriptor["meta"].get("terminal"):
            from app.runtime_terminal import reconnect
            return await reconnect(self.coordinator, descriptor, carry)
        name, provider = descriptor["name"], descriptor["provider"]
        adapter = self.adapters[provider]
        existing = adapter._sessions.get(name)
        if provider == "claude" and existing is not None and existing.vivo:
            return {"hydrated":True}
        if provider == "codex" and existing is not None and not existing["client"].closed:
            return {"hydrated":True}
        if provider == "claude":
            original = inspect.unwrap(adapter.ensure_running)
            sess = await original(adapter, name, so_reconectar=True)
            if sess is None or sess.proc is None:
                raise RuntimeError("reserva não reconectou ao cano existente")
        else:
            from app.adapters.codex import sessions
            meta = sessions.load(name)
            if not meta or meta.get("key") != descriptor["key"]:
                raise RuntimeError("sidecar mudou durante a recuperação")
            client = await adapter._ligar_sem_terminal(name, meta, reabrir=False)
            if client is None or client.closed:
                raise RuntimeError("reserva não reconectou ao cano existente")
        view = carry.get("runtime_state") or {}
        conversation = descriptor["meta"].get("session_id" if provider == "claude" else "thread_id")
        if view.get("conversation", view.get("thread_id")) in {None, conversation}:
            sess = adapter._sessions[name]
            if provider == "claude":
                for source, target in {"model":"model", "effort":"effort", "permission_mode":"permission_mode",
                    "previous_non_plan":"modo_nao_plan", "usage":"usage", "context_window":"context_window", "cost":"cost",
                    "commands":"comandos"}.items():
                    if source in view:
                        setattr(sess, target, copy.deepcopy(view[source]))
                if "terminal_commands" in view:
                    sess.comandos_terminal = frozenset(view["terminal_commands"] or [])
            else:
                for field in ("model", "effort", "mode", "token_usage", "rate_limits"):
                    if field in view:
                        sess[field] = copy.deepcopy(view[field])
                if "async_questions" in view:
                    from collections import Counter
                    questions = sess["async_questions"]
                    questions._pending = dict(copy.deepcopy(view["async_questions"]))
                    questions._seen = set(view.get("async_seen") or [])
                    questions._resolved = set(view.get("async_resolved") or [])
                    questions.skipped = set(view.get("skipped_async_questions") or [])
                    questions._local_answers = copy.deepcopy(view.get("async_local_answers") or {})
                    questions._echoes = Counter(view.get("async_echoes") or {})
                    questions._during_load = copy.deepcopy(view.get("async_during_load"))
        return {"hydrated":True}

    async def op(self, descriptor, command, operation_id):
        if descriptor["meta"].get("terminal"):
            from app.runtime_terminal import reserve_op
            return await reserve_op(self.coordinator, descriptor, command, operation_id)
        from app.adapters.codex.appserver import RequestOutcomeUnknown
        name, provider = descriptor["name"], descriptor["provider"]
        adapter, io = self.adapters[provider], LegacyIO(self.coordinator)
        kind = command["kind"]
        if kind == "ensure_projection":
            return await io._exec(name, {"kind":"ensure_projection"})
        if kind == "queue":
            return await io._exec(name, command["action"], operation_id)
        if kind == "snapshot":
            with self.coordinator.slot(name).guard:
                return copy.deepcopy(self.coordinator.slot(name).view)
        if kind == "confirm":
            return await self.confirm(descriptor)
        if kind == "drain":
            rows = await io._exec(name, {"kind":"claim", "min_ts":descriptor["meta"].get("created", 0), "limit":1, "entry_id":None})
            sent = 0
            for row in rows:
                reply = await self.op(descriptor, {"kind":"submit", "text":row["text"], "pre_transcript":row.get("pre_transcript", False)}, row["id"])
                sent += reply["disposition"] == "accepted"
            return {"sent":sent}
        if kind == "submit":
            method = "steer" if command.get("steer") else "send_prompt"
            payload = {"text":command["text"], "pre_transcript":command.get("pre_transcript", False)}
            rows = await io._exec(name, {"kind":"load"})
            if not any(row["id"] == operation_id for row in rows):
                await io._exec(name, {"kind":"append", "text":payload["text"], "delivered":False, "ts":None,
                    "pre_transcript":payload["pre_transcript"], "entry_id":operation_id})
            control_kind = "steer" if command.get("steer") else "input"
            entry_id = operation_id
        elif kind == "control":
            control_kind, payload, entry_id = command["control"], command.get("payload") or {}, None
            method = {"answer_questions":"answer_questions", "set_model":"set_model", "set_effort":"set_model",
                "set_service_tier":"set_service_tier",
                "set_permission_mode":"set_permission_mode", "list_models":"list_models", "list_skills":"list_skills",
                "read_rate_limits":"read_rate_limits", "read_settings":"read_settings", "interrupt":"interrupt",
                "select":"select", "compact":"compact", "skip_question":"skip_question", "set_mode":"set_mode"}.get(control_kind)
            if method is None:
                raise RuntimeError("controle não disponível na reserva")
            if provider == "codex" and control_kind == "set_permission_mode":
                method = "set_permission_mode_sem_terminal"
        else:
            raise RuntimeError("operação não disponível na reserva")
        intent = {"operation_id":operation_id, "kind":control_kind, "payload":payload}
        parent = await io._exec(name, {"kind":"prepare", "id":operation_id, "payload":intent, "entry_id":entry_id})
        if parent["status"] in {"accepted", "confirmed", "rejected", "unknown", "dispatching"}:
            if parent["status"] in {"unknown", "dispatching"}:
                return {"operation_id":operation_id, "disposition":"unknown", "payload":{}}
            return parent["result"]
        context = {"key":descriptor["key"], "generation":descriptor["generation"], "operation_id":operation_id,
            "entry_id":entry_id, "command":intent}
        token = _legacy_operation.set(context)
        self.coordinator.legacy_active.add(operation_id)
        try:
            original = inspect.unwrap(getattr(adapter, method))
            if inspect.ismethod(original):
                original = original.__func__
            arguments = {key:value for key,value in payload.items() if key in inspect.signature(original).parameters}
            if method == "set_model":
                arguments.setdefault("model", None)
                arguments.setdefault("effort", None)
            if provider == "codex" and control_kind == "set_permission_mode":
                arguments["modo"] = payload["mode"]
            if provider == "codex" and kind == "submit":
                await io._exec(name, {"kind": "set_delivered", "entry_id": entry_id, "value": True})
            result = await original(adapter, name, **arguments)
            if provider == "codex" and method == "send_prompt" and result == "unknown":
                reply = {"operation_id": operation_id, "disposition": "unknown",
                         "payload": {"transport_lost": True}}
                await io._exec(name, {"kind": "finish", "id": operation_id,
                                     "status": "unknown", "result": reply})
                return reply
            if method == "set_service_tier":
                result = {"service_tier": result}
            await io.finish_call(name, context, deferred=result == "deferred")
            disposition = "deferred" if result == "deferred" else "accepted"
            reply = {"operation_id":operation_id, "disposition":disposition,
                "payload":result if isinstance(result, (dict, list)) else {}}
            await io._exec(name, {"kind":"finish", "id":operation_id, "status":disposition, "result":reply})
            if disposition == "deferred" and entry_id is not None:
                await io._exec(name, {"kind":"set_delivered", "entry_id":entry_id, "value":False, "steered":False})
            return reply
        except (asyncio.CancelledError, RequestOutcomeUnknown):
            if provider == "codex" and kind == "submit":
                await io._exec(name, {"kind": "finish", "id": operation_id, "status": "unknown",
                    "result": {"operation_id": operation_id, "disposition": "unknown",
                               "payload": {"transport_lost": True}}})
            else:
                await io.finish_call(name, context, failed=True)
            raise
        except BaseException:
            await io.finish_call(name, context, failed=True)
            if provider == "codex" and kind == "submit":
                await io._exec(name, {"kind": "set_delivered", "entry_id": entry_id, "value": False})
            raise
        finally:
            self.coordinator.legacy_active.discard(operation_id)
            _legacy_operation.reset(token)

    async def confirm(self, descriptor):
        from app.runtime_receipt import ReceiptIndex
        slot = self.coordinator.slot(descriptor["name"])
        conversation = descriptor["meta"].get("session_id" if descriptor["provider"] == "claude" else "thread_id") or ""
        if not descriptor["jsonl"]:
            return {"confirmed":0}
        # Um índice por conversa: ele lê só o que o transcript ganhou desde a última confirmação.
        key = (descriptor["key"], descriptor["provider"], conversation)
        index = self.receipts.get(key)
        if index is None:
            self.receipts = {k: v for k, v in self.receipts.items() if k[0] != descriptor["key"]}
            index = self.receipts[key] = ReceiptIndex(descriptor["provider"], conversation)
        io = LegacyIO(self.coordinator)

        def pending():
            with slot.guard:
                state = slot.store.state
                rows = {row["id"]: copy.deepcopy(row) for row in state["rows"] if not row.get("confirmed")}
                candidates = [(operation_id, copy.deepcopy(operation["dispatch_cursor"]), rows[operation["entry_id"]])
                    for operation_id, operation in state["operations"].items()
                    if operation["status"] != "confirmed" and operation["payload"].get("kind") in {"input", "steer"}
                    and operation.get("dispatch_cursor") and operation.get("entry_id") in rows]
                return candidates, copy.deepcopy(state["used_occurrences"])
        candidates, used = pending()
        if not candidates:
            return {"confirmed":0}
        await asyncio.to_thread(index.scan, descriptor["jsonl"])
        count = 0
        for operation_id, cursor, row in candidates:
            proof = await asyncio.to_thread(index.match_after, cursor, row, used)
            if proof and await io._exec(descriptor["name"], {"kind":"confirm_occurrence", "id":operation_id, "proof":proof}) is True:
                count += 1
                used = pending()[1]
        return {"confirmed":count}

def accept_ack(endpoint, event):
    phase_id, outcome = event.get("operation_id"), event.get("outcome")
    if not isinstance(phase_id, str) or outcome not in {"written", "not_written", "unknown"}:
        raise ValueError("ACK do cano inválido")
    future = getattr(endpoint, "runtime_acks", {}).get(phase_id)
    if future is not None and not future.done():
        future.set_result(outcome)


def bind_client(name, client):
    coordinator = runtime_coordinator.current()
    if coordinator is None or not coordinator.managed_queue(name):
        return
    assert_legacy(name)
    slot = coordinator.slot(name)
    client.runtime_owner = (name, slot.binding.key, slot.binding.generation)


def runtime_data(name):
    slot = native_slot(name)
    return RuntimeAdapter(slot.binding.provider).view(name).data if slot is not None else None


def registry_method(original):
    # Só para casar argumentos: avaliar as anotações dentro da classe resolve `list` como o método
    # `SessionRegistry.list`, e `list[dict]` numa assinatura derrubava a importação do registry.
    signature = inspect.signature(original, annotation_format=annotationlib.Format.FORWARDREF)
    @functools.wraps(original)
    def wrapper(self, *args, **kwargs):
        arguments = signature.bind(self, *args, **kwargs).arguments
        name = arguments.get("name", arguments.get("old"))
        coordinator = runtime_coordinator.current()
        if (coordinator is None or not coordinator.managed_queue(name)
                or coordinator.in_lifecycle(coordinator.slot(name))):
            return original(self, *args, **kwargs)
        async def action():
            return await asyncio.to_thread(original, self, *args, **kwargs)
        return run_sync(lambda: coordinator.change(name, action,
            new_name=arguments.get("new"), advance=original.__name__ != "rename",
            remove=original.__name__ == "kill"), coordinator.loop)
    return wrapper


def run_sync(factory, loop):
    try:
        running = asyncio.get_running_loop()
    except RuntimeError:
        running = None
    if loop is None or not loop.is_running() or running is loop:
        raise RuntimeError("operação síncrona exige o pool e o loop do servidor")
    future = asyncio.run_coroutine_threadsafe(factory(), loop)
    try:
        # As operações esperam o desfecho do Rust (até `PENDING_WAIT_S`) antes do próprio prazo.
        return future.result(timeout=185 + runtime_coordinator.PENDING_WAIT_S)
    except BaseException:
        future.cancel()
        raise


@dataclass(frozen=True)
class RuntimeView:
    key: str
    generation: int
    revision: int
    data: dict

    @property
    def thread_id(self):
        return self.data.get("thread_id")


def native_slot(name):
    coordinator = runtime_coordinator.current()
    if coordinator is None or not coordinator.managed_runtime(name):
        return None
    slot = coordinator.slot(name)
    if not slot.binding.headless:
        return None
    if slot.phase == runtime_coordinator.Phase.Python:
        return None
    if slot.phase != runtime_coordinator.Phase.Rust:
        raise runtime_coordinator.TransferInProgress("sessão em transferência; aguarde a posse ser confirmada")
    return slot


def _problem_text(data):
    message = data.get("message")
    return (f"{data['error_code']}: {message}" if isinstance(message, str) and message else data["error_code"])[:300]


# Entrada que o terminal recusa sem escrever, pelo código do escritor Rust: cada uma tem frase na tela.
_STALLED_INPUT = {"composer_busy": "terminal_input_composer_busy",
    "composer_unreadable": "terminal_input_composer_unreadable",
    "capture_failed": "terminal_input_capture_failed", "capture_utf8": "terminal_input_capture_failed",
    "clear_not_applied": "terminal_clear_not_applied"}


def runtime_problem(name):
    """Problema publicado pelo Rust para a sessão: `("runtime_falhou", "<código>: <frase>")`, a entrada
    parada (`("terminal_input_…", "<código>")`) ou None."""
    coordinator = runtime_coordinator.current()
    slot = coordinator.slots.get(coordinator.names.get(name, "")) if coordinator is not None else None
    if slot is None or slot.phase != runtime_coordinator.Phase.Rust:
        return None
    view = slot.view or {}
    if view.get("problem"):
        return "runtime_falhou", view["problem"]
    stalled = (view.get("view") or {}).get("input_stalled")
    if isinstance(stalled, str) and stalled:
        return _STALLED_INPUT.get(stalled, "terminal_input_stalled"), stalled[:60]
    return None


def apply_event(slot, event):
    if (not isinstance(event, dict) or set(event) != {"key", "generation", "revision", "channel", "data"}
            or event["key"] != slot.binding.key or type(event["generation"]) is not int
            or event["generation"] != slot.binding.generation or type(event["revision"]) is not int
            or event["revision"] < 0 or not isinstance(event["channel"], str)):
        return False
    cached = slot.view
    previous = cached.get("revision", -1)
    channel, data, revision = event["channel"], event["data"], event["revision"]
    terminal = slot.binding.meta.get("terminal")
    if channel == "snapshot":
        if (not isinstance(data, dict) or data.get("key") != slot.binding.key
                or data.get("generation") != slot.binding.generation or data.get("revision") != revision
                or not isinstance(data.get("view"), dict) or not isinstance(data.get("channels"), dict)):
            return False
        if terminal:
            view = data["view"]
            if (view.get("terminal") is not True or view.get("conversation") != terminal["conversation"]
                    or type(view.get("deliverable")) is not bool or "public_state" in view
                    or data["channels"] or data.get("error") is not None and not isinstance(data["error"], str)
                    or view.get("input_stalled") is not None and not isinstance(view["input_stalled"], str)):
                return False
            stalled = view.get("input_stalled")
            if stalled and stalled != ((cached.get("view") or {}).get("input_stalled")) and revision >= previous:
                # Uma linha por série: o Rust só marca depois do teto, e repete o código enquanto durar.
                from app import diag
                diag.registrar("terminal.input_stalled", "aviso", sessao=slot.binding.name, codigo=stalled[:60])
        else:
            try:
                StateEvent.model_validate(data["view"]["public_state"])
            except (KeyError, ValueError):
                return False
        if revision < previous:
            return True
        slot.view = copy.deepcopy(data)
        if data.get("error") is not None:
            # A frase veio no `problem`; o snapshot só traz o código.
            same = (cached.get("problem") or "").split(":", 1)[0] == data["error"]
            slot.view["problem"] = cached["problem"] if same else data["error"]
        slot.cache_valid = data.get("error") is None
        return True
    if revision <= previous:
        return True
    if terminal:
        if channel != "problem" or not isinstance(data, dict) or not isinstance(data.get("error_code"), str):
            return False
        if revision != previous + 1:
            slot.cache_valid = False
            return False
        slot.view = {**copy.deepcopy(cached), "revision": revision, "error": data["error_code"],
                     "problem": _problem_text(data)}
        slot.cache_valid = False
        return True
    if not getattr(slot, "cache_valid", False) or revision != previous + 1:
        slot.cache_valid = False
        return False
    updated = copy.deepcopy(cached)
    try:
        if channel == "view":
            if not isinstance(data, dict):
                raise ValueError("view inválida")
            StateEvent.model_validate(data["public_state"])
            updated["view"] = copy.deepcopy(data)
        elif channel == "state":
            StateEvent.model_validate(data)
            updated["view"]["public_state"] = copy.deepcopy(data)
        elif channel in {"preview", "thinking", "tool"}:
            if (not isinstance(data, dict) or not isinstance(data.get("text"), str)
                    or data.get("full") is not True or data.get("md") is not True):
                raise ValueError("prévia inválida")
            updated.setdefault("channels", {})[channel] = copy.deepcopy(data)
        elif channel == "problem":
            if not isinstance(data, dict) or not isinstance(data.get("error_code"), str):
                raise ValueError("falha inválida")
            updated["error"] = data["error_code"]
            updated["problem"] = _problem_text(data)
            slot.cache_valid = False
        elif channel in {"voice", "voice_target"}:
            if not isinstance(data, dict) or not isinstance(data.get("event"), dict):
                raise ValueError("evento de voz inválido")
        elif channel == "rate":
            pass   # Não toca no estado: medida ruim é descartada pelo coordenador, sem invalidar o cache.
        else:
            raise ValueError("canal inválido")
    except (KeyError, ValueError):
        slot.cache_valid = False
        return False
    updated["revision"] = revision
    slot.view = updated
    return True


class RuntimeAdapter:
    def __init__(self, provider):
        self.provider = provider

    def view(self, name, *, mutating=False):
        slot = native_slot(name)
        if slot is None:
            raise RuntimeError("sessão fora da posse Rust")
        cached = slot.view
        if cached.get("key") != slot.binding.key or cached.get("generation") != slot.binding.generation:
            raise RuntimeError("estado de outra geração")
        if mutating and not getattr(slot, "cache_valid", False):
            raise RuntimeError("estado do runtime indisponível; aguarde a reposição")
        data = cached.get("view")
        if not isinstance(data, dict):
            raise RuntimeError("snapshot do runtime indisponível")
        return RuntimeView(slot.binding.key, slot.binding.generation, cached.get("revision", 0), copy.deepcopy(data))

    async def control(self, name, kind, payload=None, *, operation_id=None, allow_deferred=False):
        self.view(name, mutating=True)
        reply = await runtime_coordinator.current().op(name, {"kind":"control", "control":kind,
            "payload":payload or {}}, operation_id or uuid.uuid4().hex)
        if reply.get("disposition") != "accepted":
            if allow_deferred and reply.get("disposition") == "deferred":
                return {"_runtime_deferred":True}
            if reply.get("disposition") == "unknown":
                raise RuntimeError("resultado incerto; a operação foi conservada sem reenvio")
            raise ValueError((reply.get("payload") or {}).get("error") or "operação recusada pelo runtime")
        return reply.get("payload")

    async def ensure_running(self, name, **kwargs):
        view = self.view(name, mutating=True)
        if not view.data.get("alive"):
            raise RuntimeError("cano encerrado; recuperação requer ação explícita")
        return view

    async def list_models(self, name):
        payload = await self.control(name, "list_models")
        # O Claude responde `{"models": [...]}`; o motor Codex já devolve a lista.
        models = payload.get("models") if isinstance(payload, dict) else payload
        if not isinstance(models, list) or not models:
            raise RuntimeError("o runtime não devolveu a lista de modelos")
        return models

    def current_model(self, name):
        data = self.view(name).data
        return {"model":data.get("model"), "effort":data.get("effort"),
            **({"service_tier":data.get("service_tier")} if self.provider == "codex" else {})}

    def escolhas(self, name):
        data = self.current_model(name)
        return data["model"], data["effort"]

    def snapshot(self, name, thread_id=None):
        view = self.view(name)
        if thread_id is not None and view.thread_id != thread_id:
            raise RuntimeError("snapshot de outra conversa")
        public = view.data.get("public_state")
        if public is None:
            # Logo após a subida a vista ainda não tem o retrato. Estado inventado (idle) enganaria a lista e o
            # push; a falha deixa a lista do Rust com o último valor bom, como a vista sem snapshot.
            raise RuntimeError("snapshot do runtime indisponível")
        state = StateEvent.model_validate(public)
        slot = runtime_coordinator.current().slot(name)
        if problem := runtime_problem(name):
            state = state.model_copy(update={"problema":problem[0], "problema_detalhe":problem[1]})
        elif not slot.cache_valid:
            state = state.model_copy(update={"problema":"headless_turno_erro", "problema_detalhe":"Estado do runtime indisponível; aguarde a reposição."})
        return state

    def comandos(self, name):
        data = self.view(name).data
        return data.get("commands"), frozenset(data.get("terminal_commands") or [])

    def problema_de(self, name):
        state = self.snapshot(name)
        if self.provider == "codex":
            return state.problema
        return (state.problema, state.problema_detalhe) if state.problema else None

    def aprovacao_pendente(self, name):
        state = self.snapshot(name)
        return state.question, state.options

    def permission_modes_sem_terminal(self, name):
        from app.adapters.codex.sem_terminal import modos_para_tela
        return modos_para_tela(self.view(name).data.get("permission_mode"))

    def sync_lifecycle(self, method, name, arguments):
        coordinator = runtime_coordinator.current()
        if not hasattr(coordinator, "lifecycle_call"):
            raise RuntimeError("operação exige a barreira de lifecycle")
        return run_sync(lambda: coordinator.lifecycle_call(name, method, arguments), getattr(coordinator, "loop", None))

    async def deliverable(self, name):
        return self.view(name, mutating=True).data.get("deliverable") is True

    async def drain(self, name, path):
        self.view(name, mutating=True)
        result = await runtime_coordinator.current().op(name, {"kind":"drain"}, uuid.uuid4().hex)
        return int(result["sent"])

    async def state_stream(self, name, sid_get):
        slot = native_slot(name)
        key, generation, instance = slot.binding.key, slot.binding.generation, runtime_coordinator.current().instance
        await runtime_coordinator.current()._push_channels(slot)
        last = -1
        while True:
            coordinator = runtime_coordinator.current()
            if (coordinator.instance != instance or slot.binding.key != key or slot.binding.generation != generation
                    or slot.phase != runtime_coordinator.Phase.Rust):
                return
            slot.changed.clear()
            revision = slot.view.get("revision", -1)
            if revision != last or not slot.cache_valid:
                last = revision
                yield self.snapshot(name)
            await slot.changed.wait()

    async def dispatch(self, method, name, arguments):
        if method == "ensure_running":
            return await self.ensure_running(name)
        if method in {"deliverable", "list_models"}:
            return await getattr(self, method)(name)
        if method == "drain":
            return await self.drain(name, arguments.get("path", ""))
        if method == "send_prompt":
            self.view(name, mutating=True)
            reply = await runtime_coordinator.current().op(name, {"kind":"submit", "text":arguments["text"]}, uuid.uuid4().hex)
            if reply.get("disposition") == "unknown":
                raise RuntimeError("envio incerto; entrada conservada no diário")
            if reply.get("disposition") == "rejected":
                raise ValueError("entrada recusada pelo runtime")
            return "sent" if reply.get("disposition") == "accepted" else "deferred"
        if method == "steer":
            await self.control(name, "steer", {"text":arguments["text"], "turn_id":arguments.get("turn_id")})
            return None
        if method == "steer_queue":
            result = await self.control(name, "steer_queue", {"entry_id":arguments.get("entry_id")})
            return result["ids"]
        if method == "interrupt":
            result = await self.control(name, "interrupt")
            return result.get("interrupted", True) if isinstance(result, dict) else True
        if method == "select":
            from app.rust_server import RustOpError
            try:
                await self.control(name, "select", {"option":arguments["option"]})
            except RustOpError as exc:
                # Só este código diz que não há pedido de permissão a responder; as outras recusas sobem com o motivo.
                if exc.code == "no_pending_permission":
                    return False
                raise
            return True
        if method == "answer_questions":
            await self.control(name, "answer_questions", {"request_id":arguments["request_id"], "answers":arguments["answers"]})
            return None
        if method == "set_model":
            result = await self.control(name, "set_model", {"model":arguments.get("model"), "effort":arguments.get("effort")}, allow_deferred=self.provider == "claude")
            return not (isinstance(result, dict) and result.get("_runtime_deferred")) if self.provider == "claude" else None
        if method == "set_service_tier":
            result = await self.control(name, "set_service_tier", {"service_tier":arguments["service_tier"]})
            if not isinstance(result, dict) or result.get("service_tier") != arguments["service_tier"]:
                raise RuntimeError("O Codex não confirmou a escolha Fast")
            return result["service_tier"]
        if method == "set_permission_mode" and self.provider == "claude":
            await self.control(name, "set_permission_mode", {"mode":arguments["mode"]})
            return "manual" if arguments["mode"] == "default" else arguments["mode"]
        if method == "read_rate_limits":
            # Como o adapter Python: o retrato da conta, e None quando a leitura falha (a rota responde neutro).
            try:
                result = await self.control(name, method, {})
            except (RuntimeError, ValueError):
                return None
            return result.get("rateLimits") if isinstance(result, dict) else None
        if method in {"skip_question", "read_settings", "set_mode", "compact", "list_skills"}:
            payload = {key:value for key,value in arguments.items() if key not in {"self", "name"}}
            result = await self.control(name, method, payload)
            if method == "list_skills":
                from app.adapters.codex.chat_controls import skills_do_catalogo
                return skills_do_catalogo(result)
            return None if method in {"skip_question", "compact"} else result
        if self.provider == "codex" and method in {"recarregar", "restart"}:
            # O Rust mata e sobe o processo na mesma conversa; a sessão não sai dele.
            await self.control(name, "restart")
            return None
        if self.provider == "codex" and method == "set_permission_mode_sem_terminal":
            from app.adapters.codex.sem_terminal import Ocupada
            from app.rust_server import RustOpError
            try:
                return await self.control(name, "set_permission_mode", {"mode":arguments["modo"]})
            except RustOpError as exc:
                # Os mesmos erros do adapter Python, que a rota já traduz (409, 400).
                if exc.code == "erro_permissao_ocupada":
                    raise Ocupada(exc.message) from None
                if exc.code == "erro_modo_desconhecido":
                    raise ValueError(exc.message) from None
                raise
        if method in {"parar", "recarregar", "restart", "open_terminal", "open_headless", "set_permission_mode_sem_terminal"}:
            return await runtime_coordinator.current().lifecycle_call(name, method, arguments)
        raise RuntimeError("método exige encaminhamento explícito ao responsável")


class NativeVoiceClient:
    virtual = True
    endpoint = None

    def __init__(self, coordinator, name, call_id, target_events):
        self.coordinator, self.name, self.call_id = coordinator, name, call_id
        slot = coordinator.slot(name)
        self.binding = (coordinator.instance, slot.binding.key, slot.binding.generation)
        self.target_thread = RuntimeAdapter("codex").view(name).thread_id
        self.target_events = target_events
        self.events = asyncio.Queue(maxsize=256)
        self.closed = False
        self.failed = False
        self._closing = asyncio.Lock()
        self.thread_id = None
        self._close_id = uuid.uuid4().hex

    def valid(self):
        if self.closed or self.failed:
            return False
        try:
            slot = self.coordinator.slot(self.name)
            return ((self.coordinator.instance, slot.binding.key, slot.binding.generation) == self.binding
                and slot.phase == runtime_coordinator.Phase.Rust and slot.cache_valid
                and slot.view["view"].get("thread_id") == self.target_thread)
        except (KeyError, ValueError):
            return False

    async def request(self, method, params, timeout=30.0):
        if not self.valid():
            raise RuntimeError("chamada de outra posse ou geração")
        result = await asyncio.wait_for(RuntimeAdapter("codex").control(self.name, "voice_rpc",
            {"call_id":self.call_id, "method":method, "params":params}), timeout)
        if not isinstance(result, dict):
            raise RuntimeError("resposta do organizador inválida")
        if method == "thread/start":
            self.thread_id = result["thread"]["id"]
        return result

    async def respond(self, request_id, result, *, erro=None):
        if not self.valid() or type(request_id) not in (int, str):
            raise RuntimeError("resposta de outra chamada ou geração")
        await RuntimeAdapter("codex").control(self.name, "voice_respond",
            {"call_id":self.call_id, "request_id":request_id, "result":result, "error":erro})

    async def notifications(self):
        while True:
            event = await self.events.get()
            if isinstance(event, Exception):
                raise event
            if event is None:
                return
            yield event

    def receive(self, channel, event):
        if not self.valid():
            self.fail(RuntimeError("voz invalidada pela troca de posse"))
            return
        queue = self.events if channel == "voice" else self.target_events
        if queue.full():
            self.fail(RuntimeError("eventos de voz excederam a fila; chamada encerrada"))
            return
        queue.put_nowait(copy.deepcopy(event))

    def fail(self, error):
        self.failed = True
        while self.events.full():
            self.events.get_nowait()
        self.events.put_nowait(error)

    async def close(self):
        async with self._closing:
            if self.closed:
                return
            slot = self.coordinator.slot(self.name)
            owned = (self.coordinator.instance, slot.binding.key, slot.binding.generation) == self.binding and slot.phase == runtime_coordinator.Phase.Rust
            try:
                if owned:
                    if not slot.cache_valid and not await self.coordinator.refresh_snapshot(self.name):
                        raise RuntimeError("estado não reposto; fechamento da voz não confirmado")
                    await RuntimeAdapter("codex").control(self.name, "voice_close", {"call_id":self.call_id}, operation_id=self._close_id)
            finally:
                self.closed = True
                self.coordinator.voice_clients.pop((self.binding[1], self.call_id), None)
                self.fail(RuntimeError("chamada encerrada"))


async def open_voice(name, target_events):
    coordinator = runtime_coordinator.current()
    facade = RuntimeAdapter("codex")
    view = facade.view(name, mutating=True)
    if any(key == view.key and not client.closed for (key, _), client in coordinator.voice_clients.items()):
        raise RuntimeError("chamada de voz já aberta")
    call_id = uuid.uuid4().hex
    client = NativeVoiceClient(coordinator, name, call_id, target_events)
    coordinator.voice_clients[(view.key, call_id)] = client
    try:
        await facade.control(name, "voice_open", {"call_id":call_id})
    except BaseException:
        coordinator.voice_clients.pop((view.key, call_id), None)
        client.closed = True
        raise
    return {"thread_id":view.thread_id, "model":view.data.get("model"), "client":client,
        "voice_events":target_events, "runtime_voice":True}


def voice_current(name, target, adapter):
    if target.get("runtime_voice"):
        return target["client"].valid()
    return adapter._sessions.get(name) is target


# ponytail: a posse é conferida por consulta a cada 0,25 s; a fase muda sem aviso único a quem espera.
_OWNER_POLL_S = 0.25


def _hands_over(provider, name):
    """Só sessão sem terminal troca de dono; as outras seguem o monitor direto, sem passo a mais."""
    if provider == "claude":
        return True         # o adapter embrulhado como "claude" é o do Claude sem terminal
    from app.adapters.codex import sessions
    return bool((sessions.load(name) or {}).get("headless"))


def _state_owner(name):
    """Quem responde pelo estado agora: a sessão aberta no Rust, ou a vista do Python (parada,
    Codex, Python dono da porta)."""
    coordinator = runtime_coordinator.current()
    if coordinator is None or not coordinator.managed_runtime(name):
        return ("python",)
    slot = coordinator.slot(name)
    if slot.binding.headless and slot.phase == runtime_coordinator.Phase.Rust:
        return ("rust", coordinator.instance, slot.binding.key, slot.binding.generation)
    return ("python",)


async def owner_state_stream(legacy, native, name):
    """Estado que segue a sessão: aberta no Rust ou não. A troca de fonte não derruba o chat, e o
    erro de uma fonte cuja sessão acabou de mudar não sobe."""
    while True:
        owner = _state_owner(name)
        source = native() if owner[0] == "rust" else legacy()
        # Uma tarefa só itera a fonte, e só avança quando o chat pede o próximo: como a iteração
        # direta. O monitor do Codex guarda estado da própria tarefa entre yields.
        wanted, items = asyncio.Queue(), asyncio.Queue()

        async def pump(source=source, wanted=wanted, items=items):
            try:
                while True:
                    await wanted.get()
                    try:
                        item = await anext(source)
                    except StopAsyncIteration:
                        items.put_nowait(("end", None))
                        return
                    items.put_nowait(("item", item))
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                items.put_nowait(("error", exc))

        task = asyncio.ensure_future(pump())
        requested = False
        try:
            while True:
                if not requested:
                    wanted.put_nowait(None)
                    requested = True
                try:
                    kind, value = await asyncio.wait_for(items.get(), _OWNER_POLL_S)
                except TimeoutError:
                    if _state_owner(name) != owner:
                        break
                    continue
                requested = False
                if kind == "item":
                    yield value
                    if _state_owner(name) != owner:
                        break
                    continue
                if _state_owner(name) != owner:
                    break
                if kind == "end":
                    return
                if isinstance(value, runtime_coordinator.TransferInProgress):
                    await asyncio.sleep(_OWNER_POLL_S)    # passagem curta que a consulta não viu
                    break
                raise value
        finally:
            try:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
            finally:
                await source.aclose()

_ASYNC = {"ensure_running", "send_prompt", "deliverable", "drain", "steer", "steer_queue", "interrupt", "select",
    "answer_questions", "set_model", "set_service_tier", "set_permission_mode", "list_models", "read_settings", "read_rate_limits", "set_mode",
    "compact", "list_skills", "skip_question", "parar", "recarregar", "restart", "open_terminal", "open_headless", "set_permission_mode_sem_terminal"}
_CODEX_OPENS_IN_RUST = {"restart", "set_permission_mode_sem_terminal", "open_terminal"}
_SYNC = {"snapshot", "escolhas", "comandos", "problema_de", "current_model", "aprovacao_pendente", "permission_modes_sem_terminal", "rename", "close_sync"}


async def reserve_call(coordinator, original, adapter, method, name, arguments, args, kwargs):
    context = _legacy_operation.get()
    if context is not None and context.get("operation_id") in coordinator.legacy_active:
        return await original(adapter, *args, **kwargs)
    slot = coordinator.slot(name)
    operation_id = uuid.uuid4().hex
    kind = {"send_prompt":"input", "steer":"steer", "set_permission_mode":"set_permission_mode"}.get(method, method)
    payload = {key:value for key,value in arguments.items() if key not in {"self", "name"}}
    context = {"key":slot.binding.key, "generation":slot.binding.generation, "operation_id":operation_id,
        "command":{"operation_id":operation_id, "kind":kind, "payload":payload}}
    token = _legacy_operation.set(context)
    coordinator.legacy_active.add(operation_id)
    try:
        with coordinator.queue_gate(name):
            try:
                result = await original(adapter, *args, **kwargs)
            except BaseException:
                await LegacyIO(coordinator).finish_call(name, context, failed=True)
                raise
            await LegacyIO(coordinator).finish_call(name, context, deferred=result == "deferred")
            return result
    finally:
        coordinator.legacy_active.discard(operation_id)
        _legacy_operation.reset(token)


def install_adapter(cls, provider):
    for method in _ASYNC | _SYNC | {"state_monitor"}:
        original = getattr(cls, method, None)
        if original is None or getattr(original, "runtime_wrapped", False):
            continue
        signature = inspect.signature(original)
        facade = RuntimeAdapter(provider)
        if method == "state_monitor":
            @functools.wraps(original)
            def wrapper(self, name, sid_get, _original=original, _facade=facade):
                if not _hands_over(_facade.provider, name):
                    return _original(self, name, sid_get)
                return owner_state_stream(lambda: _original(self, name, sid_get),
                                          lambda: _facade.state_stream(name, sid_get), name)
        elif method in _SYNC:
            @functools.wraps(original)
            def wrapper(self, *args, _original=original, _signature=signature, _method=method, _facade=facade, **kwargs):
                bound = _signature.bind(self, *args, **kwargs)
                bound.apply_defaults()
                name = bound.arguments.get("name", bound.arguments.get("old"))
                try:
                    slot = native_slot(name)
                except runtime_coordinator.TransferInProgress:
                    if _method in {"rename", "close_sync"}:
                        raise
                    # Leitura na passagem (lista, monitor): vale a vista do Python, sem a sessão
                    # viva nele, até o novo dono confirmar. Recusar dava 500 na lista inteira.
                    slot = None
                if slot is not None:
                    if _method in {"rename", "close_sync"}:
                        return _facade.sync_lifecycle(_method, name, bound.arguments)
                    rest = {key:value for key,value in bound.arguments.items() if key not in {"self", "name"}}
                    return getattr(_facade, _method)(name, **rest)
                return _original(self, *args, **kwargs)
        else:
            @functools.wraps(original)
            async def wrapper(self, *args, _original=original, _signature=signature, _method=method, _facade=facade, **kwargs):
                bound = _signature.bind(self, *args, **kwargs)
                bound.apply_defaults()
                name = bound.arguments["name"]
                coordinator = runtime_coordinator.current()
                context = _legacy_operation.get()
                if context is not None and coordinator is not None and context.get("operation_id") in coordinator.legacy_active:
                    return await _original(self, *args, **kwargs)
                managed = coordinator is not None and getattr(coordinator, "legacy", None) is not None
                # Reiniciar, trocar o sandbox e passar para terminal precisam do processo: abrem no Rust
                # como o envio, senão a sessão parada caía no Python, que não pode subir o cano.
                opens = _method == "ensure_running" or _facade.provider == "codex" and _method in _CODEX_OPENS_IN_RUST
                if (managed and opens and _facade.provider == "codex" and coordinator.mode != "python"
                        and await asyncio.to_thread(_hands_over, "codex", name)):
                    await coordinator.await_mode()      # Rust esperado: o dono da sessão sai do desfecho dele
                if (opens and managed and getattr(coordinator, "transport", None) is not None
                        and (_facade.provider == "claude" or coordinator.rust_owns("codex", True)
                             and await asyncio.to_thread(_hands_over, "codex", name))
                        and not bound.arguments.get("so_reconectar") and bound.arguments.get("transfer_id") is None):
                    # Subir a sessão é abri-la no Rust, com a conta/motor pedidos e a espera do initialize;
                    # dentro da barreira (troca de conta) é reabrir já o que a administração fechou.
                    options = {"engine_models":bound.arguments.get("engine_models"),
                        "wait_initialized":bool(bound.arguments.get("esperar_pronta") or bound.arguments.get("require_initialize"))}
                    if coordinator.managed_queue(name) and coordinator.in_lifecycle(coordinator.slot(name)):
                        await coordinator.reopen_in_change(name, **options)
                    else:
                        await coordinator.ensure_open(name, **options)
                elif (coordinator is not None and getattr(coordinator, "legacy", None) is not None
                        and not (coordinator.managed_queue(name) and coordinator.in_lifecycle(coordinator.slot(name)))):
                    await coordinator.prepare_session(name, _facade.provider)
                if native_slot(name) is not None:
                    return await _facade.dispatch(_method, name, bound.arguments)
                coordinator = runtime_coordinator.current()
                if coordinator is not None and coordinator.managed_runtime(name):
                    return await reserve_call(coordinator, _original, self, _method, name, bound.arguments, args, kwargs)
                return await _original(self, *args, **kwargs)
        wrapper.runtime_wrapped = True
        setattr(cls, method, wrapper)
    original = getattr(cls, "acordar", None)
    if original is not None and not getattr(original, "runtime_wrapped", False):
        @functools.wraps(original)
        def wake(self, name, *, engine_models=None, _original=original):
            if native_slot(name) is not None:
                runtime_coordinator.current().request_drain(name)
                return
            coordinator = runtime_coordinator.current()
            if provider == "claude" and coordinator is not None and coordinator.legacy is not None and coordinator.transport is not None:
                from app.conversation_transfer import transfer_active
                if transfer_active(name):
                    return
                self.reset_start_attempts(name)   # ação do usuário: nova rodada de tentativas
                if coordinator.managed_queue(name) and (slot := coordinator.slot(name)).change is not None and coordinator.in_lifecycle(slot):
                    slot.change["relaunch"] = True      # a administração reabre lançando o processo
                    return
                # Nasce direto no Rust: o processo sobe sem cliente Python e o ator drena a fila
                # quando a sessão fica entregável.
                async def open_in_rust():
                    try:
                        slot = await coordinator.ensure_open(name, engine_models=engine_models)
                        if slot.phase != runtime_coordinator.Phase.Rust:
                            await self.ensure_running(name)
                            await coordinator.op(name, {"kind":"drain"}, uuid.uuid4().hex)
                    except Exception as exc:
                        from app import diag
                        from app.runtime_coordinator import failure_reason
                        diag.registrar("runtime.wake_failed", "erro", sessao=name, **failure_reason(exc))
                task = coordinator.loop.create_task(open_in_rust())
                self._tarefas.add(task)
                task.add_done_callback(self._tarefas.discard)
                return
            if coordinator is not None and coordinator.legacy is not None:
                async def start():
                    try:
                        await self.ensure_running(name)
                        await coordinator.prepare_session(name, provider)
                        await coordinator.op(name, {"kind":"drain"}, uuid.uuid4().hex)
                    except Exception as exc:
                        from app import diag
                        from app.runtime_coordinator import failure_reason
                        diag.registrar("runtime.wake_failed", "erro", sessao=name, **failure_reason(exc))
                task = coordinator.loop.create_task(start())
                self._tarefas.add(task)
                task.add_done_callback(self._tarefas.discard)
                return
            _original(self, name, **({"engine_models":engine_models} if engine_models is not None else {}))
        wake.runtime_wrapped = True
        cls.acordar = wake
