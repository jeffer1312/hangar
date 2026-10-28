"""Supervisor real aguarda a devolução de um viewer Root com controlador de ensaio lento."""
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
from test_shared_auth import ROOT, OWNER, _binary, _environment


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="fonte tmux de ensaio usa script POSIX, sem tmux real")
def test_supervisor_protects_six_second_browser_restore(tmp_path):
    result = subprocess.run([sys.executable, __file__, "--worker", str(_binary())], cwd=tmp_path,
                            env=_environment(tmp_path), capture_output=True, text=True, timeout=45)
    assert result.returncode == 0, result.stdout + result.stderr
    report = json.loads(result.stdout.splitlines()[-1])
    assert report["at_five"] == {"alive": True, "layout": "900x700", "commands": 3}
    assert report["layout"] == "mobile" and report["commands"] == 4
    assert report["close"] == {"code": 1012, "reason": ""}
    assert report["exit_code"] == 0 and report["kills"] == 0
    assert report["elapsed_seconds"] >= 6 and report["deadline_seconds"] == 120


async def _worker(binary):
    import httpx
    import uvicorn
    from fastapi import FastAPI, WebSocketDisconnect
    from fastapi.responses import PlainTextResponse
    from websockets.asyncio.client import connect
    from app import rust_server, runtime_process, shared_auth

    with tempfile.TemporaryDirectory(prefix="protected-browser-stop-") as temporary:
        home = Path(temporary)
        fake_bin = home / "bin"
        fake_bin.mkdir()
        tmux = fake_bin / "tmux"
        tmux.write_text("#!/bin/sh\nexit 0\n")
        tmux.chmod(0o700)
        claude = home / ".claude"
        claude.mkdir()
        nav = home / ".hangar/nav"
        nav.mkdir(parents=True)
        sessions = home / ".hangar/claude-headless"
        sessions.mkdir()
        (sessions / "nav-contract.json").write_text(json.dumps({"name": "nav-contract", "key": "nav-contract-key",
            "session_id": "headless-transcript", "cwd": str(home)}))
        measured = json.loads((ROOT / "backend/tests/fixtures/contract/remote-browser/chromium.json").read_bytes())
        reference = json.loads((ROOT / "backend/tests/fixtures/contract/remote-browser/shutdown-reference.json").read_bytes())["observed"]
        requests = []
        layout = "mobile"
        stopping = False
        cleanup_received = asyncio.Event()
        desktop = FastAPI()

        @desktop.post("/cmd")
        async def command(request: Request):
            nonlocal layout
            body = await request.json()
            requests.append({"authorization": request.headers.get("authorization"), "body": body, "receiver_pid": 123})
            if stopping and not body["args"]:
                cleanup_received.set()
                await asyncio.sleep(6)
            if body["args"]:
                layout = "x".join(body["args"])
            return PlainTextResponse("layout: " + layout)

        @desktop.websocket("/cdp")
        async def cdp(ws: WebSocket):
            await ws.accept()
            try:
                while True:
                    message = json.loads(await ws.receive_text())
                    await ws.send_json({"id": message["id"], "result": {}})
                    if message["method"] == "Page.startScreencast":
                        await ws.send_json({"method": "Page.screencastFrame", "params": measured["frame"]})
            except WebSocketDisconnect:
                pass

        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.setblocking(False)
        desktop_port = listener.getsockname()[1]
        desktop_server = uvicorn.Server(uvicorn.Config(desktop, lifespan="off", log_level="critical"))
        desktop_task = asyncio.create_task(desktop_server.serve(sockets=[listener]))
        while not desktop_server.started:
            await asyncio.sleep(.01)
        (nav / "browser.json").write_text(json.dumps({"chave": "contract::nav-contract", "pid": 123,
            "targetId": None, "url": "http://contract.test/original"}))
        (nav / "_srv.json").write_text(json.dumps({"porta": desktop_port, "pid": 123, "token": "remote-browser-contract-desktop"}))
        reservation = socket.socket()
        reservation.bind(("127.0.0.1", 0))
        public_port = reservation.getsockname()[1]
        reservation.close()
        context = shared_auth.activate(binary, OWNER, desktop_port)
        dirs = {"home": str(home), "claude": str(claude), "codex_home": str(home / ".codex"),
                "pi_sessions": str(home / ".pi/agent/sessions"), "omp_config": str(home / ".omp"),
                "omp_agent": str(home / ".omp/agent"), "kimi_home": str(home / ".kimi-code")}
        child_env = _environment(home)
        child_env.update(HOME=str(home), USERPROFILE=str(home), PATH=str(fake_bin),
            HANGAR_LIST_DIRS=json.dumps(dirs), CP_RUST_NO_ORPHAN_SWEEP="1",
            HANGAR_SERVER_LISTEN=f"127.0.0.1:{public_port}", HANGAR_SERVER_UPSTREAM=f"127.0.0.1:{desktop_port}",
            HANGAR_INTERNAL_SECRET="protected-stop-internal-fake", CP_AUTH_TOKEN=OWNER,
            CP_FORWARDED_ALLOW_IPS="127.0.0.1", HANGAR_SERVER_LOG=str(home / "owned-root.log"),
            **context.child_env(binary))
        supervisor = rust_server.Supervisor(binary, "127.0.0.1", public_port, desktop_port, OWNER, "127.0.0.1", lambda: False)
        supervisor.proc = process = rust_server._spawn(binary, child_env)
        original_kill = process.kill
        kills = [0]
        def counted_kill():
            kills[0] += 1
            return original_kill()
        process.kill = counted_kill
        stop_task = None
        try:
            async with httpx.AsyncClient(trust_env=False, timeout=1) as client:
                deadline = time.monotonic() + 12
                while True:
                    try:
                        response = await client.get(f"http://127.0.0.1:{public_port}/__hangar_server/health")
                        if response.status_code == 200:
                            assert response.json()["pid"] == process.pid
                            assert response.json()["guest_port_bound"] is False
                            break
                    except httpx.TransportError:
                        pass
                    assert process.poll() is None and time.monotonic() < deadline
                    await asyncio.sleep(.01)
            async with connect(f"ws://127.0.0.1:{public_port}/api/sessions/nav-contract/nav-remoto?token={OWNER}", proxy=None) as viewer:
                while json.loads(await asyncio.wait_for(viewer.recv(), 5))["t"] != "q":
                    pass
                await viewer.send(json.dumps({"t": "layout", "w": 900, "h": 700}))
                while json.loads(await asyncio.wait_for(viewer.recv(), 5))["t"] != "layout":
                    pass
                assert layout == "900x700"
                stopping = True
                began = time.monotonic()
                stop_task = asyncio.create_task(supervisor.stop())
                await asyncio.wait_for(cleanup_received.wait(), 5)
                await asyncio.wait({stop_task}, timeout=5.05)
                at_five = {"alive": process.poll() is None, "layout": layout, "commands": len(requests)}
                assert not stop_task.done() and at_five == {"alive": True, "layout": "900x700", "commands": 3}
                await asyncio.wait_for(asyncio.shield(stop_task), 15)
                elapsed = time.monotonic() - began
                await asyncio.wait_for(viewer.wait_closed(), 3)
                close = {"code": viewer.close_code, "reason": viewer.close_reason}
                assert requests == reference["cmd"] and layout == reference["desktop_layout"]
                assert close == reference["client_close"]
                assert process.returncode == 0 and kills[0] == 0
                return {"at_five": at_five, "layout": layout, "commands": len(requests), "close": close,
                    "exit_code": process.returncode, "kills": kills[0], "elapsed_seconds": elapsed,
                    "deadline_seconds": rust_server.PROTECTED_SHUTDOWN_SECONDS}
        finally:
            if process.poll() is None:
                original_kill()
                await asyncio.to_thread(process.wait)
            if stop_task is not None:
                await asyncio.gather(stop_task, return_exceptions=True)
            await asyncio.to_thread(runtime_process.cleanup, process)
            shared_auth.shutdown()
            desktop_server.should_exit = True
            await desktop_task
            listener.close()


if __name__ == "__main__" and sys.argv[1:2] == ["--worker"]:
    sys.path.insert(0, str(ROOT / "backend"))
    print(json.dumps(asyncio.run(_worker(Path(sys.argv[2])))))
