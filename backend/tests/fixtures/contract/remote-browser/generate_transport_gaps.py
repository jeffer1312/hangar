"""Prepara referências adicionais executando somente o navegador Python original isolado."""
from __future__ import annotations

import argparse
import asyncio
import base64
import copy
import hashlib
import io
import json
import logging
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import traceback
import weakref
from urllib.parse import parse_qs, urlsplit

from fastapi import FastAPI, WebSocket

SOURCE = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]
OWNER = "remote-browser-contract-owner"
TOKEN = "remote-browser-contract-desktop"
NAME = "nav-contract"
KEY = "contract::nav-contract"
BARRIER = "__transport_gap_barrier__"
LIMIT = 32 * 1024 * 1024


class Boundary:
    def __init__(self, root):
        self.root = root.resolve()
        self.sockets = weakref.WeakSet()
        self.listeners = weakref.WeakKeyDictionary()
        self.processes, self.connections, self.unexpected = [], [], []
        self.threads = set(threading.enumerate())

    def install(self):
        bind = socket.socket.bind

        def own_bind(sock, address):
            if sock.family != socket.AF_INET or address != ("127.0.0.1", 0):
                self.unexpected.append({"operation": "bind", "address": repr(address)})
                raise PermissionError("o ensaio só abre listener efêmero próprio de loopback")
            result = bind(sock, address)
            self.listeners[sock] = sock.getsockname()
            return result

        socket.socket.bind = own_bind
        expected_git = {
            ("git", "describe", "--tags", "--match", "dist-latest", "--long", "--always", "--dirty"),
            ("git", "rev-list", "--count", "HEAD"),
            ("git", "show", "HEAD:VERSION"),
        }

        def audit(event, args):
            if event == "socket.__new__":
                self.sockets.add(args[0])
            elif event == "subprocess.Popen":
                _, argv, cwd, _ = args
                expected = tuple(argv) in expected_git and cwd is not None and Path(cwd).resolve().is_relative_to(self.root)
                row = {"operation": event, "executable": Path(str(argv[0])).name, "arguments": list(argv) if expected else None, "expected_version_probe": expected}
                self.processes.append(row)
                if not expected:
                    self.unexpected.append(row)
                raise PermissionError("nenhum processo é permitido no filho de caracterização")
            elif event in ("os.system", "os.posix_spawn", "os.fork", "os.forkpty", "pty.spawn"):
                self.unexpected.append({"operation": event})
                raise PermissionError("processo fora da fronteira de caracterização")
            elif event == "socket.getaddrinfo":
                if args[0] not in (None, "127.0.0.1"):
                    self.unexpected.append({"operation": event, "host": str(args[0])})
                    raise PermissionError("DNS fora do loopback do ensaio")
            elif event == "socket.connect":
                sock, address = args
                allowed = sock.family == socket.AF_INET and isinstance(address, tuple) and address[0] == "127.0.0.1" and any(
                    listener.fileno() >= 0 and published == address for listener, published in list(self.listeners.items()))
                self.connections.append({"address": repr(address), "allowed": allowed})
                if not allowed:
                    self.unexpected.append({"operation": event, "address": repr(address)})
                    raise PermissionError("conexão sem listener próprio registrado")

        sys.addaudithook(audit)

    def evidence(self):
        active = [{"fd": sock.fileno(), "family": int(sock.family)} for sock in list(self.sockets)
                  if sock.fileno() >= 0 and sock.family in (socket.AF_INET, socket.AF_INET6)]
        threads = [thread.name for thread in threading.enumerate() if thread not in self.threads and thread.is_alive()]
        return {"blocked_processes": self.processes, "connections": self.connections,
                "unexpected": self.unexpected, "remaining_sockets": active, "remaining_threads": threads}

    def finish(self):
        evidence = self.evidence()
        errors = []
        if evidence["unexpected"] or evidence["remaining_sockets"] or evidence["remaining_threads"]:
            errors.append(AssertionError("a fronteira de isolamento não terminou íntegra"))
        for sock in list(self.sockets):
            if sock.fileno() >= 0 and sock.family in (socket.AF_INET, socket.AF_INET6):
                try:
                    sock.close()
                except BaseException as error:
                    errors.append(error)
        return evidence, errors


class Cleanup:
    def __init__(self):
        self.actions = []

    def add(self, label, action, phase):
        self.actions.append((phase, len(self.actions), label, action))

    async def finish(self, original=None):
        actions, self.actions = sorted(self.actions), []
        errors = []
        for _, _, label, action in actions:
            try:
                result = action()
                if hasattr(result, "__await__"):
                    await result
            except BaseException as error:
                failure = RuntimeError(f"limpeza incompleta: {label}")
                failure.__cause__ = error
                errors.append(failure)
        if errors:
            raise BaseExceptionGroup("falha e limpeza da caracterização", ([original] if original else []) + errors)


async def finish_tasks(tasks, seconds):
    tasks = set(tasks)
    if not tasks:
        return
    _, pending = await asyncio.wait(tasks, timeout=seconds)
    for task in pending:
        task.cancel()
    results = await asyncio.gather(*tasks, return_exceptions=True)
    errors = [result for result in results if isinstance(result, BaseException) and not isinstance(result, asyncio.CancelledError)]
    if pending:
        errors.insert(0, TimeoutError(f"{len(pending)} tarefas precisaram de cancelamento após {seconds} s"))
    if errors:
        raise BaseExceptionGroup("tarefas do ensaio não terminaram normalmente", errors)


