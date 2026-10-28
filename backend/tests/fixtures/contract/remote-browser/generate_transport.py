"""Caracteriza o WebSocket original com desktop de ensaio em loopback e quadro real congelado."""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import copy
import hashlib
import io
import json
import logging
import os
from pathlib import Path
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlencode

from fastapi import FastAPI, Request, WebSocket


SOURCE = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
TOKEN = "remote-browser-contract-owner"
DESKTOP_TOKEN = "remote-browser-contract-desktop"
NAME = "nav-contract"
KEY = "contract::nav-contract"
BARRIER = "__remote_browser_contract_barrier__"
DEADLINE = 20


class CaptureLog(logging.Handler):
    def __init__(self, root: Path):
        super().__init__()
        self.root = root
        self.rows: list[dict] = []

    def emit(self, record):
        self.rows.append({"level": record.levelname,
                          "message": record.getMessage().replace(str(self.root), "$ROOT")})


def isolate(root: Path) -> None:
    for key in list(os.environ):
        upper = key.upper()
        if (upper.startswith(("CP_", "HANGAR_", "CLAUDE_", "CODEX_", "TMUX", "XDG_"))
                or upper.endswith(("_TOKEN", "_KEY", "_SECRET", "_PASSWORD"))
                or upper in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "PYTHONPATH")):
            os.environ.pop(key, None)
    home = root / "home"
    home.mkdir()
    os.environ.update(HOME=str(home), USERPROFILE=str(home), HOMEDRIVE=home.drive,
                      HOMEPATH=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                      XDG_CACHE_HOME=str(home / ".cache"), XDG_DATA_HOME=str(home / ".local/share"),
                      CP_AUTH_TOKEN=TOKEN, CP_LAN_BIND_IP="127.0.0.1", CP_UPDATE_BRANCH="",
                      GIT_CONFIG_GLOBAL=os.devnull, TZ="UTC", LC_ALL="C.UTF-8")
    os.chdir(root)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


class Desktop:
    def __init__(self, frame: dict, screenshot: dict):
        self.frame = frame
        self.screenshot = screenshot
        self.mode = "frames"
        self.pid = 123
        self.token = DESKTOP_TOKEN
        self.layout = "mobile"
        self.refuse = ""
        self.fail_original = False
        self.fail_restore = False
        self.input_error = False
        self.capture_errors = 0
        self.commands: list[dict] = []
        self.requests: list[dict] = []
        self.peers: list = []
        self.peer_keys: list[str] = []
        self.finished: list[asyncio.Event] = []
        self.condition = asyncio.Condition()

    async def changed(self):
        async with self.condition:
            self.condition.notify_all()

    async def wait(self, predicate):
        async with asyncio.timeout(DEADLINE):
            async with self.condition:
                await self.condition.wait_for(predicate)

    async def cdp(self, ws):
        from starlette.websockets import WebSocketDisconnect
        if ws.headers.get("authorization") != f"Bearer {DESKTOP_TOKEN}":
            await ws.close(code=1008)
            return
        await ws.accept()
        index = len(self.peers)
        self.peers.append(ws)
        self.peer_keys.append(ws.query_params.get("chave", ""))
        await self.changed()
        acknowledged = False
        try:
            while True:
                raw = await ws.receive_text()
                message = json.loads(raw)
                self.commands.append({"peer": index, "message": message})
                await self.changed()
                method, params = message["method"], message.get("params") or {}
                result = {"id": message["id"], "result": {}}
                if method == "Page.captureScreenshot":
                    if self.capture_errors:
                        self.capture_errors -= 1
                        result = {"id": message["id"], "error": {
                            "code": -32000, "message": "capture refused by fixture"}}
                    else:
                        result["result"] = copy.deepcopy(self.screenshot)
                elif method.startswith("Input.") and self.input_error:
                    result = {"id": message["id"], "error": {
                        "code": -32000, "message": "input refused by fixture"}}
                await ws.send_text(json.dumps(result))
                if method == "Page.startScreencast" and self.mode == "frames":
                    await ws.send_text(json.dumps({"method": "Page.screencastFrame",
                                                   "params": copy.deepcopy(self.frame)}))
                elif method == "Page.screencastFrameAck" and not acknowledged:
                    acknowledged = True
                    await ws.send_text(json.dumps({"method": "Page.frameNavigated", "params": {
                        "frame": {"url": "http://contract.test/ready"}}}))
                elif method == "Input.insertText" and params.get("text") == BARRIER:
                    await ws.send_text(json.dumps({"method": "Page.frameNavigated", "params": {
                        "frame": {"url": "http://contract.test/barrier"}}}))
        except WebSocketDisconnect:
            pass

    async def command(self, request):
        from starlette.responses import PlainTextResponse
        payload = await request.json()
        self.requests.append({"authorization": request.headers.get("authorization"), "body": payload,
                              "receiver_pid": self.pid})
        await self.changed()
        if request.headers.get("authorization") != f"Bearer {self.token}":
            return PlainTextResponse("unauthorized", status_code=401)
        args = payload.get("args") or []
        if not args and self.fail_original:
            return PlainTextResponse("erro: layout indisponível")
        if args:
            proposed = "x".join(args) if len(args) == 2 else args[0]
            if proposed == self.refuse or (self.fail_restore and proposed == "mobile"):
                return PlainTextResponse(f"erro: layout {proposed}: recusado")
            self.layout = proposed
        return PlainTextResponse(f"layout: {self.layout}")

    async def detach(self, index: int, reason: str):
        await self.peers[index].send_text(json.dumps({"method": "Inspector.detached",
                                                    "params": {"reason": reason}}))

    async def finish(self, index: int):
        async with asyncio.timeout(DEADLINE):
            await self.finished[index].wait()

    def result(self, events: list[dict]) -> dict:
        return {"events": events, "cdp": self.commands, "cmd": self.requests,
                "desktop_layout": self.layout, "cdp_keys": self.peer_keys}


