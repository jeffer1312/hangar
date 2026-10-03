"""A reserva de leitura não pode repetir uma alteração já enviada."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from app import workspace_bridge
from app.git_ops import GitError


@pytest.fixture
def bridge_server():
    requests = []
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers["content-length"])))
            requests.append(payload)
            if payload["op"] == "push":
                self.close_connection = True
                return
            body = json.dumps({"ok": False, "error": {"status": 409, "detail": "recusado"}}).encode()
            self.send_response(200)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    workspace_bridge.configure(f"127.0.0.1:{server.server_port}", "segredo-sintético")
    yield requests
    workspace_bridge.configure(None, None)
    server.shutdown()
    server.server_close()
    thread.join()


def test_domain_refusal_is_not_replaced_by_python(bridge_server):
    calls = []
    @workspace_bridge.delegate("list_branches", GitError)
    def original(cwd):
        calls.append(cwd)
    with pytest.raises(GitError) as error:
        original("pasta")
    assert error.value.status == 409
    assert calls == []
    assert len(bridge_server) == 1


def test_lost_mutation_reply_never_repeats_the_operation(bridge_server):
    calls = []
    @workspace_bridge.delegate("push", GitError, mutation=True)
    def original(cwd):
        calls.append(cwd)
    with pytest.raises(GitError) as error:
        original("pasta")
    assert error.value.status == 503
    assert calls == []
    assert len(bridge_server) == 1


def test_disabled_bridge_uses_python_once():
    workspace_bridge.configure(None, None)
    calls = []
    @workspace_bridge.delegate("list_branches", GitError)
    def original(cwd):
        calls.append(cwd)
        return {"current": "main"}
    assert original("pasta") == {"current": "main"}
    assert calls == ["pasta"]


@pytest.mark.parametrize("address", ["example.invalid:8765", "0.0.0.0:8765", "127.0.0.1:0", "127.0.0.1:8765/fora"])
def test_only_literal_loopback_is_accepted(address):
    with pytest.raises(ValueError):
        workspace_bridge.configure(address, "segredo-sintético")
