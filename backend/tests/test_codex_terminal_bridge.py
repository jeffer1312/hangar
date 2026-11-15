"""Repasse num sentido, à ponte do Rust, do que entra pelo Python para um Codex com terminal dele."""
import asyncio
import http.server
import threading

import pytest
from starlette.requests import Request

import app.api as api_mod
from app import list_bridge, runtime_coordinator
from app.runtime_coordinator import RuntimeCoordinator
from app.config import GUEST_PORT


def _owner(monkeypatch, mode="rust", terminal=True):
    owner = RuntimeCoordinator(None, None)
    owner.mode = mode
    owner._owns = {("claude", True), ("claude", False), ("codex", True)} | ({("codex", False)} if terminal else set())
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    return owner


def _provider(monkeypatch, **providers):
    from types import SimpleNamespace
    monkeypatch.setattr(api_mod, "_cached_info_sync", lambda name: SimpleNamespace(provider=providers[name]))


def test_the_bridge_is_called_one_way_and_421_means_not_ours(monkeypatch):
    seen = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("content-length") or 0))
            seen.append((self.path, self.headers.get("x-hangar-internal"), body))
            self.send_response(421 if "/py/" in self.path else 200)
            self.send_header("content-length", "2")
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{server.server_port}", "s"))
        owner = _owner(monkeypatch, terminal=False)
        assert runtime_coordinator.rust_codex_terminal("cx", "input", body=b"{}") is None, "sem o anúncio, nada sai"
        owner._owns.add(("codex", False))
        assert runtime_coordinator.rust_codex_terminal("cx", "input", body=b'{"text":"oi"}') == (200, b"{}")
        assert seen == [("/__hangar_server/codex/cx/input", "s", b'{"text":"oi"}')]
        assert runtime_coordinator.rust_codex_terminal("py", "kill") is None, "421: o Python segue como hoje"
        owner.mode = "python"
        assert runtime_coordinator.rust_codex_terminal("cx", "input") is None
        assert len(seen) == 2
    finally:
        server.shutdown()



def test_the_claude_bridge_carries_who_asked(monkeypatch):
    seen = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            seen.append((self.path, self.headers.get("user-agent"), self.headers.get("x-hangar-req"),
                         self.headers.get("x-forwarded-for"), self.headers.get("x-hangar-internal")))
            self.send_response(200)
            self.send_header("content-length", "2")
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{server.server_port}", "s"))
        _owner(monkeypatch)
        who = {"user-agent": "Hangar/iPad", "x-hangar-req": "r-1", "x-forwarded-for": "100.64.0.7"}
        assert runtime_coordinator.rust_claude("ct", "close", "POST", b"", headers=who) == (200, b"{}")
        assert seen == [("/__hangar_server/claude/ct/close", "Hangar/iPad", "r-1", "100.64.0.7", "s")]
    finally:
        server.shutdown()



def _request(port, path="/api/sessions/cx/keys", query=b"", body=b'{"key":"Up"}', method="POST"):
    async def receive():
        return {"type": "http.request", "body": body, "more_body": False}
    return Request({"type": "http", "method": method, "path": path, "query_string": query, "headers": [],
                    "server": ("127.0.0.1", port)}, receive)



def test_claude_question_reaches_the_rust_whole_and_421_leaves_it_to_python(monkeypatch):
    seen = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            seen.append((self.path, self.headers.get("x-hangar-internal"), self.rfile.read(int(self.headers.get("content-length") or 0))))
            status, body = (421, b"") if "/py/" in self.path else (409, b'{"detail": {"code": "erro_interacao_desatualizada"}}')
            self.send_response(status)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    body = b'{"answers": [], "interaction_id": "ask:7"}'
    try:
        monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{server.server_port}", "s"))
        owner = _owner(monkeypatch)
        assert runtime_coordinator.rust_claude("ct", "answer", body=body) == (409, b'{"detail": {"code": "erro_interacao_desatualizada"}}'), \
            "status e corpo do Rust voltam como vieram"
        assert seen == [("/__hangar_server/claude/ct/answer", "s", body)], "corpo e interaction_id intactos"
        assert runtime_coordinator.rust_claude("py", "select") is None, "421: o Rust não atende, segue o caminho de hoje"
        owner.mode = "python"
        assert runtime_coordinator.rust_claude("ct", "answer", body=body) is None
        assert len(seen) == 2
    finally:
        server.shutdown()