async def receive_until(ws, events: list[dict], predicate):
    async with asyncio.timeout(DEADLINE):
        while True:
            item = json.loads(await ws.recv())
            events.append(item)
            if predicate(item):
                return item


async def receive_ready(ws, events):
    return await receive_until(ws, events, lambda item: item == {
        "t": "url", "url": "http://contract.test/ready"})


async def barrier(ws, events):
    await ws.send(json.dumps({"t": "txt", "s": BARRIER}))
    return await receive_until(ws, events, lambda item: item == {
        "t": "url", "url": "http://contract.test/barrier"})


def input_cases():
    cases = [
        ("mouse_center", {"t": "m", "acao": "press", "x": .5, "y": .5}, ["NS040", "NS041"]),
        ("mouse_release_clamped", {"t": "m", "acao": "release", "x": -1, "y": 2}, ["NS040", "NS041"]),
        ("mouse_move_modifiers", {"t": "m", "acao": "move", "x": .3, "y": .7, "mod": 5}, ["NS041"]),
        ("mouse_unknown_action", {"t": "m", "acao": "click", "x": .5, "y": .5}, ["NS047"]),
        ("mouse_missing_position", {"t": "m", "acao": "press"}, ["NS040", "NS041"]),
        ("mouse_numeric_strings", {"t": "m", "acao": "press", "x": " 5e-1 ", "y": "0.25", "mod": "2"}, ["NS040", "NS066"]),
        ("mouse_invalid_position", {"t": "m", "acao": "press", "x": "invalid", "y": None}, ["NS040"]),
        ("wheel_fractional", {"t": "w", "x": .25, "y": .75, "dx": -1.5, "dy": 220.5}, ["NS042"]),
        ("wheel_defaults", {"t": "w"}, ["NS042"]),
        ("text_unicode", {"t": "txt", "s": "ação 😀 e\u0301\nsegunda linha"}, ["NS043", "NS067"]),
        ("text_empty", {"t": "txt", "s": ""}, ["NS043"]),
        ("text_null", {"t": "txt", "s": None}, ["NS043", "NS066"]),
        ("text_number", {"t": "txt", "s": 123}, ["NS043", "NS066"]),
        ("text_limit_unicode", {"t": "txt", "s": "😀" * 4097}, ["NS043", "NS067"]),
        ("text_limit_combining", {"t": "txt", "s": "e\u0301" * 2049}, ["NS043", "NS067"]),
        ("key_custom", {"t": "k", "key": "a", "code": 65, "mod": 2}, ["NS044", "NS045"]),
        ("key_empty", {"t": "k", "key": ""}, ["NS044"]),
        ("key_long_unicode", {"t": "k", "key": "😀" * 33}, ["NS044", "NS067"]),
        ("unknown_message", {"t": "unknown"}, ["NS047"]),
        ("missing_message_type", {}, ["NS047"]),
    ]
    for button in ("right", "middle", "none", "unknown", None):
        cases.append((f"mouse_button_{button}", {"t": "m", "acao": "press", "b": button}, ["NS041"]))
    for key in ("Enter", "Tab", "Escape", "Backspace", "Delete", "ArrowUp", "ArrowDown",
                "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown"):
        cases.append((f"key_{key}", {"t": "k", "key": key}, ["NS045"]))
    return cases


