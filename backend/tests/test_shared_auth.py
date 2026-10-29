"""Dois processos reais: roteador Rust e bordas ASGI Python usando a DLL, em HOME descartável."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

import pytest
from fastapi import Request, WebSocket

ROOT = Path(__file__).resolve().parents[2]
OWNER = "shared-auth-integration-owner-fake"


def _binary():
    explicit = os.environ.get("HANGAR_SHARED_AUTH_TEST_BIN")
    names = [Path(explicit)] if explicit else [ROOT / "crates/target" / profile / ("hangar-server.exe" if os.name == "nt" else "hangar-server")
                                             for profile in ("debug", "release")]
    for path in names:
        if path.is_file():
            return path.resolve()
    pytest.fail("compile hangar-server e hangar-auth e fixe o par antes deste ensaio")


def _environment(tmp_path):
    env = {key: os.environ[key] for key in ("SYSTEMROOT", "WINDIR", "COMSPEC", "TMPDIR") if key in os.environ}
    paths = [str(ROOT / "backend")]
    paths.extend(path for path in os.environ.get("PYTHONPATH", "").split(os.pathsep) if path)
    env.update(PYTHONPATH=os.pathsep.join(paths), PYTHONIOENCODING="utf-8")
    for key in ("HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
        env[key] = str(tmp_path)
    env.update(CP_AUTH_TOKEN=OWNER, CLAUDE_CONFIG_DIR=str(tmp_path / ".claude"), CODEX_HOME=str(tmp_path / ".codex"))
    return env


def test_root_browser_python_me_and_websocket_share_one_parent_authority(tmp_path):
    binary = _binary()
    result = subprocess.run([sys.executable, __file__, "--worker", str(binary)], cwd=tmp_path,
                            env=_environment(tmp_path), capture_output=True, text=True, timeout=90)
    assert result.returncode == 0, result.stdout + result.stderr
    report = json.loads(result.stdout.splitlines()[-1])
    assert report["root_and_python_real"] and report["child_restart_kept_eight"]
    assert report["concurrent_statuses"] == [401, 429]
    assert report["body_calls"] == 1 and report["sse_calls"] == 1 and report["upgrade_once"]
    assert report["receipt_kept_interleaved_failure"] and report["public_receipt_was_not_reused"]
    assert report["login_guest_did_not_count"]
    assert report["empty_query_kept_one_admission"]
    assert report["corrupt_store_statuses"] == [503, 503, 503, 503]


def test_loaded_library_stays_immutable_when_a_new_pair_is_published(tmp_path):
    result = subprocess.run([sys.executable, __file__, "--pinning-worker", str(_binary())], cwd=tmp_path,
                            env=_environment(tmp_path), capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(result.stdout.splitlines()[-1]) == {"loaded_library_pinned": True, "new_parent_epoch_is_empty": True}


def _pinning_worker(binary):
    import importlib.util
    from app import rust_bins, shared_auth
    with tempfile.TemporaryDirectory(prefix="shared-auth-pinning-") as temporary:
        root = Path(temporary)
        source, _ = rust_bins.auth_pair(binary)
        library = root / source.name
        original = source.read_bytes()
        library.write_bytes(original)
        server = root / binary.name
        server.write_bytes(b"server-fixture")
        spec = importlib.util.spec_from_file_location("pin_auth", ROOT / "scripts/pin-auth-pair.py")
        publisher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(publisher)
        publisher.publish(server, library)
        context = shared_auth.Context(server, OWNER, 12345)
        try:
            old_library, _ = rust_bins.auth_pair(server)
            for _ in range(8):
                assert context.evaluate(shared_auth.HTTP, "198.51.100.1", "wrong").status == shared_auth.UNAUTHORIZED
            library.write_bytes(original + b"\0new-build-fixture")
            server.write_bytes(b"new-server-fixture")
            publisher.publish(server, library)
            assert old_library.read_bytes() == original
            assert rust_bins.auth_pair(server)[0] != old_library
            assert context.evaluate(shared_auth.HTTP, "198.51.100.1", OWNER).status == shared_auth.BLOCKED
            try:
                context.child_env(server)
            except shared_auth.Unavailable as error:
                assert error.code == "auth_pair_changed_restart_parent"
            else:
                raise AssertionError("o filho novo não pode trocar a biblioteca do pai")
            # Outra vida é outro interpretador, sem receber caminho/época/chave do anterior.
            next_root = root / "next-parent"
            next_root.mkdir()
            result = subprocess.run([sys.executable, __file__, "--fresh-parent", str(binary)], cwd=next_root,
                                    env=_environment(next_root), capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, result.stdout + result.stderr
            next_parent = json.loads(result.stdout.splitlines()[-1])
            assert next_parent["parent_epoch"] != context.config["parent_epoch"] and next_parent["empty"]
            assert context.evaluate(shared_auth.HTTP, "198.51.100.1", OWNER).status == shared_auth.BLOCKED
            return {"loaded_library_pinned": True, "new_parent_epoch_is_empty": True}
        finally:
            context.close()


async def _worker(binary):
    import httpx2 as httpx
    import uvicorn
    from fastapi import Depends, FastAPI, Request, WebSocket
    from fastapi.responses import StreamingResponse
    from websockets.asyncio.client import connect
    from websockets.exceptions import InvalidStatus

    with tempfile.TemporaryDirectory(prefix="shared-auth-real-") as temporary:
        home = Path(temporary)
        for name in ("HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            os.environ[name] = str(home)
        os.environ["CLAUDE_CONFIG_DIR"] = str(home / ".claude")
        os.environ["CODEX_HOME"] = str(home / ".codex")
        from app import auth, shared_auth, navsock, voicesock, termsock, guest_users
        from app.guest_user_gate import GuestUserGate
        auth.settings.auth_token = OWNER
        (home / ".claude").mkdir()
        guest_users._path_override = home / ".claude/guests.json"
        guest_users._reset()
        _, guest_token = guest_users.create("isolated-guest", str(home), False, True)
        app = FastAPI()
        app.add_middleware(GuestUserGate)
        admitted, release, sse_release = asyncio.Event(), asyncio.Event(), asyncio.Event()
        captured = {}
        body_calls = sse_calls = upgrade_calls = 0

        @app.get("/api/me", dependencies=[Depends(auth.require_auth)])
        async def me():
            return {"owner": True}

        @app.get("/api/update-channel")
        async def delayed(request: Request):
            captured["receipt"] = request.headers.get(shared_auth.RECEIPT)
            captured["empty_query"] = request.headers.get(shared_auth.EMPTY_QUERY)
            if request.headers.get("x-test-delay"):
                admitted.set()
                await release.wait()
            await auth.require_auth(request)
            return {"owner": True}

        @app.post("/api/auth-body", dependencies=[Depends(auth.require_auth)])
        async def body(request: Request):
            nonlocal body_calls
            body_calls += 1
            data = await request.body()
            return {"bytes": len(data), "digest": __import__("hashlib").sha256(data).hexdigest()}

        @app.get("/api/auth-sse", dependencies=[Depends(auth.require_auth)])
        async def sse():
            nonlocal sse_calls
            sse_calls += 1
            async def chunks():
                yield b"event: message\ndata: first\n\n"
                await sse_release.wait()
                yield b"event: message\ndata: last\n\n"
            return StreamingResponse(chunks(), media_type="text/event-stream")

        @app.websocket("/api/auth-ws/{profile}")
        async def ws(websocket: WebSocket, profile: str):
            nonlocal upgrade_calls
            upgrade_calls += 1
            if profile == "browser":
                allowed = await navsock._autorizado(websocket)
            elif profile == "voice":
                allowed = await voicesock._owner_only(websocket)
            else:
                # Só o verificador de existência externo é substituído; nenhum PTY nasce.
                termsock.tmux.has_session = lambda _: True
                allowed = await termsock._porta_de_entrada(websocket, "isolated-session") is not None
            if allowed:
                await websocket.accept()
                await websocket.close(code=1000)

        def listener():
            sock = socket.socket()
            sock.bind(("127.0.0.1", 0))
            sock.listen()
            sock.setblocking(False)
            return sock

        upstream, public_python = listener(), listener()
        context = await asyncio.to_thread(shared_auth.activate, binary, OWNER, upstream.getsockname()[1])
        native_calls = [0]
        evaluate = context.evaluate
        def counted_evaluate(*args, **kwargs):
            native_calls[0] += 1
            return evaluate(*args, **kwargs)
        context.evaluate = counted_evaluate
        assert auth._fails == {}
        config = dict(lifespan="off", log_level="critical", proxy_headers=True, forwarded_allow_ips="127.0.0.1")
        py = uvicorn.Server(uvicorn.Config(app, **config))
        py_public = uvicorn.Server(uvicorn.Config(app, **config))
        py_task = asyncio.create_task(py.serve(sockets=[upstream]))
        public_task = asyncio.create_task(py_public.serve(sockets=[public_python]))
        while not (py.started and py_public.started):
            await asyncio.sleep(.01)
        root_socket = listener()
        root_port = root_socket.getsockname()[1]
        root_socket.close()
        child_env = {key: os.environ[key] for key in ("SYSTEMROOT", "WINDIR", "COMSPEC") if key in os.environ}
        child_env.update(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(home / "config"),
                         XDG_DATA_HOME=str(home / "data"), XDG_CACHE_HOME=str(home / "cache"), PATH=str(home / "empty-path"),
                         HANGAR_SERVER_LISTEN=f"127.0.0.1:{root_port}",
                         HANGAR_SERVER_UPSTREAM=f"127.0.0.1:{upstream.getsockname()[1]}",
                         HANGAR_INTERNAL_SECRET="shared-auth-private-secret-fake", CP_AUTH_TOKEN=OWNER,
                         CP_FORWARDED_ALLOW_IPS="127.0.0.1", CP_RUST_NO_ORPHAN_SWEEP="1", **context.child_env(binary))
        child = None

        async def start():
            child_env["HANGAR_RUNTIME_INSTANCE"] = __import__("secrets").token_hex(16)
            process = await asyncio.create_subprocess_exec(str(binary), stdin=asyncio.subprocess.PIPE,
                        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL, env=child_env, cwd=home)
            try:
                ready = json.loads(await asyncio.wait_for(process.stdout.readline(), 15))
                assert ready["type"] == "runtime_ready"
                async with httpx.AsyncClient(trust_env=False, timeout=1) as probe:
                    deadline = time.monotonic() + 10
                    while True:
                        try:
                            response = await probe.get(f"http://127.0.0.1:{root_port}/__hangar_server/health")
                            if response.status_code == 200:
                                assert response.json()["pid"] == process.pid
                                assert response.json()["guest_port_bound"] is False
                                break
                        except httpx.TransportError:
                            pass
                        if time.monotonic() >= deadline:
                            raise RuntimeError("servidor de ensaio não abriu a porta pública")
                        await asyncio.sleep(.01)
                return process
            except BaseException:
                process.kill()
                await process.wait()
                raise

        async def stop(process):
            if process.returncode is None:
                process.stdin.close()
                try:
                    await asyncio.wait_for(process.wait(), 8)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()

        root = f"http://127.0.0.1:{root_port}"
        direct = f"http://127.0.0.1:{upstream.getsockname()[1]}"
        public = f"http://127.0.0.1:{public_python.getsockname()[1]}"
        browser = "/api/sessions/isolated-session/navegador"

        def hits(ip):
            value = json.loads(Path(context.config["state_path"]).read_bytes())
            return next((times for origin, times in value["origins"] if origin == ip), [])

        async with httpx.AsyncClient(trust_env=False, timeout=10) as client:
            async def request(base, path, token="wrong", ip="198.51.100.10", **kwargs):
                headers = {"x-forwarded-for": ip, **kwargs.pop("headers", {})}
                query = {} if token is None else {"token": token}
                return await client.request(kwargs.pop("method", "GET"), base + path, params=query, headers=headers, **kwargs)

            async def ws_status(base, path, token, ip):
                from urllib.parse import urlencode
                query = "?" + urlencode({"token": token}) if token is not None else ""
                try:
                    async with connect(base.replace("http:", "ws:") + path + query,
                                       additional_headers={"x-forwarded-for": ip}, proxy=None) as socket:
                        await socket.wait_closed()
                        return socket.close_code
                except InvalidStatus as error:
                    return error.response.status_code

            async def raw_get(base, target, headers):
                # http.client conserva o '?' final que alguns clientes de alto nível normalizam.
                def send():
                    from http.client import HTTPConnection
                    from urllib.parse import urlsplit
                    connection = HTTPConnection("127.0.0.1", urlsplit(base).port, timeout=10)
                    try:
                        connection.request("GET", target, headers=headers)
                        response = connection.getresponse()
                        response.read()
                        return response.status, dict(response.getheaders())
                    finally:
                        connection.close()
                return await asyncio.to_thread(send)

            try:
                child = await start()
                # As falhas atravessam as duas implementações e duas portas de verdade.
                for i in range(7):
                    response = await request(root, browser if i % 2 else "/api/me")
                    assert response.status_code == 401
                assert await ws_status(direct, "/api/auth-ws/browser", "wrong", "198.51.100.10") == 403
                assert len(hits("198.51.100.10")) == 8
                for path in (browser, "/api/me"):
                    assert (await request(root, path, OWNER)).status_code == 429

                ip = "198.51.100.11"
                for _ in range(7):
                    assert (await request(direct, "/api/me", ip=ip)).status_code == 401
                statuses = sorted(response.status_code for response in await asyncio.gather(
                    request(root, browser, ip=ip), request(direct, "/api/me", ip=ip)))
                assert statuses == [401, 429] and len(hits(ip)) == 8

                ip = "198.51.100.12"
                for _ in range(7):
                    assert (await request(root, browser, ip=ip)).status_code == 401
                assert await ws_status(direct, "/api/auth-ws/browser", OWNER, ip) == 1000
                assert await ws_status(direct, "/api/auth-ws/voice", OWNER, ip) == 1000
                assert await ws_status(direct, "/api/auth-ws/terminal", OWNER, ip) == 1000
                before_upgrade = upgrade_calls
                assert await ws_status(root, "/api/auth-ws/browser", OWNER, ip) == 1000
                assert upgrade_calls == before_upgrade + 1
                assert len(hits(ip)) == 7
                assert (await request(direct, "/api/me", OWNER, ip)).status_code == 200 and not hits(ip)
                for _ in range(7):
                    assert (await request(direct, "/api/me", ip=ip)).status_code == 401
                assert (await request(root, browser, OWNER, ip)).status_code == 200 and not hits(ip)

                ip = "198.51.100.15"
                for _ in range(7):
                    assert (await request(root, browser, ip=ip)).status_code == 401
                assert (await request(root, browser, guest_token, ip)).status_code == 403
                assert await ws_status(root, "/api/sessions/isolated-session/nav-remoto", guest_token, ip) == 403
                assert (await request(root, "/api/me", guest_token, ip)).status_code == 200
                assert len(hits(ip)) == 7
                assert (await request(root, browser, ip=ip)).status_code == 401
                assert (await request(root, browser, guest_token, ip)).status_code == 403
                assert (await request(direct, "/api/me", guest_token, ip)).status_code == 200
                assert len(hits(ip)) == 8

                ip = "192.0.2.1"
                for _ in range(9):
                    assert (await request(root, browser, None, ip)).status_code == 401
                assert not hits(ip)
                for _ in range(7):
                    assert (await request(direct, "/api/me", ip=ip)).status_code == 401
                assert (await request(root, browser, OWNER, ip)).status_code == 200 and len(hits(ip)) == 7
                assert await ws_status(direct, "/api/auth-ws/voice", None, ip) == 403 and len(hits(ip)) == 7
                assert await ws_status(direct, "/api/auth-ws/browser", None, ip) == 403 and len(hits(ip)) == 8

                # Um gate do Rust admite; uma falha intercalada não pode sumir no Python.
                ip = "198.51.100.13"
                delayed_call = asyncio.create_task(request(root, "/api/update-channel", OWNER, ip, headers={"x-test-delay": "1"}))
                await asyncio.wait_for(admitted.wait(), 5)
                assert captured["receipt"]
                receipt = captured["receipt"]
                assert (await request(root, browser, ip=ip)).status_code == 401
                release.set()
                assert (await delayed_call).status_code == 200 and len(hits(ip)) == 1
                assert (await request(public, "/api/update-channel", OWNER, ip, headers={shared_auth.RECEIPT: receipt})).status_code == 200
                assert not hits(ip), "porta pública deve executar a transação, não reutilizar o recibo"

                receipts = []
                for index, target in enumerate(("/api/update-channel", "/api/update-channel?", "/api/update-channel?view=1")):
                    ip = f"198.51.100.{30 + index}"
                    admitted.clear()
                    release.clear()
                    bearer = {"authorization": "Bearer " + OWNER, "x-forwarded-for": ip}
                    delayed_call = asyncio.create_task(raw_get(root, target, {**bearer, "x-test-delay": "1",
                        shared_auth.RECEIPT: "forged", shared_auth.EMPTY_QUERY: "truncated"}))
                    await asyncio.wait_for(admitted.wait(), 5)
                    receipt, empty_query = captured["receipt"], captured["empty_query"]
                    assert receipt and OWNER not in receipt
                    assert empty_query == ("1" if target.endswith("?") else None)
                    assert (await request(root, browser, ip=ip)).status_code == 401
                    before_native = native_calls[0]
                    release.set()
                    status, response_headers = await delayed_call
                    assert status == 200 and len(hits(ip)) == 1 and native_calls[0] == before_native
                    assert shared_auth.RECEIPT not in {key.lower() for key in response_headers}
                    assert shared_auth.EMPTY_QUERY not in {key.lower() for key in response_headers}
                    receipts.append((target, bearer, receipt, empty_query))
                for target, bearer, receipt, marker in receipts:
                    ip = bearer["x-forwarded-for"]
                    before_native = native_calls[0]
                    if marker is None and "?" not in target:
                        status, _ = await raw_get(direct, target, {**bearer, shared_auth.RECEIPT: receipt, shared_auth.EMPTY_QUERY: "1"})
                    else:
                        status, _ = await raw_get(direct, target, {**bearer, shared_auth.RECEIPT: receipt[:-1],
                            **({shared_auth.EMPTY_QUERY: marker} if marker else {})})
                    assert status == 503 and len(hits(ip)) == 1 and native_calls[0] == before_native
                target, bearer, receipt, marker = receipts[1]
                for malformed in ({shared_auth.RECEIPT: receipt},
                                  {shared_auth.RECEIPT: receipt, shared_auth.EMPTY_QUERY: "truncated"},
                                  {shared_auth.RECEIPT: "", shared_auth.EMPTY_QUERY: "1"}):
                    before_native = native_calls[0]
                    status, _ = await raw_get(direct, target, {**bearer, **malformed})
                    assert status == 503 and native_calls[0] == before_native and len(hits(bearer["x-forwarded-for"])) == 1
                nonempty_target, nonempty_bearer, nonempty_receipt, _ = receipts[2]
                before_native = native_calls[0]
                status, _ = await raw_get(direct, nonempty_target, {**nonempty_bearer,
                    shared_auth.RECEIPT: nonempty_receipt, shared_auth.EMPTY_QUERY: "1"})
                assert status == 503 and native_calls[0] == before_native and len(hits(nonempty_bearer["x-forwarded-for"])) == 1
                before_native = native_calls[0]
                status, _ = await raw_get(public, target, {**bearer, shared_auth.RECEIPT: receipt, shared_auth.EMPTY_QUERY: "1"})
                assert status == 200 and native_calls[0] == before_native + 1 and not hits(bearer["x-forwarded-for"])

                payload = b"body-once-" * 131072
                async def upload():
                    for start in range(0, len(payload), 32000):
                        yield payload[start:start + 32000]
                        await asyncio.sleep(0)
                response = await request(root, "/api/auth-body", OWNER, "198.51.100.14", method="POST", content=upload())
                assert response.status_code == 200 and response.json()["bytes"] == len(payload)
                assert response.json()["digest"] == __import__("hashlib").sha256(payload).hexdigest()
                async with client.stream("GET", root + "/api/auth-sse", params={"token": OWNER},
                                         headers={"x-forwarded-for": "198.51.100.14"}) as response:
                    chunks = response.aiter_raw()
                    first = await asyncio.wait_for(anext(chunks), 3)
                    assert first == b"event: message\ndata: first\n\n"
                    sse_release.set()
                    assert b"".join([chunk async for chunk in chunks]) == b"event: message\ndata: last\n\n"
                assert body_calls == 1 and sse_calls == 1

                # Mesmo pai e mesma tabela durante queda, fallback e nova vida do filho.
                restart_ip = "198.51.100.19"
                for _ in range(8):
                    assert (await request(root, browser, ip=restart_ip)).status_code == 401
                await stop(child)
                assert (await request(direct, "/api/me", OWNER, restart_ip)).status_code == 429
                child = await start()
                assert (await request(root, browser, OWNER, restart_ip)).status_code == 429
                assert len(hits(restart_ip)) == 8

                # Falha do arquivo após ativação não abre uma autoridade de reserva.
                Path(context.config["state_path"]).write_bytes(b"invalid")
                broken = [(await request(root, browser, OWNER, "198.51.100.20")).status_code,
                          (await request(root, "/api/me", OWNER, "198.51.100.20")).status_code,
                          await ws_status(direct, "/api/auth-ws/browser", OWNER, "198.51.100.20"),
                          await ws_status(root, "/api/sessions/isolated-session/nav-remoto", OWNER, "198.51.100.20")]
                assert broken == [503, 503, 503, 503] and auth._fails == {}
                return {"root_and_python_real": True, "child_restart_kept_eight": True,
                        "concurrent_statuses": statuses, "body_calls": body_calls, "sse_calls": sse_calls, "upgrade_once": True,
                        "receipt_kept_interleaved_failure": True, "public_receipt_was_not_reused": True,
                        "login_guest_did_not_count": True,
                        "empty_query_kept_one_admission": True,
                        "corrupt_store_statuses": broken}
            finally:
                sse_release.set()
                if child is not None:
                    await stop(child)
                py.should_exit = py_public.should_exit = True
                await asyncio.gather(py_task, public_task)
                upstream.close()
                public_python.close()


if __name__ == "__main__" and sys.argv[1:2] == ["--worker"]:
    sys.path.insert(0, str(ROOT / "backend"))
    print(json.dumps(asyncio.run(_worker(Path(sys.argv[2])))))
elif __name__ == "__main__" and sys.argv[1:2] == ["--pinning-worker"]:
    sys.path.insert(0, str(ROOT / "backend"))
    print(json.dumps(_pinning_worker(Path(sys.argv[2]))))
elif __name__ == "__main__" and sys.argv[1:2] == ["--fresh-parent"]:
    sys.path.insert(0, str(ROOT / "backend"))
    from app import shared_auth
    context = shared_auth.activate(Path(sys.argv[2]), OWNER, 12345)
    try:
        print(json.dumps({"parent_epoch": context.config["parent_epoch"],
                          "empty": context.evaluate(shared_auth.HTTP, "198.51.100.1", OWNER).status == shared_auth.OWNER}))
    finally:
        shared_auth.shutdown()