async def wait_events(events):
    results = await asyncio.gather(*(asyncio.wait_for(event.wait(), 5) for event in events), return_exceptions=True)
    errors = [result for result in results if isinstance(result, BaseException)]
    if errors:
        for task in asyncio.all_tasks():
            if not task.done():
                print(f"tarefa pendente: {task.get_name()} {task.get_coro()}", file=sys.stderr)
                current = task.get_coro()
                while current is not None:
                    frame = getattr(current, "cr_frame", None) or getattr(current, "gi_frame", None)
                    if frame is not None:
                        print(f"  aguardando {frame.f_code.co_filename}:{frame.f_lineno} {frame.f_code.co_name}", file=sys.stderr)
                    current = getattr(current, "cr_await", None) or getattr(current, "gi_yieldfrom", None)
        raise BaseExceptionGroup("receptores CDP não terminaram", errors)


def encoded(value):
    if value is None:
        return {"kind": "null"}
    if isinstance(value, bool):
        return {"kind": "bool", "value": value}
    if isinstance(value, int):
        return {"kind": "integer", "decimal": str(value)}
    if isinstance(value, float):
        return {"kind": "float", "bits": struct.pack(">d", value).hex()}
    if isinstance(value, str):
        return {"kind": "string", "points": [ord(character) for character in value]}
    if isinstance(value, list):
        return {"kind": "array", "items": [encoded(item) for item in value]}
    return {"kind": "object", "entries": [[encoded(key), encoded(value[key])] for key in sorted(value)]}


class Logs(logging.Handler):
    def __init__(self):
        super().__init__()
        self.rows = []

    def emit(self, record):
        self.rows.append({"level": record.levelname, "message": record.getMessage()})