async def characterize(root: Path, measured: dict) -> list[dict]:
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect
    import uvicorn
    import websockets
    from app import auth, navsock

    # O prazo da janela de autenticação não pode depender da carga da máquina de prova.
    navsock.time = SimpleNamespace(time=lambda: 1000.0, monotonic=time.monotonic)
    auth.time = SimpleNamespace(time=lambda: 1000.0)

    nav = Path.home() / ".hangar" / "nav"
    nav.mkdir(parents=True)
    cases = []
    log = CaptureLog(root)
    source_logger = logging.getLogger("hangar.navsock")
    source_logger.addHandler(log)
    app = FastAPI()
    current: Desktop | None = None

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def remote(ws: WebSocket, name: str):
        assert current is not None
        desktop = current
        index = len(desktop.finished)
        finished = asyncio.Event()
        desktop.finished.append(finished)
        try:
            await navsock.nav_ws(ws, name, lambda value: value == NAME)
        finally:
            finished.set()
            await desktop.changed()

    @app.websocket("/cdp")
    async def cdp(ws: WebSocket):
        assert current is not None
        await current.cdp(ws)

    @app.post("/cmd")
    async def command(request: Request):
        assert current is not None
        return await current.command(request)

    class Server(uvicorn.Server):
        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            started.set()

    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    started = asyncio.Event()
    server = Server(uvicorn.Config(app, host="127.0.0.1", port=port, log_level="critical",
                                   lifespan="off", ws="websockets-sansio", timeout_graceful_shutdown=3))
    running = asyncio.create_task(server.serve(sockets=[listener]))
    original_connect = socket.socket.connect

    def loopback_only(sock, address):
        if isinstance(address, tuple) and (address[0] not in ("127.0.0.1", "::1") or address[1] != port):
            raise PermissionError("a caracterização só permite sua própria porta de loopback")
        return original_connect(sock, address)

    socket.socket.connect = loopback_only
    try:
        async with asyncio.timeout(DEADLINE):
            await started.wait()

        def reset():
            nonlocal current
            current = Desktop(copy.deepcopy(measured["frame"]), copy.deepcopy(measured["screenshot"]))
            for path in nav.iterdir():
                path.unlink()
            write_json(nav / "browser.json", {"chave": KEY, "url": "http://contract.test/original",
                                               "targetId": None, "pid": 123, "ts": 1,
                                               "ativa": 1, "abas": []})
            write_json(nav / "_srv.json", {"porta": port, "token": DESKTOP_TOKEN, "pid": 123})
            auth.reset_backoff()
            log.rows.clear()
            return current

        def record(name, ids, inputs, result, limits=None):
            result["log"] = copy.deepcopy(log.rows)
            cases.append({"name": name, "ids": ids, "inputs": inputs, "expected": result,
                          "seam": "WS público → nav_ws original → CDP e /cmd de loopback",
                          "limits": limits or []})

        def uri(query="", name=NAME):
            return f"ws://127.0.0.1:{port}/api/sessions/{name}/nav-remoto?token={TOKEN}" + query

        async def disconnect(ws, desktop, index=0):
            await ws.close()
            await desktop.finish(index)

        for name, message, ids in input_cases():
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(json.dumps(message, ensure_ascii=False))
                await barrier(ws, events)
                await disconnect(ws, desktop)
            record(name, ids + ["NS001", "NS008", "NS027", "NS028", "NS031", "NS032", "NS048", "NS068"],
                   {"messages": [message, {"t": "txt", "s": BARRIER}]}, desktop.result(events))

        raw_cases = [("invalid_json", "{"), ("json_array", "[]"), ("json_null", "null"),
                     ("json_number", "123"), ("json_string", '"texto"')]
        payload = json.dumps({"t": "txt", "s": "😀"}, ensure_ascii=False)
        raw_cases.extend([("message_at_character_limit", payload + " " * (65536 - len(payload))),
                          ("message_over_character_limit", payload + " " * (65537 - len(payload)))])
        for name, raw in raw_cases:
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(raw)
                await barrier(ws, events)
                await disconnect(ws, desktop)
            record(name, ["NS037", "NS038", "NS043", "NS067"],
                   {"raw_messages": [raw], "barrier": BARRIER}, desktop.result(events))

        for query in ("", "&q=0", "&q=-1", "&q=invalid", "&q=1", "&q=30", "&q=80", "&q=100",
                      "&w=0", "&w=-1", "&w=invalid", "&w=390", "&w=1401"):
            desktop = reset()
            events = []
            async with websockets.connect(uri(query), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await disconnect(ws, desktop)
            record("query_" + (query.lstrip("&").replace("=", "_") or "default"),
                   ["NS017", "NS018", "NS029"], {"query": query}, desktop.result(events))

        for name, message in [
            ("layout_size", {"t": "layout", "w": 900, "h": 700}),
            ("layout_size_minimum", {"t": "layout", "w": 1, "h": 1}),
            ("layout_size_maximum", {"t": "layout", "w": 8192, "h": 8192}),
            ("layout_size_zero", {"t": "layout", "w": 0, "h": 700}),
            ("layout_size_above_limit", {"t": "layout", "w": 8193, "h": 700}),
            ("layout_missing_height", {"t": "layout", "w": 900}),
            ("layout_invalid_type", {"t": "layout", "w": "invalid", "h": 700}),
            ("layout_mode_mobile", {"t": "layout", "modo": "mobile"}),
            ("layout_mode_desktop", {"t": "layout", "modo": "desktop"}),
            ("layout_mode_invalid", {"t": "layout", "modo": "unknown"}),
            ("layout_dimensions_beat_mode", {"t": "layout", "modo": "desktop", "w": None}),
        ]:
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(json.dumps(message))
                await barrier(ws, events)
                await disconnect(ws, desktop)
            record(name, ["NS039", "NS052", "NS054", "NS056", "NS058", "NS059", "NS060"],
                   {"messages": [message], "barrier": BARRIER}, desktop.result(events))

        for name, attr, value in [("layout_refused", "refuse", "900x700"),
                                  ("layout_original_unavailable", "fail_original", True),
                                  ("layout_restore_refused", "fail_restore", True)]:
            desktop = reset()
            setattr(desktop, attr, value)
            events = []
            message = {"t": "layout", "w": 900, "h": 700}
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(json.dumps(message))
                await barrier(ws, events)
                await disconnect(ws, desktop)
            record(name, ["NS055", "NS056", "NS061"],
                   {"messages": [message], "desktop_fault": {attr: value}}, desktop.result(events),
                   ["Falha injetada na fronteira externa /cmd; não muda a implementação Python."])

        for reason in ("replaced_by_another_viewer", "target_closed"):
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await desktop.detach(0, reason)
                await receive_until(ws, events, lambda item: item.get("t") == "erro")
                await desktop.finish(0)
                close = {"code": ws.close_code, "reason": ws.close_reason}
                if close["code"] is None:
                    await ws.wait_closed()
                    close = {"code": ws.close_code, "reason": ws.close_reason}
            result = desktop.result(events)
            result["close"] = close
            record("detached_" + reason, ["NS034", "NS049", "NS050", "NS051", "NS064"],
                   {"detached_reason": reason}, result)

        for name, error_count in (("screenshots_continue", 0), ("screenshot_error_is_silent", 1)):
            desktop = reset()
            desktop.mode = "screenshots"
            desktop.capture_errors = error_count
            events = []
            async with websockets.connect(uri("&q=80"), max_size=32 * 1024 * 1024) as ws:
                await receive_until(ws, events, lambda item: item.get("t") == "q" and item.get("lento"))
                if not error_count:
                    await receive_until(ws, events, lambda item: item.get("t") == "q" and item.get("lento"))
                await barrier(ws, events)
                await disconnect(ws, desktop)
            record(name, ["NS018", "NS030", "NS035", "NS036"],
                   {"query": "q=80", "capture_errors": error_count,
                    "capture_data_source": "chromium.json:screenshot"}, desktop.result(events),
                   ["Sem asserção de tempo real: aguarda quadros por condição; teto só detecta travamento."])

        desktop = reset()
        desktop.input_error = True
        events = []
        async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
            await receive_ready(ws, events)
            await ws.send(json.dumps({"t": "k", "key": "Enter"}))
            await barrier(ws, events)
            await disconnect(ws, desktop)
        record("cdp_input_error_is_silent", ["NS020", "NS025"],
               {"messages": [{"t": "k", "key": "Enter"}], "input_error": True}, desktop.result(events),
               ["Resposta CDP de erro injetada na fronteira; o transporte original continua aberto."])

        for name, message in [("invalid_modifier_closes", {"t": "k", "key": "Enter", "mod": "invalid"}),
                              ("invalid_wheel_delta_closes", {"t": "w", "dy": "invalid"})]:
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(json.dumps(message))
                await receive_until(ws, events, lambda item: item.get("t") == "erro")
                await desktop.finish(0)
            record(name, ["NS046", "NS050", "NS066"], {"messages": [message]}, desktop.result(events),
                   ["Entrada fora dos tipos emitidos pelos clientes atuais; não é defeito de uso real afirmado."])

        # A saída do canal antigo pode chegar depois de o desktop publicar fontes novas.
        for name, new_key in (("same_name_new_browser_receives_old_restore", "new-server::nav-contract"),
                              ("same_key_new_desktop_receives_old_restore", KEY)):
            desktop = reset()
            events = []
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as ws:
                await receive_ready(ws, events)
                await ws.send(json.dumps({"t": "layout", "w": 900, "h": 700}))
                await barrier(ws, events)
                desktop.pid = 456
                desktop.token = DESKTOP_TOKEN + "-new"
                write_json(nav / "browser.json", {"chave": new_key, "url": "http://contract.test/new",
                                                   "targetId": None, "pid": desktop.pid})
                write_json(nav / "_srv.json", {"porta": port, "token": desktop.token, "pid": desktop.pid})
                await desktop.detach(0, "target_closed")
                await receive_until(ws, events, lambda item: item.get("t") == "erro")
                await desktop.finish(0)
            record(name, ["NS062", "NS063", "NS065"],
                   {"messages": [{"t": "layout", "w": 900, "h": 700}],
                    "source_change": {"key": new_key, "old_desktop_pid": 123, "new_desktop_pid": 456,
                                      "new_desktop_token": desktop.token,
                                      "ending_old_cdp": "Inspector.detached:target_closed"}}, desktop.result(events),
                   ["A identidade do desktop é controlada na fronteira; não executa processos desktop nem fecha/reabre sessão Hangar.",
                    "A entrega tardia do fim CDP acontece após a nova fonte; receiver_pid identifica o destinatário do /cmd."])

        desktop = reset()
        events_a, events_b = [], []
        async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as first:
            await receive_ready(first, events_a)
            await first.send(json.dumps({"t": "layout", "w": 900, "h": 700}))
            await barrier(first, events_a)
            async with websockets.connect(uri(), max_size=32 * 1024 * 1024) as second:
                await receive_ready(second, events_b)
                await second.send(json.dumps({"t": "layout", "w": 900, "h": 700}))
                await barrier(second, events_b)
                await disconnect(first, desktop, 0)
                after_old = desktop.layout
                await disconnect(second, desktop, 1)
        result = desktop.result([])
        result.update(events_by_viewer=[events_a, events_b], layout_after_old_viewer=after_old)
        record("old_viewer_leaves_after_same_size_new_viewer", ["NS053", "NS054", "NS059", "NS060"],
               {"layout_by_viewer": [{"w": 900, "h": 700}, {"w": 900, "h": 700}]}, result,
               ["Entrega tardia do fim do espectador anterior é controlada pelo desktop sintético."])

        # A rota mínima usa o ponto de injeção público de existência, sem tmux nem registry reais.
        auth_app = FastAPI()

        @auth_app.websocket("/api/sessions/{name}/nav-remoto")
        async def auth_route(ws: WebSocket, name: str):
            await navsock.nav_ws(ws, name, lambda value: False)

        def auth_case(name, token=None, headers=None, client_ip="198.51.100.2", attempts=1, tokens=None):
            reset()
            tokens = tokens if tokens is not None else [token] * attempts
            outputs = []
            with TestClient(auth_app, client=(client_ip, 1)) as client:
                for value in tokens:
                    query = "?" + urlencode({"token": value}) if value is not None else ""
                    try:
                        with client.websocket_connect("/api/sessions/missing/nav-remoto" + query, headers=headers or {}):
                            raise AssertionError("a sessão ausente não pode aceitar o WebSocket")
                    except WebSocketDisconnect as exc:
                        outputs.append({"code": exc.code, "reason": exc.reason})
            record(name, ["NS009", "NS011", "NS012", "NS013"],
                   {"token_sequence": tokens, "headers": headers or {}, "client_ip": client_ip,
                    "clock_epoch_seconds": 1000},
                   {"closes": outputs}, ["TestClient, sem TCP: preserva o IP do cliente para o limitador real."])

        for name, token, headers in [
            ("auth_missing", None, {}), ("auth_wrong", "wrong", {}),
            ("auth_bearer_only", None, {"authorization": f"Bearer {TOKEN}"}),
            ("auth_cookie_only", None, {"cookie": f"cp_token={TOKEN}"}),
            ("auth_owner_foreign_origin", TOKEN, {"origin": "https://foreign.invalid"}),
        ]:
            auth_case(name, token, headers)
        auth_case("auth_loopback_not_rate_limited", client_ip="127.0.0.1", tokens=["wrong"] * 10 + [TOKEN])
        auth_case("auth_blocked_before_correct_token", tokens=["wrong"] * 8 + [TOKEN])
        auth_case("auth_success_does_not_reset_ws_failures", tokens=["wrong"] * 7 + [TOKEN, "wrong", TOKEN])

        for name, kind in (("opening_missing_browser", "absent"), ("opening_wrong_desktop_pid", "wrong_pid")):
            desktop = reset()
            if kind == "absent":
                (nav / "browser.json").unlink()
            else:
                write_json(nav / "_srv.json", {"porta": port, "token": DESKTOP_TOKEN, "pid": 999})
            with TestClient(app, client=("127.0.0.1", 1)) as client:
                try:
                    with client.websocket_connect(f"/api/sessions/{NAME}/nav-remoto?token={TOKEN}"):
                        raise AssertionError("a ausência de navegador válido deve recusar antes do aceite")
                except WebSocketDisconnect as exc:
                    output = {"close": {"code": exc.code, "reason": exc.reason}}
            record(name, ["NS008", "NS013", "NS014", "NS016"], {"browser_source": kind}, output)

        return cases
    finally:
        socket.socket.connect = original_connect
        source_logger.removeHandler(log)
        server.should_exit = True
        try:
            async with asyncio.timeout(DEADLINE):
                await running
        finally:
            if not running.done():
                running.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await running
            listener.close()
        pending = [task for task in asyncio.all_tasks() if task is not asyncio.current_task() and not task.done()]
        if pending:
            for task in pending:
                task.cancel()
            await asyncio.gather(*pending, return_exceptions=True)
            raise AssertionError(f"tarefas não encerradas pelo ensaio: {[task.get_name() for task in pending]}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", default=SOURCE)
    parser.add_argument("--output", type=Path, default=Path(__file__).with_name("transport.json"))
    parser.add_argument("--chromium", type=Path, default=Path(__file__).with_name("chromium.json"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[5]
    output = args.output.resolve()
    measured_path = args.chromium.resolve()
    measured = json.loads(measured_path.read_text(encoding="utf-8"))
    assert measured["source_commit"] == args.source
    assert measured["frame"]["data"].startswith("/9j/") and measured["screenshot"]["data"].startswith("/9j/")
    commit = subprocess.run(["git", "rev-parse", args.source], cwd=repo,
                            check=True, capture_output=True, text=True).stdout.strip()
    archive = subprocess.run(["git", "archive", "--format=tar", commit, "backend/app"], cwd=repo,
                             check=True, capture_output=True).stdout
    previous_cwd = Path.cwd()
    previous_env = dict(os.environ)
    with tempfile.TemporaryDirectory(prefix="hangar-browser-transport-contract-") as temporary:
        root = Path(temporary)
        reference = root / "reference"
        reference.mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            source.extractall(reference, filter="data")
        try:
            isolate(root)
            sys.path.insert(0, str(reference / "backend"))
            cases = asyncio.run(characterize(root, measured))
            assert not [thread.name for thread in threading.enumerate() if thread is not threading.main_thread()], "thread do ensaio continuou viva"
            by_name = {case["name"]: case for case in cases}
            enter = [row["message"]["params"] for row in by_name["key_Enter"]["expected"]["cdp"]
                     if row["message"]["method"] == "Input.dispatchKeyEvent"]
            assert enter[0]["windowsVirtualKeyCode"] == 13 and enter[0]["text"] == "\r"
            assert enter[1]["type"] == "keyUp" and "text" not in enter[1]
            assert by_name["auth_owner_foreign_origin"]["expected"]["closes"] == [{"code": 1008, "reason": "sessao nao existe"}]
            assert by_name["old_viewer_leaves_after_same_size_new_viewer"]["expected"]["layout_after_old_viewer"] == "900x700"
            assert by_name["old_viewer_leaves_after_same_size_new_viewer"]["expected"]["desktop_layout"] == "mobile"
            payload = {
            "source_commit": commit,
            "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_transport.py --source " + commit,
            "python_version": sys.version.split()[0], "platform": sys.platform,
            "source_sha256": hashlib.sha256((reference / "backend/app/navsock.py").read_bytes()).hexdigest(),
            "measured_frame": {"file": "chromium.json", "sha256": hashlib.sha256(measured_path.read_bytes()).hexdigest(),
                               "browser_version": measured["browser_version"]},
            "isolation": "HOME/config/cwd temporários; fonte git exportada; somente a porta loopback criada pelo ensaio; lifespan desligado",
            "normalization": {"$ROOT": "somente caminho do diretório temporário em mensagens de log"},
            "limits": [
                "Sem backend real, tmux, registry, Electron, WebView2 ou ciclo completo de sessão Hangar.",
                "CDP e controlador sintéticos exercitam protocolo real com JPEG previamente medido em Chromium.",
                "A montagem FastAPI chama nav_ws original e injeta somente o parâmetro público de existência.",
                "Relógio de autenticação fixado em 1000 segundos; relógio monotônico real só controla o fluxo, sem medir desempenho.",
            ],
            "cases": cases,
            }
            text = json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
            if args.check:
                assert output.read_text(encoding="utf-8") == text, "a regeneração divergiu da fixture congelada"
                print(f"Regeneração idêntica: {len(cases)} casos, fonte {commit}")
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_text(text, encoding="utf-8")
                print(f"Caracterização gravada: {len(cases)} casos, fonte {commit}")
        finally:
            os.chdir(previous_cwd)
            os.environ.clear()
            os.environ.update(previous_env)


if __name__ == "__main__":
    main()
