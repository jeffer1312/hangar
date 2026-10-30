"""Captura desconexão HTTP em app mínimo: sem lifespan, descoberta ou sessão real."""
import asyncio
import json
from pathlib import Path
import socket
import subprocess
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from fastapi import FastAPI
import uvicorn
from app import config_sync as cs, config_sync_api as api


async def capture():
    started, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    events = []

    async def accepted():
        pass

    async def apply(*args, **kwargs):
        events.append("started")
        started.set()
        await release.wait()
        events.append("finished")
        finished.set()
        return {"items": {}}

    with tempfile.TemporaryDirectory() as folder, socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.setblocking(False)
        app = FastAPI()
        app.include_router(api.config_sync_router)
        app.dependency_overrides[api.require_auth] = accepted
        server = uvicorn.Server(uvicorn.Config(app, log_level="error", lifespan="off"))
        roots = SimpleNamespace(this_machine=lambda: cs.Roots(home=folder, claude=folder, codex=folder, hangar=folder))
        with patch.object(api, "Roots", roots), patch.object(api, "_APPLYING", asyncio.Lock()), patch.object(cs, "apply_bundle", apply):
            serving = asyncio.create_task(server.serve(sockets=[listener]))
            try:
                for _ in range(500):
                    if server.started:
                        break
                    await asyncio.sleep(0.01)
                assert server.started
                payload = cs.pack(cs.Bundle(items={"hangar_prefs": {"prefs": {"editor": "vim"}}}))
                _, writer = await asyncio.open_connection(*listener.getsockname())
                writer.write((f"POST /api/config-sync/apply?items=hangar_prefs HTTP/1.1\r\nHost: fixture\r\nContent-Length: {len(payload)}\r\n\r\n").encode() + payload)
                await writer.drain()
                await asyncio.wait_for(started.wait(), 5)
                writer.close()
                await writer.wait_closed()
                events.append("closed")
                await asyncio.sleep(0.05)
                held_after_disconnect = api._APPLYING.locked()
                release.set()
                await asyncio.wait_for(finished.wait(), 5)
                await asyncio.sleep(0)
                return {"events": events, "locked_after_disconnect": held_after_disconnect,
                        "locked_after_finish": api._APPLYING.locked()}
            finally:
                release.set()
                server.should_exit = True
                await asyncio.wait_for(serving, 5)


Path(__file__).with_name("apply-socket-disconnect.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "command": "cd backend && PYTHONPATH=. uv run python tests/fixtures/contract/config_sync/generate_apply_socket_disconnect.py",
    "injections": ["aplicador suspenso por eventos; autenticação aceita; app mínimo sem lifespan; socket descartável de loopback"],
    "result": asyncio.run(capture()),
}, indent=2) + "\n", encoding="utf-8")
