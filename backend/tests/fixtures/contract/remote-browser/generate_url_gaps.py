"""Captura lacunas de seleção e layout somente nas fontes Python originais."""
from __future__ import annotations

import argparse
import ast
import asyncio
import copy
import contextlib
from contextlib import asynccontextmanager
import hashlib
import io
import json
import logging
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.parse import quote

from generate_url_commands import SOURCE, REPO, clean_environment

HERE = Path(__file__).resolve().parent
NAME = "nav-contract"
KEY = "contract::nav-contract"
DESKTOP_TOKEN = "remote-browser-contract-desktop"


def text_file(name, value):
    return {"name": name, "text": json.dumps(value, ensure_ascii=False)}


def browser(key=KEY, url="http://native.test/original", **extra):
    return {"chave": key, "pid": 123, "targetId": None, "url": url, **extra}


def server_file():
    return text_file("_srv.json", {"porta": "$CONTROLLER_PORT", "pid": 123, "token": DESKTOP_TOKEN})


def ordinary_files(value=None):
    return [server_file(), text_file("browser.json", browser() if value is None else value)]


def warning_files():
    # A ordem faz parte da entrada; outra enumeração deve interromper a prova.
    return ordinary_files() + [
        text_file("array.json", []), text_file("null.json", None), text_file("scalar.json", 7),
        {"name": "broken.json", "text": "{"}, {"name": "bytes.json", "bytes_hex": "ff"},
        {"name": "directory.json", "directory": True},
    ]


def selection_cases():
    cases = []

    def add(name, ids, files, *, session=NAME, evaluate=None, phases=None):
        phase = {"session": session, "files": files, "evaluate": evaluate or {"result": {"result": {"value": "http://electron.test/current"}}}}
        cases.append({"name": name, "ids": ids, "kind": "selection", "inputs": {"phases": phases or [phase]}})

    add("ws_directory_absent", ["NS002"], None)
    add("ws_underscore_match_is_ignored", ["NS003"], ordinary_files() + [
        text_file("_matching.json", browser(url="http://underscore.test/ignored")),
    ])
    add("ws_unreadable_and_non_objects_before_valid", ["NS004"], warning_files())
    for order in ("a_b", "b_a"):
        entries = {"a": text_file("a.json", browser(NAME, "http://first.test/exact")),
                   "b": text_file("b.json", browser(KEY, "http://second.test/suffix"))}
        add("ws_first_match_creation_order_" + order, ["NS005"],
            [server_file()] + [entries[key] for key in order.split("_")])
    for label, key, session in [("number", 17, "17"), ("boolean", True, "True"), ("null", None, "None")]:
        add("ws_" + label + "_key", ["NS005", "NS008"], ordinary_files(browser(key)), session=session)
    for label, key in [("array", []), ("object", {}), ("false", False)]:
        add("ws_" + label + "_key_before_valid", ["NS005"], ordinary_files() + [
            text_file("nontext.json", browser(key)),
        ])
    missing_key = browser()
    del missing_key["chave"]
    add("ws_native_missing_only_key", ["NS008"], ordinary_files(missing_key))
    native_phase = {"session": NAME, "files": ordinary_files(), "evaluate": {}}
    electron_files = ordinary_files() + [text_file("electron.json", browser(
        url="http://electron.test/stale", targetId="GAP-TARGET"))]
    add("ws_electron_precedes_usable_native", ["NS006", "NR016"], None, phases=[native_phase, {
        "session": NAME, "files": electron_files,
        "evaluate": {"result": {"result": {"value": "http://electron.test/current"}}},
    }])
    for label, reply in [("error", {"error": {"code": -32000, "message": "evaluate refused by fixture"}}),
                         ("number", {"result": {"result": {"value": 17}}}),
                         ("empty", {"result": {"result": {"value": ""}}})]:
        add("ws_evaluate_" + label + "_keeps_sidecar_url", ["NS027"], electron_files, evaluate=reply)
    return cases


def write_files(nav, files, port):
    if nav.exists():
        shutil.rmtree(nav)
    if files is None:
        return
    nav.mkdir(parents=True)
    for entry in files:
        path = nav / entry["name"]
        if entry.get("directory"):
            path.mkdir()
        elif "bytes_hex" in entry:
            path.write_bytes(bytes.fromhex(entry["bytes_hex"]))
        else:
            path.write_text(entry["text"].replace('"$CONTROLLER_PORT"', str(port)), encoding="utf-8")


