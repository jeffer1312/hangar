"""O Rust é o único que grava `.hangar-pair`: o que chega às portas do Python segue pela ponte
`/__hangar_server/groups`, num sentido."""
import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import api, groups_bridge, pair, runtime_coordinator
from app.config import settings

H = {"Authorization": "Bearer secret"}


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "arquivo")
    return tmp_path


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "secret")
    return TestClient(api.app)


def test_ownership_follows_mode(monkeypatch):
    for mode, owned in (("rust", True), ("pending", True)):
        monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode=mode))
        assert groups_bridge.rust_owns_groups() is owned, mode
    monkeypatch.setattr(runtime_coordinator, "_current", None)
    assert groups_bridge.rust_owns_groups() is False


def test_call_sends_envelope_and_maps_errors(monkeypatch):
    seen = []
    replies = [{"ok": True, "result": {"ex_peers": ["b"], "warnings": []}},
               {"ok": False, "error": {"code": "groups_store_failed", "detail": "x"}}]

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            seen.append((self.path, self.headers["x-hangar-internal"],
                         json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
            body = json.dumps(replies.pop(0)).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    groups_bridge.configure(f"127.0.0.1:{srv.server_port}", "s3gredo")
    try:
        assert groups_bridge.call("group.leave", name="a") == {"ex_peers": ["b"], "warnings": []}
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.rename", old="a", new="b")
        assert e.value.code == "groups_store_failed"
    finally:
        srv.shutdown()
        groups_bridge.configure(None, None)
    assert seen == [("/__hangar_server/groups", "s3gredo", {"op": "group.leave", "args": {"name": "a"}}),
                    ("/__hangar_server/groups", "s3gredo", {"op": "group.rename", "args": {"old": "a", "new": "b"}})]
    with pytest.raises(groups_bridge.GroupsBridgeError) as e:
        groups_bridge.call("group.leave", name="a")
    assert e.value.code == "groups_bridge_off"


def _pending_owner(outcome, on_settle=lambda: None):
    """Dono em `pending` cujo `await_mode` decide `outcome` no laço do servidor (outra thread)."""
    loop = asyncio.new_event_loop()
    threading.Thread(target=loop.run_forever, daemon=True).start()
    owner = SimpleNamespace(mode="pending", loop=loop)

    async def await_mode():
        on_settle()
        owner.mode = outcome
        return outcome
    owner.await_mode = await_mode
    return owner


def test_call_waits_for_pending_rust_then_uses_the_new_bridge(monkeypatch):
    """O servidor limpa as pontes antes de (re)subir o Rust: no `pending` a chamada espera o filho
    novo em vez de falhar na hora com `groups_bridge_off`."""
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.dumps({"ok": True, "result": {}}).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    groups_bridge.configure(None, None)
    owner = _pending_owner("rust", lambda: groups_bridge.configure(f"127.0.0.1:{srv.server_port}", "s"))
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    try:
        assert groups_bridge.call("group.rename", old="a", new="b") == {}
    finally:
        srv.shutdown()
        groups_bridge.configure(None, None)
        owner.loop.call_soon_threadsafe(owner.loop.stop)


def test_call_waits_while_the_mode_is_settling(monkeypatch):
    """Entrando no modo `rust` (`_settling`) as pontes ainda podem estar limpas: espera como o `await_mode`."""
    seen = []
    owner = _pending_owner("rust", lambda: groups_bridge.configure("127.0.0.1:9", "s"))
    owner.mode, owner._settling = "rust", True
    monkeypatch.setattr(groups_bridge, "_opener", SimpleNamespace(open=lambda req, timeout: seen.append(req) or (_ for _ in ()).throw(OSError())))
    groups_bridge.configure(None, None)
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    try:
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.leave", name="a")
        # Chegou à ponte nova (o transporte falso falha), em vez de `groups_bridge_off` na hora.
        assert (e.value.code, len(seen)) == ("groups_bridge_unavailable", 1)
    finally:
        groups_bridge.configure(None, None)
        owner.loop.call_soon_threadsafe(owner.loop.stop)


def test_lone_surrogate_in_the_task_reaches_the_rust_instead_of_500(monkeypatch):
    # Parity B13 (`test_set_com_surrogate_solto_na_task_grava_e_le_de_volta`): texto colado com surrogate
    # solto não pode derrubar a rota; o Rust só aceita UTF-8 válido, então vai o caractere de troca.
    seen = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            seen.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            body = json.dumps({"ok": True, "result": {}}).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    groups_bridge.configure(f"127.0.0.1:{srv.server_port}", "s3gredo")
    try:
        assert groups_bridge.call("group.task", name="a", task="ok \ud800 fim") == {}
    finally:
        srv.shutdown()
        groups_bridge.configure(None, None)
    assert seen[0]["args"]["task"] == "ok \ufffd fim"


def test_bridge_gives_up_on_a_silent_rust_with_a_code(monkeypatch):
    """BRG-06: a ponte tem prazo (90 s, para o `/pair` entre m\u00e1quinas); Rust mudo vira c\u00f3digo, n\u00e3o espera eterna."""
    assert groups_bridge._TIMEOUT == 90
    release = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            release.wait(5)

        def log_message(self, *a):
            pass
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    monkeypatch.setattr(groups_bridge, "_TIMEOUT", 0.2)
    groups_bridge.configure(f"127.0.0.1:{srv.server_port}", "s3gredo")
    try:
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.leave", name="a")
        assert e.value.code == "groups_bridge_unavailable"
    finally:
        release.set()
        srv.shutdown()
        groups_bridge.configure(None, None)




def test_associate_in_rust_mode_forwards_without_rebuilding_orq_facts(monkeypatch, client):
    from app import account_bridge
    from fastapi.responses import JSONResponse
    calls = []
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    monkeypatch.setattr(groups_bridge, "call", lambda *a, **k: pytest.fail("a associação HTTP não volta pela ponte de grupos"))

    async def forward(request, destination, **options):
        calls.append((destination, request.method, request.url.path, await request.json(), options))
        return JSONResponse({"ok": True, "gid": "g1", "grouped": True})
    monkeypatch.setattr(account_bridge, "forward_public", forward)
    r = client.post("/api/sessions/arb/orq/grupo", headers=H, json={"gid": "g1", "mtime": 1.5})
    assert r.status_code == 200, r.text
    assert r.json() == {"ok": True, "gid": "g1", "grouped": True}
    assert calls == [("/__hangar_server/forwarded", "POST", "/api/sessions/arb/orq/grupo", {"gid": "g1", "mtime": 1.5}, {"limit": None})]


def test_child_gets_group_env(monkeypatch, tmp_path):
    from app import rust_server
    monkeypatch.delenv("HANGAR_SERVER_ID", raising=False)
    monkeypatch.delenv("HANGAR_PEERS_FILE", raising=False)
    monkeypatch.setattr(settings, "server_id", "casa")
    sup = rust_server.Supervisor(tmp_path / "bin", "127.0.0.1", 1, 2, "t", "127.0.0.1", lambda: False)
    try:
        env = sup._env()
    finally:
        from app import internal_api
        internal_api.set_secret(None)
    assert "HANGAR_SERVER_ID" not in env
    assert "HANGAR_PEERS_FILE" not in env
    assert env["HANGAR_PAIR_ARCHIVE"] == str(pair._arquivo_dir())
