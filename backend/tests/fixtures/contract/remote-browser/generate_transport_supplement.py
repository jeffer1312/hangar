"""Caracterização adicional do navsock original em processos e portas descartáveis."""
from __future__ import annotations

import argparse
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import types

SOURCE = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]


async def capture(source: Path) -> dict:
    import websockets
    from websockets.exceptions import ConnectionClosed
    module = types.ModuleType("original_navsock")
    exec(compile(source.read_text(), str(source), "exec"), module.__dict__)
    measured = json.loads((HERE / "chromium.json").read_text())
    result = {}
    result["query_numbers"] = []
    for name, params in [
        ("width_huge", {"w": "9" * 100}), ("quality_huge", {"q": "9" * 100}),
        ("width_negative_huge", {"w": "-" + "9" * 100}),
        ("quality_negative_huge", {"q": "-" + "9" * 100}),
        ("width_underscores", {"w": "1_000"}), ("quality_underscores", {"q": "8_0"}),
        ("width_bad_underscores", {"w": "1__0"}), ("quality_bad_underscores", {"q": "8__0"}),
    ]:
        ws = types.SimpleNamespace(query_params=params)
        result["query_numbers"].append({"name": name, "params": params,
                                         "width": module._largura_pedida(ws),
                                         "quality": module._qualidade({"qualidade": module._qualidade_pedida(ws)})})
    result["input_numeric_separators"] = []

    class InputSink:
        def __init__(self):
            self.commands = []

        def envia(self, method, params):
            self.commands.append({"method": method, "params": params})

    for name, message in [
        ("mouse_valid_modifier", {"t": "m", "acao": "press", "mod": "1_0"}),
        ("mouse_invalid_modifier", {"t": "m", "acao": "press", "mod": "1__0"}),
        ("key_invalid_code", {"t": "k", "key": "custom", "code": "_1"}),
        ("wheel_valid_delta", {"t": "w", "dy": "1_0.5"}),
        ("wheel_invalid_delta", {"t": "w", "dy": "1__0.5"}),
        ("mouse_valid_coordinate", {"t": "m", "acao": "press", "x": "0_0.5"}),
        ("mouse_invalid_coordinate", {"t": "m", "acao": "press", "x": "0__0.5"}),
    ]:
        sink = InputSink()
        case = {"name": name, "message": message}
        try:
            await module._aplicar_entrada(sink, message, 800, 600)
        except Exception as error:
            case["error"] = {"type": type(error).__name__, "message": str(error), "repr": repr(error)}
        case["commands"] = sink.commands
        result["input_numeric_separators"].append(case)

    import uvicorn
    from fastapi import FastAPI, WebSocket
    from websockets.exceptions import InvalidStatus
    app = FastAPI()

    @app.websocket("/api/sessions/missing/nav-remoto")
    async def missing(ws: WebSocket):
        await module.nav_ws(ws, "missing", lambda _: False)

    started = asyncio.Event()

    class Probe(uvicorn.Server):
        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            started.set()

    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    probe = Probe(uvicorn.Config(app, host="127.0.0.1", port=port, lifespan="off", log_level="critical"))
    serving = asyncio.create_task(probe.serve(sockets=[listener]))
    try:
        await asyncio.wait_for(started.wait(), 3)
        try:
            async with websockets.connect(f"ws://127.0.0.1:{port}/api/sessions/missing/nav-remoto?token=isolated-original"):
                raise AssertionError("sessão ausente não pode aceitar")
        except InvalidStatus as error:
            result["preaccept_http"] = {"status": error.response.status_code,
                                        "body_hex": bytes(error.response.body or b"").hex()}
    finally:
        probe.should_exit = True
        await asyncio.wait_for(serving, 3)
        listener.close()

    async def no_response(ws):
        try:
            async for _ in ws:
                pass
        except ConnectionClosed:
            pass

    async with websockets.serve(no_response, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            began = time.monotonic()
            try:
                await cdp.cmd("Page.enable")
                raise AssertionError("comando sem resposta não expirou")
            except TimeoutError as error:
                result["command_timeout"] = {"message": str(error), "repr": repr(error),
                                             "pending_after_timeout": len(cdp._pendentes),
                                             "elapsed_seconds": time.monotonic() - began}

    async def close_after_start(ws):
        try:
            async for raw in ws:
                message = json.loads(raw)
                await ws.send(json.dumps({"id": message["id"], "result": {}}))
                if message["method"] == "Page.startScreencast":
                    await ws.close()
        except ConnectionClosed:
            pass

    class Screen:
        async def send_text(self, raw):
            raise AssertionError("não há quadro na conexão fechada")

    async with websockets.serve(close_after_start, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            try:
                await asyncio.wait_for(module._fluxo_de_quadros(cdp, Screen(), {"w": 1280, "h": 800}), 5)
                raise AssertionError("CDP fechado deixou captura aberta")
            except ConnectionError as error:
                result["closed_cdp"] = {"message": str(error), "repr": repr(error)}

    async def burst(ws):
        try:
            async for raw in ws:
                message = json.loads(raw)
                for index in range(100):
                    await ws.send(json.dumps({"method": "Page.frameNavigated", "params": {
                        "frame": {"url": f"http://contract.test/{index}"}}}))
                await ws.send(json.dumps({"method": "Page.screencastFrame", "params": measured["frame"],
                                          "transport_padding": "x" * (1024 * 1024 + 1)}))
                await ws.send(json.dumps({"id": message["id"], "result": {}}))
        except ConnectionClosed:
            pass

    async with websockets.serve(burst, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            await cdp.cmd("Page.enable")
            events = []
            while not cdp.eventos.empty():
                events.append(cdp.eventos.get_nowait())
            result["queue_pressure"] = {"count": len(events), "first": events[0],
                                        "last_method": events[-1]["method"],
                                        "last_frame": events[-1]["params"],
                                        "padding_characters": len(events[-1]["transport_padding"])}

    capture_started = asyncio.Event()
    inserted = asyncio.get_running_loop().create_future()

    async def pending_screenshot(ws):
        try:
            async for raw in ws:
                message = json.loads(raw)
                if message["method"] == "Page.captureScreenshot":
                    capture_started.set()
                    continue
                if message["method"] == "Input.insertText":
                    inserted.set_result(message["params"])
                await ws.send(json.dumps({"id": message["id"], "result": {}}))
        except ConnectionClosed:
            pass

    async with websockets.serve(pending_screenshot, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            flow = asyncio.create_task(module._fluxo_de_quadros(cdp, Screen(), {"w": 1280, "h": 800}))
            await asyncio.wait_for(capture_started.wait(), 3)
            await module._aplicar_entrada(cdp, {"t": "txt", "s": "ação 😀"}, 1280, 800)
            result["concurrent_input"] = await asyncio.wait_for(inserted, 1)
            result["screenshot_still_waiting"] = not flow.done()
            flow.cancel()
            await asyncio.gather(flow, return_exceptions=True)

    snapshots = []
    snapshot_times = []
    six_snapshots = asyncio.Event()

    class SnapshotScreen:
        async def send_text(self, raw):
            snapshots.append(json.loads(raw))
            snapshot_times.append(time.monotonic())
            if len(snapshots) == 6:
                six_snapshots.set()

    async def screenshots(ws):
        try:
            async for raw in ws:
                message = json.loads(raw)
                response = measured["screenshot"] if message["method"] == "Page.captureScreenshot" else {}
                await ws.send(json.dumps({"id": message["id"], "result": response}))
        except ConnectionClosed:
            pass

    async with websockets.serve(screenshots, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            flow = asyncio.create_task(module._fluxo_de_quadros(cdp, SnapshotScreen(), {"w": 1280, "h": 800}))
            await asyncio.wait_for(six_snapshots.wait(), 6)
            flow.cancel()
            await asyncio.gather(flow, return_exceptions=True)
            result["screenshot_cadence"] = {"frames": len(snapshots), "first_event": snapshots[0],
                                             "between_first_and_last_seconds": snapshot_times[-1] - snapshot_times[0]}

    main_frame_seen = asyncio.Event()
    navigation_events = []

    class NavigationScreen:
        async def send_text(self, raw):
            event = json.loads(raw)
            navigation_events.append(event)
            if event.get("url") == "http://contract.test/top":
                main_frame_seen.set()

    async def frames(ws):
        try:
            async for raw in ws:
                message = json.loads(raw)
                await ws.send(json.dumps({"id": message["id"], "result": {}}))
                if message["method"] == "Page.startScreencast":
                    await ws.send(json.dumps({"method": "Page.frameNavigated", "params": {
                        "frame": {"parentId": "main", "url": "http://contract.test/iframe"}}}))
                    await ws.send(json.dumps({"method": "Page.frameNavigated", "params": {
                        "frame": {"url": "http://contract.test/top"}}}))
        except ConnectionClosed:
            pass

    async with websockets.serve(frames, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            flow = asyncio.create_task(module._fluxo_de_quadros(cdp, NavigationScreen(), {"w": 1280, "h": 800}))
            await asyncio.wait_for(main_frame_seen.wait(), 3)
            flow.cancel()
            await asyncio.gather(flow, return_exceptions=True)
            result["subframe_urls"] = navigation_events

    ping_result = asyncio.get_running_loop().create_future()

    async def raw_browser(reader, writer):
        head = await reader.readuntil(b"\r\n\r\n")
        key = next(line.split(b":", 1)[1].strip() for line in head.split(b"\r\n")
                   if line.lower().startswith(b"sec-websocket-key:"))
        accept = base64.b64encode(hashlib.sha1(key + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest())
        writer.write(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " + accept + b"\r\n\r\n")
        await writer.drain()
        first_ping = None
        try:
            while True:
                head = await reader.readexactly(2)
                length = head[1] & 127
                if length == 126:
                    length = int.from_bytes(await reader.readexactly(2), "big")
                elif length == 127:
                    length = int.from_bytes(await reader.readexactly(8), "big")
                mask = await reader.readexactly(4) if head[1] & 128 else b"\0" * 4
                body = bytes(value ^ mask[index % 4] for index, value in enumerate(await reader.readexactly(length)))
                if head[0] & 15 == 9 and first_ping is None:
                    first_ping = time.monotonic()
                elif head[0] & 15 == 8:
                    ping_result.set_result({"code": int.from_bytes(body[:2], "big"),
                                            "reason": body[2:].decode(),
                                            "seconds_since_ping": time.monotonic() - first_ping})
                    writer.write(bytes([0x88, len(body)]) + body)
                    await writer.drain()
                    break
        finally:
            writer.close()
            await writer.wait_closed()

    async with await asyncio.start_server(raw_browser, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        async with module._Cdp(f"ws://127.0.0.1:{port}") as cdp:
            result["default_close_timeout_seconds"] = cdp._ws.close_timeout
            result["ping_timeout"] = await asyncio.wait_for(ping_result, 43)

    await asyncio.sleep(0)
    remaining = [task for task in asyncio.all_tasks() if task is not asyncio.current_task() and not task.done()]
    if remaining:
        raise AssertionError(f"tarefas sobreviventes: {[task.get_name() for task in remaining]}")
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--child", type=Path)
    parser.add_argument("--output", type=Path, default=HERE / "transport-supplement.json")
    args = parser.parse_args()
    if args.child:
        result = asyncio.run(capture(args.child / "navsock.py"))
        (args.child / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
        return
    raw = subprocess.check_output(["git", "show", f"{SOURCE}:backend/app/navsock.py"], cwd=REPO)
    with tempfile.TemporaryDirectory(prefix="hangar-nav-original-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir()
        (root / "navsock.py").write_bytes(raw)
        env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "USERPROFILE": str(home),
               "XDG_CONFIG_HOME": str(home / ".config"), "XDG_CACHE_HOME": str(home / ".cache"),
               "XDG_DATA_HOME": str(home / ".local/share"), "CLAUDE_CONFIG_DIR": str(home / ".claude"),
               "CODEX_HOME": str(home / ".codex"), "CP_SCAN_ROOTS": str(root / "workspace"),
               "CP_PROJECTS_DIR": str(root / "workspace"), "CP_AUTH_TOKEN": "isolated-original",
               "PYTHONPATH": str(REPO / "backend"), "PYTHONDONTWRITEBYTECODE": "1",
               "NO_PROXY": "127.0.0.1,localhost", "PYTHONNOUSERSITE": "1"}
        subprocess.run([sys.executable, str(Path(__file__).resolve()), "--child", str(root)],
                       cwd=root, env=env, timeout=75, check=True)
        result = json.loads((root / "result.json").read_text())
    output = {"source_commit": SOURCE, "source_sha256": hashlib.sha256(raw).hexdigest(),
              "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_transport_supplement.py",
              "python_version": sys.version.split()[0], "websockets_version": __import__("websockets").__version__,
              "platform": sys.platform, "isolation": "filho env_clear, HOME/config/cwd temporários; CDP de ensaio em portas próprias",
              "comparison": {"measured_fields": ["observed.command_timeout.elapsed_seconds", "observed.ping_timeout.seconds_since_ping", "observed.screenshot_cadence.between_first_and_last_seconds"],
                             "command_timeout_seconds": [9.9, 12], "ping_timeout_seconds": [19.9, 23], "five_screenshot_intervals_seconds": [2, 3.2]},
              "limits": ["Linux; não representa Electron/WebView2 ou sessão Hangar real.",
                         "A pressão usa quadro medido e padding de transporte declarado; não inventa JPEG.",
                         "O prazo do Ping e o prazo de comando foram executados com os defaults reais, sem relógio simulado.",
                         "A concorrência observa Input.insertText na borda externa durante screenshot pendente; não espera URL do emissor ocupado."],
              "observed": result}
    args.output.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