def snapshot(nav):
    if not nav.exists():
        return None
    return {path.name: {"directory": True} if path.is_dir() else {"bytes_hex": path.read_bytes().hex()}
            for path in nav.iterdir()}


def directory_order(nav):
    return [path.name for path in nav.glob("*.json")] if nav.is_dir() else None


class Isolation:
    def __init__(self, root):
        self.root = root
        self.ports = set()
        self.allowed_bind = None
        assert os.environ["PATH"] == str(root / "bin")
        assert not any((root / "bin").iterdir())
        assert Path.home() == root / "home"
        assert "app" not in sys.modules, "a fonte original foi importada antes da barreira"
        sys.addaudithook(self.audit)

    def refuse(self, event, details):
        record = {"event": event, "details": str(details).replace(str(self.root), "$ROOT")}
        with (self.root / "blocked-attempts.jsonl").open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, ensure_ascii=False) + "\n")
        raise PermissionError("a captura bloqueou processo/rede fora dos recursos próprios: " + event)

    def audit(self, event, args):
        if event == "subprocess.Popen":
            self.refuse(event, {"program": args[0], "argv": args[1]})
        if event in ("os.system", "os.posix_spawn", "os.posix_spawnp", "os.exec", "os.spawn", "os.fork", "os.forkpty"):
            self.refuse(event, args[0] if args else "")
        if event == "socket.bind":
            if self.allowed_bind != (id(args[0]), args[1]):
                self.refuse(event, args[1])
        elif event == "socket.connect":
            address = args[1]
            if not isinstance(address, tuple) or address[0] != "127.0.0.1" or address[1] not in self.ports:
                self.refuse(event, address)
        elif event == "socket.getaddrinfo":
            if args[0] != "127.0.0.1" or args[1] not in self.ports:
                self.refuse(event, args[:2])
        elif event in ("socket.sendto", "socket.sendmsg", "socket.gethostbyname", "socket.gethostbyaddr"):
            self.refuse(event, args[1] if len(args) > 1 else args[0])

    def listener(self, port=0):
        listener = socket.socket()
        address = ("127.0.0.1", port)
        self.allowed_bind = (id(listener), address)
        try:
            listener.bind(address)
        except BaseException:
            listener.close()
            raise
        finally:
            self.allowed_bind = None
        self.ports.add(listener.getsockname()[1])
        return listener


@asynccontextmanager
async def serve(app, isolation, port=0):
    import uvicorn
    listener = isolation.listener(port)
    address = listener.getsockname()
    started = asyncio.Event()
    server = None
    running = None
    ready = None
    failure = None
    try:
        class Server(uvicorn.Server):
            async def startup(self, sockets=None):
                await super().startup(sockets=sockets)
                started.set()

        server = Server(uvicorn.Config(app, log_level="critical", lifespan="off", ws="websockets-sansio",
                                       timeout_graceful_shutdown=3))
        running = asyncio.create_task(server.serve(sockets=[listener]))
        ready = asyncio.create_task(started.wait())
        async with asyncio.timeout(5):
            done, _ = await asyncio.wait((running, ready), return_when=asyncio.FIRST_COMPLETED)
            if running in done:
                await running
                raise RuntimeError("o servidor de ensaio encerrou antes de ficar disponível")
            await ready
        yield address[1]
    except BaseException as error:
        failure = error
        raise
    finally:
        cleanup_errors = []
        try:
            if ready is not None:
                ready.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await ready
        except BaseException as error:
            cleanup_errors.append(error)
        try:
            if server is not None:
                server.should_exit = True
            if running is not None:
                async with asyncio.timeout(8):
                    await running
        except BaseException as error:
            if error is not failure:
                cleanup_errors.append(error)
        try:
            listener.close()
        except BaseException as error:
            cleanup_errors.append(error)
        try:
            isolation.ports.remove(address[1])
        except BaseException as error:
            cleanup_errors.append(error)
        if cleanup_errors:
            errors = ([failure] if failure is not None else []) + cleanup_errors
            raise BaseExceptionGroup("falha do ensaio e da limpeza dos recursos próprios", errors) from None