class Desktop:
    def __init__(self, measured, inputs):
        self.measured = measured
        self.inputs = inputs
        self.layout = inputs.get("original", "mobile")
        self.commands, self.requests, self.keys = [], [], []
        self.peers, self.closed, self.cdp_closes = [], [], []
        self.handlers = set()
        self.changed = asyncio.Condition()
        self.release = asyncio.Event()
        self.gated = asyncio.Event()
        self.capture = asyncio.Event()
        self.close_started = asyncio.Event()
        self.capture_times = []

    async def notify(self):
        async with self.changed:
            self.changed.notify_all()

    async def wait(self, predicate, seconds=5):
        async with asyncio.timeout(seconds):
            async with self.changed:
                await self.changed.wait_for(predicate)

    async def cdp(self, ws):
        assert ws.headers.get("authorization") == f"Bearer {TOKEN}"
        await ws.accept()
        peer = len(self.peers)
        self.peers.append(ws)
        self.closed.append(asyncio.Event())
        self.cdp_closes.append(None)
        self.keys.append(ws.query_params.get("chave"))
        pending_writes = set()
        try:
            while True:
                message = json.loads(await ws.receive_text())
                self.commands.append({"peer": peer, "message": message})
                method = message["method"]
                await self.notify()
                if method == "Page.captureScreenshot":
                    self.capture_times.append(time.monotonic())
                    self.capture.set()
                    if self.inputs.get("hold_capture"):
                        continue
                    result = copy.deepcopy(self.measured["screenshot"])
                else:
                    result = {}
                if method == "Page.enable" and "response_bytes" in self.inputs:
                    response = json.dumps({"id": message["id"], "result": {}, "padding": ""}, separators=(",", ":"))
                    response = response[:-2] + "x" * (self.inputs["response_bytes"] - len(response)) + '"}'
                    assert len(response.encode()) == self.inputs["response_bytes"]
                    if self.inputs["response_bytes"] > LIMIT:
                        pending_writes.add(asyncio.create_task(ws.send_text(response)))
                    else:
                        await ws.send_text(response)
                else:
                    await ws.send_text(json.dumps({"id": message["id"], "result": result}))
                if method == "Page.startScreencast":
                    await self.emit(peer, {"method": "Page.frameNavigated", "params": {"frame": {"url": "http://contract.test/ready"}}})
                if method == "Input.insertText" and message["params"].get("text") == BARRIER:
                    await self.emit(peer, {"method": "Page.frameNavigated", "params": {"frame": {"url": "http://contract.test/barrier"}}})
        except Exception as error:
            from starlette.websockets import WebSocketDisconnect
            if not isinstance(error, WebSocketDisconnect):
                raise
            self.cdp_closes[peer] = {"code": error.code, "reason": error.reason or ""}
        finally:
            for task in pending_writes:
                if not task.done():
                    task.cancel()
            if pending_writes:
                results = await asyncio.gather(*pending_writes, return_exceptions=True)
                errors = [error for error in results if isinstance(error, BaseException)
                          and not isinstance(error, (asyncio.CancelledError, ConnectionError))]
                if errors:
                    self.closed[peer].set()
                    raise BaseExceptionGroup("envio CDP de ensaio falhou", errors)
            self.closed[peer].set()
            await self.notify()

    async def emit(self, peer, message):
        await self.peers[peer].send_text(json.dumps(message))

    async def frame(self, peer, session, width, height):
        frame = copy.deepcopy(self.measured["frame"])
        frame["sessionId"] = session
        frame["metadata"].update(deviceWidth=width, deviceHeight=height)
        await self.emit(peer, {"method": "Page.screencastFrame", "params": frame})

    async def command(self, reader, writer):
        try:
            header = await reader.readuntil(b"\r\n\r\n")
            fields = {key.lower(): value.strip() for key, value in
                      (line.split(b":", 1) for line in header.split(b"\r\n")[1:] if b":" in line)}
            body = json.loads(await reader.readexactly(int(fields[b"content-length"])))
            self.requests.append({"authorization": fields[b"authorization"].decode(), "body": body, "receiver_pid": 123})
            number = len(self.requests)
            await self.notify()
            if number == self.inputs.get("gate_at"):
                if self.inputs.get("apply_before_gate"):
                    self.layout = "x".join(body["args"])
                self.gated.set()
                await self.release.wait()
            if number == self.inputs.get("disconnect_at"):
                return
            if number == self.inputs.get("timeout_at"):
                await reader.read()
                return
            arguments = body["args"]
            if arguments:
                self.layout = "x".join(arguments) if len(arguments) == 2 else arguments[0]
            data = f"layout: {self.layout}".encode()
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: " + str(len(data)).encode() + b"\r\n\r\n" + data)
            try:
                await writer.drain()
            except ConnectionError:
                if not self.inputs.get("detach_pending"):
                    raise
        finally:
            writer.close()
            await writer.wait_closed()

    def accept(self, reader, writer):
        self.handlers.add(asyncio.create_task(self.command(reader, writer)))

    async def raw_cdp(self, reader, writer):
        peer = len(self.keys)
        self.closed.append(asyncio.Event())
        self.cdp_closes.append(None)
        try:
            header = await reader.readuntil(b"\r\n\r\n")
            rows = header.decode().split("\r\n")
            fields = {key.lower(): value.strip() for key, value in
                      (row.split(":", 1) for row in rows[1:] if ":" in row)}
            assert fields["authorization"] == f"Bearer {TOKEN}"
            key = parse_qs(urlsplit(rows[0].split()[1]).query)["chave"][0]
            assert key == KEY
            self.keys.append(key)
            accepted = base64.b64encode(hashlib.sha1(fields["sec-websocket-key"].encode() + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest())
            writer.write(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " + accepted + b"\r\n\r\n")
            await writer.drain()

            async def write(opcode, payload):
                length = len(payload)
                size = bytes([length]) if length < 126 else bytes([126]) + length.to_bytes(2, "big")
                writer.write(bytes([0x80 | opcode]) + size + payload)
                await writer.drain()

            async def text(value):
                await write(1, json.dumps(value).encode())

            while True:
                head = await reader.readexactly(2)
                length = head[1] & 127
                if length == 126:
                    length = int.from_bytes(await reader.readexactly(2), "big")
                elif length == 127:
                    length = int.from_bytes(await reader.readexactly(8), "big")
                mask = await reader.readexactly(4) if head[1] & 128 else b"\0" * 4
                raw = await reader.readexactly(length)
                payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(raw))
                opcode = head[0] & 15
                if opcode == 8:
                    self.cdp_closes[peer] = {"code": int.from_bytes(payload[:2], "big") if len(payload) >= 2 else 1005,
                                             "reason": payload[2:].decode() if len(payload) >= 2 else ""}
                    self.close_started.set()
                    await self.release.wait()
                    await write(8, payload)
                    break
                if opcode == 9:
                    await write(10, payload)
                    continue
                assert opcode == 1
                message = json.loads(payload)
                self.commands.append({"peer": peer, "message": message})
                await self.notify()
                await text({"id": message["id"], "result": {}})
                if message["method"] == "Page.startScreencast":
                    await text({"method": "Page.frameNavigated", "params": {"frame": {"url": "http://contract.test/ready"}}})
        finally:
            writer.close()
            await writer.wait_closed()
            self.closed[peer].set()
            await self.notify()

    def accept_raw_cdp(self, reader, writer):
        self.handlers.add(asyncio.create_task(self.raw_cdp(reader, writer)))

    def result(self, events, logs, clients):
        return {"events_by_viewer": events, "cdp": self.commands, "cmd": self.requests,
                "desktop_layout": self.layout, "cdp_keys": self.keys, "log": logs.rows,
                "cdp_close": self.cdp_closes,
                "viewer_close": [{"code": client.close_code, "reason": client.close_reason or ""} for client in clients]}


def special_inputs():
    cases = []
    for label, field, template in [
        ("x", "x", {"t": "m", "acao": "press", "y": 0.5}),
        ("y", "y", {"t": "m", "acao": "press", "x": 0.5}),
        ("mod", "mod", {"t": "m", "acao": "press"}),
        ("key_mod", "mod", {"t": "k", "key": "custom"}),
        ("dx", "dx", {"t": "w"}),
        ("dy", "dy", {"t": "w"}),
        ("code", "code", {"t": "k", "key": "custom"}),
        ("w", "w", {"t": "layout", "h": 700}),
        ("h", "h", {"t": "layout", "w": 900}),
        ("s", "s", {"t": "txt"}),
        ("key", "key", {"t": "k"}),
        ("t", "t", {}),
        ("action", "acao", {"t": "m"}),
        ("button", "b", {"t": "m", "acao": "press"}),
        ("mode", "modo", {"t": "layout"}),
    ]:
        for special, value in [("nan", float("nan")), ("infinity", float("inf")),
                               ("negative_infinity", -float("inf")), ("huge", 10 ** 100),
                               ("surrogate", "\ud800")]:
            message = {**template, field: value}
            cases.append((f"special_{label}_{special}", {"raw": json.dumps(message), "encoded": True}))
    for field in ("deviceWidth", "deviceHeight"):
        for label, value in [("nan", float("nan")), ("infinity", float("inf")),
                             ("negative_infinity", -float("inf")), ("huge", 10 ** 100),
                             ("surrogate", "\ud800")]:
            metadata = {"deviceWidth": 1000, "deviceHeight": 500, field: value}
            prefix = "metadata" if field == "deviceWidth" else "metadata_height"
            cases.append((f"{prefix}_{label}", {"raw_metadata": json.dumps(metadata), "encoded": True}))
    cases.extend([
        ("only_height_selects_dimensions_before_mode", {"raw": json.dumps({"t": "layout", "h": 700, "modo": "desktop"})}),
        ("fractional_pixels_are_truncated", {"raw": json.dumps({"t": "m", "acao": "press", "x": 0.1234, "y": 0.3459})}),
        ("numeric_key_is_stringified_before_dispatch", {"raw": json.dumps({"t": "k", "key": 12345})}),
    ])
    return cases


async def public_cases(root, measured):
    resources = Cleanup()
    try:
        return await public_cases_owned(root, measured, resources)
    finally:
        await resources.finish(sys.exception())


async def public_cases_owned(root, measured, resources):
    import uvicorn
    import websockets
    from websockets.exceptions import ConnectionClosed
    from app import navsock
    current = None
    handlers = []
    logs = Logs()
    navsock._log.addHandler(logs)
    resources.add("coletor de log", lambda: navsock._log.removeHandler(logs), 80)
    navsock._log.setLevel(logging.DEBUG)
    app = FastAPI()

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def remote(ws: WebSocket, name: str):
        task = asyncio.current_task()
        task.set_name("original-viewer-handler")
        handlers.append(task)
        await navsock.nav_ws(ws, name, lambda candidate: candidate == NAME)

    @app.websocket("/cdp")
    async def cdp(ws: WebSocket):
        asyncio.current_task().set_name("owned-cdp-handler")
        await current.cdp(ws)

    started = asyncio.Event()

    class Probe(uvicorn.Server):
        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            started.set()

    listener = socket.socket()
    resources.add("socket HTTP público", listener.close, 40)
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    probe = Probe(uvicorn.Config(app, log_level="critical", lifespan="off", ws="websockets-sansio",
                                 ws_per_message_deflate=False, timeout_graceful_shutdown=3))
    serving = asyncio.create_task(probe.serve(sockets=[listener]))
    resources.add("sinal de parada HTTP", lambda: setattr(probe, "should_exit", True), 0)
    resources.add("tarefa HTTP pública", lambda: finish_tasks([serving], 5), 20)
    await asyncio.wait_for(started.wait(), 5)
    nav = Path.home() / ".hangar/nav"
    nav.mkdir(parents=True)
    result = []
    plans = [
        ("initial_dimensions_before_frame", {}),
        ("ack_rate_and_false_metadata", {}),
        ("screencast_resets_fallback_wait_q30", {"query": "q=30"}),
        ("screencast_resets_fallback_wait_q45", {"query": "q=45"}),
        ("numeric_original_restore", {"original": "1024x768"}),
        ("layout_original_disconnect", {"disconnect_at": 1}),
        ("layout_apply_disconnect", {"disconnect_at": 2}),
        ("layout_mode_disconnect", {"disconnect_at": 1, "mode": True}),
        ("layout_restore_disconnect", {"disconnect_at": 4}),
        ("layout_restore_timeout", {"timeout_at": 4}),
        ("failed_error_delivery_still_restores_registered_layout", {"hold_close": True}),
        ("restore_query_failure_allows_a_fresh_original_on_reacquisition", {"disconnect_at": 3}),
        ("registered_restore_serializes_with_next_viewer_apply", {"gate_at": 4, "original": "1024x768"}),
        ("detached_while_apply_pending_preserves_unregistered_application", {"gate_at": 2, "apply_before_gate": True, "detach_pending": True}),
        ("serialized_viewers_apply_and_restore", {"gate_at": 2, "original": "1024x768"}),
        ("pending_capture_and_rust_layout_keep_other_viewer_input_independent", {"gate_at": 1, "hold_capture": True}),
        ("cdp_response_exactly_32mib", {"response_bytes": LIMIT}),
        ("cdp_response_over_32mib", {"response_bytes": LIMIT + 1}),
    ] + special_inputs() + [("registered_restore_continues_after_viewer_exit", {"gate_at": 4})]
    selected = json.loads(os.environ.get("HANGAR_GAP_CASES", "[]"))
    if selected:
        plans = [(name, inputs) for name, inputs in plans if name in selected]
        assert {name for name, _ in plans} == set(selected), "caso de captura não encontrado"
    try:
        for name, inputs in plans:
            print(f"capturando {name}", file=sys.stderr, flush=True)
            navsock._imposto.clear()
            navsock._travas.clear()
            logs.rows.clear()
            current = Desktop(measured, inputs)
            desktop = current
            case_cleanup = Cleanup()
            resources.add(f"recursos do caso {name}", case_cleanup.finish, 10)
            case_cleanup.add("liberação do controlador", desktop.release.set, 0)
            clients, events, observations = [], [], {}
            first_handler = len(handlers)
            case_cleanup.add("handlers originais", lambda: finish_tasks(handlers[first_handler:], 55), 20)
            case_cleanup.add("receptores CDP", lambda: wait_events(desktop.closed), 30)
            case_cleanup.add("tarefas do desktop de ensaio", lambda: finish_tasks(desktop.handlers, 5), 60)
            controller = await asyncio.start_server(desktop.accept, "127.0.0.1", 0)
            case_cleanup.add("listener do controlador", controller.close, 40)
            case_cleanup.add("fechamento do controlador", lambda: asyncio.wait_for(controller.wait_closed(), 5), 50)
            command_port = controller.sockets[0].getsockname()[1]
            raw_cdp = await asyncio.start_server(desktop.accept_raw_cdp, "127.0.0.1", 0) if inputs.get("hold_close") else None
            if raw_cdp:
                case_cleanup.add("listener CDP bruto", raw_cdp.close, 40)
                case_cleanup.add("fechamento CDP bruto", lambda: asyncio.wait_for(raw_cdp.wait_closed(), 5), 50)
            cdp_port = raw_cdp.sockets[0].getsockname()[1] if raw_cdp else port
            (nav / "browser.json").write_text(json.dumps({"chave": KEY, "pid": 123, "targetId": None, "url": "http://contract.test/original"}))
            # A conexão CDP é resolvida antes de o controlador de comandos ser publicado.
            (nav / "_srv.json").write_text(json.dumps({"porta": cdp_port, "pid": 123, "token": TOKEN}))

            async def drain(client, received):
                try:
                    async for raw in client:
                        received.append(json.loads(raw))
                except ConnectionClosed:
                    pass

            async def open_viewer():
                (nav / "_srv.json").write_text(json.dumps({"porta": cdp_port, "pid": 123, "token": TOKEN}))
                client = await websockets.connect(f"ws://127.0.0.1:{port}/api/sessions/{NAME}/nav-remoto?token={OWNER}&{inputs.get('query', '')}", max_size=LIMIT)
                clients.append(client)
                events.append([])
                received = events[-1]
                case_cleanup.add("fechamento do espectador", client.close, 10)
                case_cleanup.add("eventos finais do espectador", lambda client=client, received=received: asyncio.wait_for(drain(client, received), 7), 11)
                await receive(client, events[-1], lambda value: value.get("url") == "http://contract.test/ready" or value.get("t") == "erro")
                (nav / "_srv.json").write_text(json.dumps({"porta": command_port, "pid": 123, "token": TOKEN}))

            async def send(message, peer=0):
                await clients[peer].send(json.dumps(message))

            async def barrier(peer=0):
                await send({"t": "txt", "s": BARRIER}, peer)
                await receive(clients[peer], events[peer], lambda value: value.get("url") == "http://contract.test/barrier" or value.get("t") == "erro")

            try:
                await open_viewer()
                if name == "initial_dimensions_before_frame":
                    await send({"t": "m", "acao": "press", "x": 0.5, "y": 0.5})
                    await barrier()
                elif name == "ack_rate_and_false_metadata":
                    began = time.monotonic()
                    await desktop.frame(0, 501, 1000, 500)
                    await receive(clients[0], events[0], lambda value: value.get("t") == "q")
                    for session in range(502, 510):
                        await desktop.frame(0, session, 0, False)
                    await desktop.wait(lambda: sum(row["message"]["method"] == "Page.screencastFrameAck" for row in desktop.commands) == 9)
                    observations["burst_seconds"] = time.monotonic() - began
                    if observations["burst_seconds"] >= 1 / 24:
                        raise AssertionError("rajada não ficou dentro da janela real de observação de 1/24 s")
                    await asyncio.sleep(max(0, 0.080 - (time.monotonic() - began)))
                    await desktop.frame(0, 510, 0, False)
                    await desktop.wait(lambda: sum(row["message"]["method"] == "Page.screencastFrameAck" for row in desktop.commands) == 10)
                    observations["before_rate_boundary_seconds"] = time.monotonic() - began
                    if not 0.079 <= observations["before_rate_boundary_seconds"] < 1 / 12:
                        raise AssertionError("a sonda anterior ao limite de taxa saiu da janela real de observação")
                    await asyncio.sleep(max(0, 0.110 - (time.monotonic() - began)))
                    await desktop.frame(0, 511, 0, False)
                    await receive(clients[0], events[0], lambda value: value.get("t") == "q")
                    observations["after_rate_boundary_seconds"] = time.monotonic() - began
                    await send({"t": "m", "acao": "press", "x": 0.5, "y": 0.5})
                    await barrier()
                elif name.startswith("screencast_resets_"):
                    await receive(clients[0], events[0], lambda value: value.get("lento") is True)
                    await desktop.frame(0, 601, 1000, 500)
                    await receive(clients[0], events[0], lambda value: value.get("t") == "q" and "lento" not in value)
                    began = time.monotonic()
                    await receive(clients[0], events[0], lambda value: value.get("lento") is True)
                    observations["after_screencast_seconds"] = time.monotonic() - began
                elif name == "serialized_viewers_apply_and_restore":
                    await open_viewer()
                    await send({"t": "layout", "w": 900, "h": 700})
                    await asyncio.wait_for(desktop.gated.wait(), 5)
                    await send({"t": "layout", "w": 1200, "h": 900}, 1)
                    await asyncio.sleep(0.25)
                    observations["requests_while_first_apply_held"] = copy.deepcopy(desktop.requests)
                    desktop.release.set()
                    for peer in (0, 1):
                        await receive(clients[peer], events[peer], lambda value: value.get("t") == "layout")
                elif name == "pending_capture_and_rust_layout_keep_other_viewer_input_independent":
                    await asyncio.wait_for(desktop.capture.wait(), 5)
                    await open_viewer()
                    await desktop.wait(lambda: sum(row["message"]["method"] == "Page.captureScreenshot" for row in desktop.commands) == 2)
                    await send({"t": "layout", "modo": "desktop"})
                    await asyncio.wait_for(desktop.gated.wait(), 5)
                    await send({"t": "txt", "s": "same-reader-after-layout"})
                    await send({"t": "txt", "s": "outro espectador 😀"}, 1)
                    await send({"t": "txt", "s": BARRIER}, 1)
                    await desktop.wait(lambda: any(row["peer"] == 1 and row["message"]["method"] == "Input.insertText" and row["message"]["params"].get("text") == BARRIER for row in desktop.commands))
                    observations["cdp_while_capture_and_layout_held"] = copy.deepcopy(desktop.commands)
                    observations["requests_while_layout_held"] = copy.deepcopy(desktop.requests)
                    desktop.release.set()
                    await receive(clients[0], events[0], lambda value: value.get("t") == "layout")
                    await send({"t": "txt", "s": BARRIER})
                    await desktop.wait(lambda: any(row["peer"] == 0 and row["message"]["method"] == "Input.insertText" and row["message"]["params"].get("text") == BARRIER for row in desktop.commands))
                elif inputs.get("detach_pending"):
                    await send({"t": "layout", "w": 900, "h": 700})
                    await asyncio.wait_for(desktop.gated.wait(), 5)
                    await desktop.emit(0, {"method": "Inspector.detached", "params": {"reason": "target_closed"}})
                    await receive(clients[0], events[0], lambda value: value.get("t") == "erro")
                    await asyncio.wait_for(asyncio.shield(handlers[first_handler]), 5)
                    observations["layout_after_handler_ended_before_apply_release"] = desktop.layout
                    observations["requests_after_handler_ended_before_apply_release"] = copy.deepcopy(desktop.requests)
                    observations["unregistered_application"] = NAME not in navsock._imposto
                elif "raw" in inputs:
                    await clients[0].send(inputs["raw"])
                    await barrier()
                elif "raw_metadata" in inputs:
                    frame = copy.deepcopy(measured["frame"])
                    frame["metadata"] = json.loads(inputs["raw_metadata"])
                    await desktop.emit(0, {"method": "Page.screencastFrame", "params": frame})
                    await receive(clients[0], events[0], lambda value: value.get("t") in ("q", "erro"))
                elif "response_bytes" not in inputs:
                    await send({"t": "layout", "modo": "desktop"} if inputs.get("mode") else {"t": "layout", "w": 900, "h": 700})
                    await receive(clients[0], events[0], lambda value: value.get("t") in ("layout", "erro"))
                    if name == "registered_restore_serializes_with_next_viewer_apply":
                        await open_viewer()
                        await clients[0].close()
                        await asyncio.wait_for(desktop.gated.wait(), 5)
                        await send({"t": "layout", "w": 1200, "h": 900}, 1)
                        await asyncio.sleep(0.25)
                        observations["requests_while_registered_restore_held"] = copy.deepcopy(desktop.requests)
                        desktop.release.set()
                        await receive(clients[1], events[1], lambda value: value.get("t") == "layout")
                    if name == "restore_query_failure_allows_a_fresh_original_on_reacquisition":
                        await clients[0].close()
                        await asyncio.wait_for(asyncio.shield(handlers[first_handler]), 5)
                        observations["requests_after_restore_failure"] = copy.deepcopy(desktop.requests)
                        await open_viewer()
                        await send({"t": "layout", "w": 1200, "h": 900}, 1)
                        await receive(clients[1], events[1], lambda value: value.get("t") == "layout")
                    if name == "failed_error_delivery_still_restores_registered_layout":
                        await send({"t": "k", "key": "Enter", "mod": "invalid"})
                        await asyncio.wait_for(desktop.close_started.wait(), 5)
                        clients[0].transport.get_extra_info("socket").setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                        clients[0].transport.abort()
                        await clients[0].wait_closed()
                        observations["viewer_reset_before_cdp_close_release"] = clients[0].close_code == 1006
                        desktop.release.set()
            finally:
                original = sys.exception()
                try:
                    if name == "registered_restore_continues_after_viewer_exit" and original is None:
                        for index, client in enumerate(clients):
                            await client.close()
                            await asyncio.wait_for(drain(client, events[index]), 7)
                        await asyncio.wait_for(desktop.gated.wait(), 5)
                        observations["layout_while_registered_restore_held"] = desktop.layout
                        probe.should_exit = True
                        async with asyncio.timeout(2):
                            while any(server.is_serving() for server in probe.servers):
                                await asyncio.sleep(0.01)
                        await asyncio.sleep(0.25)
                        observations["lifecycle_finished_before_restore_release"] = serving.done()
                        desktop.release.set()
                        await finish_tasks([serving], 5)
                        observations["lifecycle_finished_after_restore_release"] = serving.done()
                finally:
                    await case_cleanup.finish(sys.exception() or original)
            observed = copy.deepcopy(desktop.result(events, logs, clients))
            result.append({"name": name, "inputs": inputs, "expected_encoded" if inputs.get("encoded") else "expected": encoded(observed) if inputs.get("encoded") else observed,
                           "observations": encoded(observations) if inputs.get("encoded") else observations})
            print(f"concluído {name}", file=sys.stderr, flush=True)
    finally:
        await resources.finish(sys.exception())
    return result


async def receive(client, events, predicate):
    async with asyncio.timeout(7):
        while True:
            value = json.loads(await client.recv())
            events.append(value)
            if predicate(value):
                return value


async def cdp_cases():
    import websockets
    from app import navsock
    result = {}

    async def replies(ws):
        first, second = json.loads(await ws.recv()), json.loads(await ws.recv())
        await ws.send(json.dumps({"id": second["id"], "error": {"code": -32001, "message": "second refused", "data": {"full": [1, "ação 😀"]}}, "extra": "kept"}))
        await ws.send(json.dumps({"id": first["id"], "result": {"first": True}, "extra": {"nested": [False, None]}}))
        await ws.send(json.dumps({"id": first["id"], "method": "Must.not.be.event", "result": {"duplicate": True}}))
        await ws.send(json.dumps({"id": 99999, "method": "Must.not.be.event", "result": {"unknown": True}}))
        await ws.send(json.dumps({"method": "Gap.done", "params": {"complete": True}}))
        await ws.wait_closed()

    async with websockets.serve(replies, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with navsock._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            values = await asyncio.gather(cdp.cmd("Gap.first", []), cdp.cmd("Gap.second", False))
            event = await asyncio.wait_for(cdp.eventos.get(), 5)
            result["out_of_order"] = {"replies": values, "event": event, "events_remaining": cdp.eventos.qsize(), "pending": len(cdp._pendentes)}

    async def pending(ws):
        async for raw in ws:
            message = json.loads(raw)
            if message["method"] == "Gap.finish":
                await ws.close()

    async with websockets.serve(pending, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with navsock._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            try:
                await cdp.cmd("Gap.timeout")
            except Exception as error:
                result["timeout"] = {"message": str(error), "repr": repr(error), "pending": len(cdp._pendentes)}
            waits = [asyncio.create_task(cdp.cmd("Gap.pending")) for _ in range(2)]
            await asyncio.sleep(0)
            began = time.monotonic()
            cdp.envia("Gap.finish")
            errors = await asyncio.gather(*waits, return_exceptions=True)
            result["closed_waiters"] = {"errors": [{"message": str(error), "repr": repr(error)} for error in errors], "pending": len(cdp._pendentes), "events_remaining": cdp.eventos.qsize(), "elapsed_seconds": time.monotonic() - began}
            silent = [cdp.envia("Input.insertText", {"text": "depois da queda"}),
                      cdp.envia("Page.screencastFrameAck", {"sessionId": 701})]
            result["silent_send_failures"] = {"results": await asyncio.gather(*silent), "pending": len(cdp._pendentes)}
        result["after_exit"] = {"pump_done": cdp._bomba.done(), "loose_pending": sum(not task.done() for task in cdp._soltas), "pending": len(cdp._pendentes)}
    received, changed = [], asyncio.Event()

    async def unanswered(ws):
        async for raw in ws:
            received.append(json.loads(raw))
            if len(received) == 2:
                changed.set()

    async with websockets.serve(unanswered, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with navsock._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            cdp.envia("Input.insertText", {"text": "ação 😀"})
            cdp.envia("Page.screencastFrameAck", {"sessionId": 702})
            await asyncio.wait_for(changed.wait(), 5)
        result["fire_and_forget_exit"] = {"wire": received, "pump_done": cdp._bomba.done(),
                                           "loose_pending": sum(not task.done() for task in cdp._soltas),
                                           "pending": len(cdp._pendentes)}
    return result


async def slow_discovery():
    resources = Cleanup()
    try:
        return await slow_discovery_owned(resources)
    finally:
        await resources.finish(sys.exception())


async def slow_discovery_owned(resources):
    import threading
    import httpx
    import uvicorn
    import websockets
    from app import navsock
    from websockets.exceptions import InvalidStatus
    began, started = asyncio.Event(), asyncio.Event()
    release = threading.Event()
    loop = asyncio.get_running_loop()
    app = FastAPI()

    def exists(_):
        loop.call_soon_threadsafe(began.set)
        if not release.wait(5):
            raise TimeoutError("a prova não liberou a descoberta")
        return False

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def remote(ws: WebSocket, name: str):
        await navsock.nav_ws(ws, name, exists)

    @app.get("/alive")
    async def alive():
        return {"alive": True}

    class Probe(uvicorn.Server):
        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            started.set()

    listener = socket.socket()
    resources.add("socket de descoberta lenta", listener.close, 40)
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    probe = Probe(uvicorn.Config(app, log_level="critical", lifespan="off", timeout_graceful_shutdown=3))
    serving = asyncio.create_task(probe.serve(sockets=[listener]))
    resources.add("sinal de parada da descoberta", lambda: setattr(probe, "should_exit", True), 10)
    resources.add("servidor de descoberta", lambda: finish_tasks([serving], 5), 30)
    resources.add("liberação da descoberta", release.set, 0)

    async def connect():
        try:
            async with websockets.connect(f"ws://127.0.0.1:{port}/api/sessions/{NAME}/nav-remoto?token={OWNER}"):
                raise AssertionError("a sessão ausente não pode abrir")
        except InvalidStatus as error:
            return error.response.status_code

    pending = None
    try:
        await asyncio.wait_for(started.wait(), 5)
        pending = asyncio.create_task(connect())
        resources.add("abertura pendente da descoberta", lambda: finish_tasks([pending], 5), 20)
        await asyncio.wait_for(began.wait(), 5)
        async with httpx.AsyncClient(trust_env=False, timeout=1) as client:
            response = await client.get(f"http://127.0.0.1:{port}/alive")
        observed = {"other_connection_before_release": response.json(), "opening_still_pending": not pending.done()}
        release.set()
        observed["opening_status"] = await asyncio.wait_for(pending, 5)
        return observed
    finally:
        await resources.finish(sys.exception())


async def capture(root):
    measured = json.loads((HERE / "chromium.json").read_text())
    public = await public_cases(root, measured)
    partial = json.loads(os.environ.get("HANGAR_GAP_CASES", "[]"))
    cdp = {} if partial else await cdp_cases()
    discovery = {} if partial else await slow_discovery()
    await asyncio.sleep(0)
    remaining = [task for task in asyncio.all_tasks() if task is not asyncio.current_task() and not task.done()]
    if remaining:
        raise AssertionError(f"tarefas sobreviventes: {[task.get_name() for task in remaining]}")
    return {"public": public, "cdp": cdp, "slow_discovery": discovery, "partial_cases": partial}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--child", type=Path)
    parser.add_argument("--output", type=Path, default=HERE / "transport-gaps.json")
    parser.add_argument("--case", action="append", default=[])
    args = parser.parse_args()
    if args.child:
        assert Path.home() == args.child / "home"
        assert os.environ["PATH"] == str(args.child / "bin")
        assert not any((args.child / "bin").iterdir())
        assert "app" not in sys.modules, "a fonte original foi importada antes da barreira"
        boundary = Boundary(args.child)
        boundary.install()
        original = None
        observed = None
        try:
            observed = asyncio.run(capture(args.child))
        except BaseException as error:
            original = error
        isolation, errors = boundary.finish()
        if errors:
            raise BaseExceptionGroup("falha e recursos da captura original", ([original] if original else []) + errors)
        if original is not None:
            raise original
        observed["isolation_evidence"] = isolation
        (args.child / "result.json").write_text(json.dumps(observed, ensure_ascii=True, allow_nan=False, indent=2) + "\n")
        return
    archive = subprocess.check_output(["git", "archive", SOURCE, "backend/app"], cwd=REPO)
    with tempfile.TemporaryDirectory(prefix="hangar-transport-gaps-original-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir()
        for name in ("bin", "tmp", "workspace"):
            (root / name).mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            source.extractall(root, filter="data")
        env = {"PATH": str(root / "bin"), "HOME": str(home), "USERPROFILE": str(home),
               "XDG_CONFIG_HOME": str(home / ".config"), "XDG_CACHE_HOME": str(home / ".cache"),
               "XDG_DATA_HOME": str(home / ".local/share"), "CLAUDE_CONFIG_DIR": str(home / ".claude"),
               "CODEX_HOME": str(home / ".codex"), "CP_SCAN_ROOTS": str(root / "workspace"),
               "CP_PROJECTS_DIR": str(root / "workspace"), "CP_AUTH_TOKEN": OWNER,
               "PYTHONPATH": os.pathsep.join([str(root / "backend"), *filter(None, os.environ.get("PYTHONPATH", "").split(os.pathsep))]),
               "TMPDIR": str(root / "tmp"), "PYTHONDONTWRITEBYTECODE": "1",
               "NO_PROXY": "127.0.0.1,localhost", "PYTHONNOUSERSITE": "1"}
        env["HANGAR_GAP_CASES"] = json.dumps(args.case)
        subprocess.run([sys.executable, str(Path(__file__).resolve()), "--child", str(root)],
                       cwd=root, env=env, timeout=180, check=True)
        observed = json.loads((root / "result.json").read_text())
        hashes = {f"backend/app/{name}.py": hashlib.sha256((root / "backend/app" / f"{name}.py").read_bytes()).hexdigest() for name in ("navsock", "navshell")}
    output = {"source_commit": SOURCE, "source_sha256": hashes,
              "measured_input_sha256": hashlib.sha256((HERE / "chromium.json").read_bytes()).hexdigest(),
              "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_transport_gaps.py",
              "python_version": sys.version.split()[0], "websockets_version": __import__("websockets").__version__,
              "platform": sys.platform, "isolation": observed.pop("isolation_evidence"),
              "cdp_peer_extensions": [],
              "comparison": {"complete_fields": ["expected", "expected_encoded"],
                             "measured_fields": ["public[].observations.burst_seconds", "public[].observations.before_rate_boundary_seconds", "public[].observations.after_rate_boundary_seconds", "public[].observations.after_screencast_seconds", "cdp.closed_waiters.elapsed_seconds"],
                             "burst_seconds": [0, 1 / 24], "after_screencast_seconds": [1.4, 2.5],
                             "before_rate_boundary_seconds": [0.079, 1 / 12], "after_rate_boundary_seconds": [0.1, 0.2],
                             "closed_waiters_seconds": [0, 2],
                             "encoded": "Árvore integral tipada: strings em pontos de código, inteiros decimais, floats IEEE754 big-endian; conserva NaN/Infinity/surrogates sem JSON inválido."},
              "limits": ["Preparação do gerador não é execução; JSON só existe depois de executar esta fonte.",
                         "Linux; não representa Electron/WebView2, SSE real, desktop ou sessão do usuário.",
                         "nav_ws original usa seu parâmetro público existe somente para a sessão temporária; Rust usa descoberta real.",
                         "Rajada exige janela real menor que 1/24 s; lentidão do ensaio falha explicitamente, sem fabricar relógio.",
                         "NS070 conserva a serialização do mesmo leitor; outro espectador progride com captura e comando recebidos pelo caminho migrado pendentes.",
                         "Casos especiais são caracterização de efeitos completos, inclusive divergências que exigem decisão do coordenador."],
              **observed}
    args.output.write_text(json.dumps(output, ensure_ascii=True, allow_nan=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
