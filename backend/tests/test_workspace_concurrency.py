"""Comandos lentos não bloqueiam metadados nem repositórios independentes."""
import concurrent.futures
import asyncio
import json
import os
import socket
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest

pytestmark = pytest.mark.skipif(os.name == "nt", reason="Fixture de Git usa shell POSIX")
_BINARY = Path(__file__).parents[2] / "crates/target/debug/hangar-server"


def test_supervisor_starts_runtime_and_workspace_bridges_together(tmp_path, monkeypatch):
    from app import account_bridge, internal_api, runtime_coordinator, runtime_queue, rust_server, workspace_bridge

    monkeypatch.setattr(runtime_coordinator, "_current", None)
    monkeypatch.setattr(runtime_queue, "_coordinator", None)
    monkeypatch.setattr(rust_server, "server_log_path", lambda: tmp_path / "server.log")
    # O binário real varre canos órfãos ao subir: nunca com o HOME e os processos desta máquina.
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("CP_RUST_NO_ORPHAN_SWEEP", "1")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git/HEAD").write_text("ref: refs/heads/main\n")
    supervisor = rust_server.Supervisor(_BINARY, "127.0.0.1", port, 9,
                                        "fixture-owner", "127.0.0.1", lambda: False)

    async def scenario():
        try:
            assert await supervisor._start() == "up"
            assert supervisor.runtime_transport is not None
            configured = account_bridge.private_transport() is not None
            assert configured, "a ponte de contas não recebeu o endereço privado"
            assert await asyncio.to_thread(workspace_bridge.request, "head_info", {"cwd": str(tmp_path)}) == {
                "ok": True, "result": ["main", False]}
        finally:
            await supervisor.stop()
            internal_api.set_secret(None)
        assert workspace_bridge.request("head_info", {"cwd": str(tmp_path)}) is None
        assert supervisor.runtime_transport is None
        # O segredo interno não pode seguir para uma porta que o filho morto liberou.
        cleared = account_bridge.private_transport() is None
        assert cleared, "a ponte de contas ainda guarda o endereço do filho encerrado"

    asyncio.run(scenario())


@pytest.fixture
def server(tmp_path):
    shim = tmp_path / "bin"
    shim.mkdir()
    markers = tmp_path / "markers"
    markers.mkdir()
    git = shim / "git"
    git.write_text('#!/bin/sh\nif [ "$3" = fetch ] || [ "$2" = "$HANGAR_SLOW_CWD" ]; then touch "$HANGAR_MARKERS/start$$"; sleep 1; touch "$HANGAR_MARKERS/done$$"; fi\nprintf "## main\\n"\n', encoding="utf-8")
    git.chmod(0o755)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = {**os.environ, "PATH":str(shim)+os.pathsep+os.environ["PATH"], "HANGAR_MARKERS":str(markers),
           "HANGAR_SLOW_CWD":str(tmp_path / "slow"), "HANGAR_SERVER_LISTEN":f"127.0.0.1:{port}",
           "HANGAR_SERVER_UPSTREAM":"127.0.0.1:9", "HANGAR_INTERNAL_SECRET":"fixture-internal",
           "CP_AUTH_TOKEN":"fixture-owner", "CP_FORWARDED_ALLOW_IPS":"127.0.0.1"}
    proc = subprocess.Popen([str(_BINARY)], env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 5
    while True:
        try:
            health = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/__hangar_server/health", timeout=1))
            break
        except OSError:
            assert time.monotonic() < deadline
            time.sleep(.01)
    def call(operation, **args):
        req = urllib.request.Request(f"http://{health['terminal_address']}/__hangar_server/workspace",
            data=json.dumps({"op":operation,"args":args}, default=os.fspath).encode(),
            headers={"x-hangar-internal":"fixture-internal","content-type":"application/json"})
        return json.load(urllib.request.urlopen(req, timeout=5))
    yield tmp_path, markers, call
    proc.stdin.close()
    proc.wait(timeout=5)


def wait_markers(markers, count):
    deadline = time.monotonic() + 4
    while len(list(markers.glob("start*"))) < count:
        assert time.monotonic() < deadline
        time.sleep(.01)


def test_metadata_remains_available_while_mutation_slots_are_busy(server):
    root, markers, call = server
    (root / ".git").mkdir()
    (root / ".git/HEAD").write_text("ref: refs/heads/main\n")
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        pending = [pool.submit(call, "git_action", cwd=root, action="fetch") for _ in range(4)]
        wait_markers(markers, 4)
        assert call("head_info", cwd=root) == {"ok":True,"result":["main",False]}
        assert not list(markers.glob("done*"))


def test_slow_repo_does_not_serialize_other_repo_summaries(server):
    root, markers, call = server
    for name in ("slow", "fast"):
        (root / name / ".git").mkdir(parents=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        pending = pool.submit(call, "git_summary", cwd=root / "slow")
        wait_markers(markers, 1)
        assert call("git_summary", cwd=root / "fast")["ok"]
        assert not list(markers.glob("done*"))
