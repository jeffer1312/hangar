"""Ponte da lista: erro levanta, nunca vira lista vazia."""
import asyncio
import json
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from app import list_bridge, tmux


@pytest.fixture
def bridge():
    state = {"requests": [], "reply": None}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers["content-length"])))
            state["requests"].append((payload, self.headers["x-hangar-internal"]))
            body = json.dumps(state["reply"]).encode()
            self.send_response(200)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    list_bridge.configure(f"127.0.0.1:{server.server_port}", "segredo-sintético")
    yield state
    list_bridge.configure(None, None)
    server.shutdown()
    server.server_close()
    thread.join()


def test_list_bridge_returns_rows(bridge):
    bridge["reply"] = {"ok": True, "result": [{"name": "alfa", "provider": "claude", "state": "idle"}]}
    rows = list_bridge.discover(newer_than=12.5)
    assert [r.name for r in rows] == ["alfa"]
    assert bridge["requests"] == [({"op": "list.discover", "args": {"newer_than": 12.5}}, "segredo-sintético")]


@pytest.mark.parametrize("reply", [
    {"ok": False, "error": {"code": "list_procs_unreadable", "detail": "mapa de processos ilegível"}},
    {"ok": True, "result": None},
    {"resultado": []},
    {"ok": True, "result": [{"state": "idle"}]},
])
def test_list_bridge_error_raises_never_empty(bridge, reply):
    bridge["reply"] = reply
    for call in (list_bridge.discover, list_bridge.snapshot):
        with pytest.raises(list_bridge.ListBridgeError):
            call()


def test_list_bridge_mux_unavailable_raises_mux_error(bridge):
    bridge["reply"] = {"ok": False, "error": {"code": "mux_unavailable", "detail": "mux_timeout"}}
    with pytest.raises(tmux.MuxIndisponivel):
        list_bridge.discover()


def test_list_bridge_resolve_returns_pair(bridge):
    bridge["reply"] = {"ok": True, "result": {"jsonl": "/x/a.jsonl", "tracked": True}}
    assert list_bridge.resolve("alfa", "/w", pid=7) == ("/x/a.jsonl", True)
    assert bridge["requests"][0][0] == {"op": "list.resolve", "args": {"name": "alfa", "cwd": "/w", "pid": 7}}


def test_list_bridge_off_or_down_raises():
    list_bridge.configure(None, None)
    with pytest.raises(list_bridge.ListBridgeError) as off:
        list_bridge.discover()
    assert off.value.code == "list_bridge_off"
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    list_bridge.configure(f"127.0.0.1:{port}", "segredo")
    try:
        with pytest.raises(list_bridge.ListBridgeError) as down:
            list_bridge.seed("alfa", "/x/a.jsonl")
        assert down.value.code == "list_bridge_unavailable"
    finally:
        list_bridge.configure(None, None)


@pytest.mark.asyncio
async def test_list_bridge_on_the_loop_is_logged_once_and_still_runs(bridge, monkeypatch):
    from app import diag
    logged = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel, **campos: logged.append((evento, campos["codigo"])))
    monkeypatch.setattr(list_bridge, "_on_loop_seen", set())
    bridge["reply"] = {"ok": True, "result": None}
    for _ in range(2):
        list_bridge.invalidate()
    assert logged == [("lista.ponte_no_laco", "list.invalidate")]
    list_bridge.invalidate()
    await asyncio.to_thread(list_bridge.invalidate)
    assert logged == [("lista.ponte_no_laco", "list.invalidate")] * 2
    assert len(bridge["requests"]) == 4


def test_dirs_env_has_every_folder(hangar_server_bin):
    assert set(json.loads(list_bridge.dirs_env())) == {
        "home", "claude", "codex_home", "pi_sessions", "omp_config", "omp_agent", "kimi_home"}