async def capture(root):
    isolation = Isolation(root)
    sys.path.insert(0, str(root / "reference/backend"))
    from fastapi import FastAPI, Request, WebSocket
    from starlette.websockets import WebSocketDisconnect
    import websockets
    from app import auth, navshell, navsock
    from app.adapters.claude_headless import sessions as headless_sessions
    from app.adapters.codex import sessions as codex_sessions
    from generate_transport import CaptureLog, Desktop, TOKEN, receive_ready, barrier

    assert Path.home() == root / "home"
    api_path = root / "reference/backend/app/api.py"
    exists_node = next(node for node in ast.parse(api_path.read_text(encoding="utf-8")).body
                       if isinstance(node, ast.FunctionDef) and node.name == "_session_exists")
    scope = {"headless_sessions": headless_sessions, "codex_sessions": codex_sessions}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[exists_node], type_ignores=[])), str(api_path), "exec"), scope)
    measured = json.loads((HERE / "chromium.json").read_text(encoding="utf-8"))
    nav = root / "home/.hangar/nav"
    logs = CaptureLog(root)
    logger = logging.getLogger("hangar.navsock")
    logger.addHandler(logs)
    logger.setLevel(logging.DEBUG)

    class Peer(Desktop):
        def __init__(self, evaluate):
            super().__init__(copy.deepcopy(measured["frame"]), copy.deepcopy(measured["screenshot"]))
            self.evaluate = evaluate
            self.handshakes = []
            self.mode_requests = []
            self.electron_commands = []

        async def native(self, ws):
            self.handshakes.append({"transport": "native", "path": ws.url.path,
                                    "authorization": ws.headers.get("authorization"), "key": ws.query_params.get("chave")})
            await self.cdp(ws)

        async def electron(self, ws):
            self.handshakes.append({"transport": "electron", "path": ws.url.path,
                                    "authorization": ws.headers.get("authorization"), "key": ws.query_params.get("chave")})
            assert ws.headers.get("authorization") is None
            await ws.accept()
            acknowledged = False
            try:
                while True:
                    message = json.loads(await ws.receive_text())
                    self.electron_commands.append(message)
                    reply = self.evaluate if message["method"] == "Runtime.evaluate" else {"result": {}}
                    if message["method"] == "Page.captureScreenshot":
                        reply = {"result": self.screenshot}
                    await ws.send_text(json.dumps({"id": message["id"], **reply}))
                    if message["method"] == "Page.startScreencast":
                        await ws.send_text(json.dumps({"method": "Page.screencastFrame", "params": self.frame}))
                    elif message["method"] == "Page.screencastFrameAck" and not acknowledged:
                        acknowledged = True
                        await ws.send_text(json.dumps({"method": "Page.frameNavigated", "params": {"frame": {"url": "http://contract.test/ready"}}}))
            except WebSocketDisconnect:
                pass

        async def command(self, request):
            self.mode_requests.append({"method": request.method, "path": request.url.path,
                                       "authorization": request.headers.get("authorization"),
                                       "content_type": request.headers.get("content-type"), "body": await request.json()})
            return await super().command(request)

    current = None
    app = FastAPI()

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def remote(ws: WebSocket, name: str):
        peer = current
        finished = asyncio.Event()
        peer.finished.append(finished)
        try:
            await navsock.nav_ws(ws, name, scope["_session_exists"])
        finally:
            finished.set()

    @app.websocket("/cdp")
    async def native(ws: WebSocket):
        await current.native(ws)

    @app.websocket("/devtools/page/GAP-TARGET")
    async def electron(ws: WebSocket):
        await current.electron(ws)

    @app.post("/cmd")
    async def command(request: Request):
        return await current.command(request)

    def seed(name):
        path = root / "home/.hangar/claude-headless" / (name + ".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"name": name, "key": name + "-key", "session_id": "gap-transcript", "cwd": str(root / "workspace")}), encoding="utf-8")
        assert scope["_session_exists"](name)

    def uri(port, name=NAME):
        return f"ws://127.0.0.1:{port}/api/sessions/{quote(name, safe='')}/nav-remoto?token={TOKEN}"

    async def open_ready(port, name=NAME):
        ws = await websockets.connect(uri(port, name), max_size=32 * 1024 * 1024)
        events = []
        await receive_ready(ws, events)
        return ws, events

    async def finish(ws, peer):
        await ws.close()
        await peer.finish(0)

    results = selection_cases()
    try:
        async with serve(app, isolation) as port, serve(app, isolation, 9223):
            for case in results:
                case["expected"] = {"phases": []}
                for phase in case["inputs"]["phases"]:
                    seed(phase["session"])
                    current = Peer(phase["evaluate"])
                    write_files(nav, phase["files"], port)
                    phase["directory_order"] = directory_order(nav)
                    if "before_valid" in case["name"] and "unreadable" in case["name"]:
                        assert all(phase["directory_order"].index(name) < phase["directory_order"].index("browser.json")
                                   for name in ("array.json", "null.json", "scalar.json", "broken.json", "bytes.json", "directory.json")), "a enumeração não conserva os arquivos ruins antes do válido"
                    if case["name"] == "ws_underscore_match_is_ignored":
                        assert phase["directory_order"].index("_matching.json") < phase["directory_order"].index("browser.json"), "o auxiliar precisa anteceder o navegador válido"
                    before = snapshot(nav)
                    logs.rows.clear()
                    auth.reset_backoff()
                    try:
                        ws, events = await open_ready(port, phase["session"])
                    except websockets.exceptions.InvalidStatus as error:
                        observed = {"status": error.response.status_code, "body_hex": bytes(error.response.body).hex(), "events": []}
                        await current.finish(0)
                    else:
                        await finish(ws, current)
                        observed = {"status": ws.response.status_code, "body_hex": None, "events": events}
                    observed.update(handshakes=current.handshakes, requests=current.mode_requests,
                                    native_cdp=current.commands, electron_cdp=current.electron_commands,
                                    warnings=copy.deepcopy(logs.rows), files_unchanged=snapshot(nav) == before)
                    assert observed["files_unchanged"]
                    case["expected"]["phases"].append(observed)

            for name, ids, stages in [
                ("cmd_warning_and_non_objects_before_valid", ["NB005"], [warning_files()]),
                ("cmd_reloads_changed_removed_and_exact_key", ["NR013"], [ordinary_files(browser("updated::" + NAME)), ordinary_files({"pid": 123}), ordinary_files(browser(NAME))]),
            ]:
                seed(NAME)
                current = Peer({})
                initial_files = ordinary_files()
                write_files(nav, initial_files, port)
                initial_order = directory_order(nav)
                ws, events = await open_ready(port)
                outcomes = []
                inputs = []
                logs.rows.clear()
                for files in stages:
                    write_files(nav, files, port)
                    order = directory_order(nav)
                    if name.startswith("cmd_warning"):
                        assert all(order.index(item) < order.index("browser.json") for item in (
                            "array.json", "null.json", "scalar.json", "broken.json", "bytes.json", "directory.json")), "arquivos ruins precisam anteceder o navegador válido"
                    before = snapshot(nav)
                    await ws.send(json.dumps({"t": "layout", "modo": "mobile"}))
                    from generate_transport import receive_until
                    outcome = await receive_until(ws, [], lambda item: item.get("t") in ("layout", "erro"))
                    await barrier(ws, [])
                    assert snapshot(nav) == before
                    outcomes.append(outcome)
                    inputs.append({"files": files, "directory_order": order})
                await finish(ws, current)
                results.append({"name": name, "ids": ids, "kind": "commands", "inputs": {"initial_files": initial_files, "initial_directory_order": initial_order, "stages": inputs},
                                "expected": {"outcomes": outcomes, "requests": current.mode_requests, "warnings": copy.deepcopy(logs.rows)}})

            seed(NAME)
            current = Peer({})
            initial_files = ordinary_files()
            write_files(nav, initial_files, port)
            initial_order = directory_order(nav)
            ws, events = await open_ready(port)
            requests = []
            accepted_connections = 0
            ended = asyncio.Event()
            logs.rows.clear()

            async def held_body(reader, writer):
                nonlocal accepted_connections
                accepted_connections += 1
                try:
                    head = (await reader.readuntil(b"\r\n\r\n")).decode("ascii")
                    lines = head.split("\r\n")
                    fields = {key.lower(): value.strip() for line in lines[1:] if ":" in line for key, value in [line.split(":", 1)]}
                    raw = await reader.readexactly(int(fields["content-length"]))
                    requests.append({"method": lines[0].split()[0], "path": lines[0].split()[1],
                                     "authorization": fields.get("authorization"), "content_type": fields.get("content-type"), "body": json.loads(raw)})
                    writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nlayout:")
                    await writer.drain()
                    await reader.read()
                finally:
                    writer.close()
                    with contextlib.suppress(ConnectionError):
                        await writer.wait_closed()
                    ended.set()

            slow_listener = isolation.listener()
            slow = await asyncio.start_server(held_body, sock=slow_listener)
            slow_port = slow.sockets[0].getsockname()[1]
            try:
                write_files(nav, ordinary_files(), slow_port)
                before = snapshot(nav)
                started = time.monotonic()
                await ws.send(json.dumps({"t": "layout", "modo": "mobile"}))
                async with asyncio.timeout(65):
                    while True:
                        event = json.loads(await ws.recv())
                        if event.get("t") in ("layout", "erro"):
                            break
                    elapsed = time.monotonic() - started
                    await ended.wait()
                await barrier(ws, [])
                assert snapshot(nav) == before
                await finish(ws, current)
                results.append({"name": "cmd_default_read_timeout_45_seconds", "ids": ["NB010", "NB013", "NR015"], "kind": "timeout",
                                "inputs": {"initial_files": initial_files, "initial_directory_order": initial_order, "files": ordinary_files(), "directory_order": directory_order(nav), "deadline_seconds": navshell.TETO_S},
                                "expected": {"outcomes": [event], "requests": requests, "accepted_connections": accepted_connections, "elapsed_seconds": elapsed, "warnings": copy.deepcopy(logs.rows)}})
            finally:
                slow.close()
                await slow.wait_closed()
                isolation.ports.remove(slow_port)
    finally:
        logger.removeHandler(logs)
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    parser.add_argument("--capture", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.capture:
        result = asyncio.run(capture(args.capture))
        print(json.dumps(result, ensure_ascii=False, allow_nan=False))
        return
    if args.output is None:
        parser.error("--output é obrigatório; nenhuma referência é substituída implicitamente")
    with tempfile.TemporaryDirectory(prefix="hangar-url-gaps-", dir="/tmp") as temporary:
        root = Path(temporary)
        for name in ("home", "workspace", "reference", "tmp", "bin"):
            (root / name).mkdir()
        archive = subprocess.run(["git", "archive", "--format=tar", SOURCE, "backend/app"], cwd=REPO,
                                 check=True, capture_output=True, timeout=30)
        with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as bundle:
            bundle.extractall(root / "reference", filter="data")
        env = clean_environment(root)
        env.update(CP_AUTH_TOKEN="remote-browser-contract-owner", TMPDIR=str(root / "tmp"), PATH=str(root / "bin"))
        child = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--capture", str(root)],
                               cwd=root / "workspace", env=env, capture_output=True, timeout=240)
        blocked_path = root / "blocked-attempts.jsonl"
        blocked_attempts = [json.loads(line) for line in blocked_path.read_text(encoding="utf-8").splitlines()] if blocked_path.exists() else []
        if child.returncode:
            sys.stderr.write(child.stderr.decode("utf-8", errors="replace"))
            sys.stderr.write(json.dumps({"blocked_attempts": blocked_attempts}, ensure_ascii=False) + "\n")
            child.check_returncode()
        payload = {"source_commit": SOURCE,
                   "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_url_gaps.py --output backend/tests/fixtures/contract/remote-browser/url-gaps.json",
                   "source_sha256": {name: hashlib.sha256((root / "reference/backend/app" / name).read_bytes()).hexdigest() for name in ("api.py", "navsock.py", "navshell.py")},
                   "measured_sha256": hashlib.sha256((HERE / "chromium.json").read_bytes()).hexdigest(),
                   "platform": sys.platform, "python_version": sys.version.split()[0],
                   "isolation": {"path": "$ROOT/bin (vazio)", "processes": "todos recusados no filho antes dos imports originais", "network": "somente sockets/portas explicitamente reservados pelo ensaio", "blocked_attempts": blocked_attempts},
                   "normalization": {"$ROOT": "prefixo do temporário em logs, como nos corpora existentes", "$CONTROLLER_PORT": "porta própria injetada apenas nos arquivos de entrada"},
                   "limits": ["HOME/config/cwd/TMPDIR isolados; fonte integral exportada da base e _session_exists original por AST com leitores reais de sidecar sem terminal.",
                              "Porta Electron 9223 só após bind exclusivo próprio; uma porta ocupada impede a captura, sem reutilizar navegador existente.",
                              "Ordem de diretório registrada sem ordenar. A rodada Rust precisa reproduzir a enumeração; condição diferente é falha de ambiente.",
                              "Python usa prazo de leitura real e elapsed_seconds cru; Rust controla somente seu relógio de teste, sem equivalência de desempenho.",
                              "Warning integral conservado e comparado no Rust após somente $ROOT; texto diferente de parser/SO precisa ser classificado, sem normalização adicional.",
                              "Sem desktop, sessão, conta, tmux, instalação ou serviço reais; JPEG vem da captura Chromium já congelada."],
                   "cases": json.loads(child.stdout)}
    args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
