"""A borda Python conserva autenticação e só transporta a consulta ao Rust."""
import ast
import base64
import hashlib
import hmac
import json
import logging
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

import pytest
import uvicorn
from fastapi import Depends, FastAPI, Request
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
from app import account_bridge, auth, guest_users, internal_api, runtime_coordinator
from app.guest_user_gate import GuestUserGate
from app.mensagens import erro

OWNER = "contract-owner-token"
SECRET = "remote-browser-test-internal"


@pytest.fixture
def border(monkeypatch, tmp_path, request):
    monkeypatch.setattr(auth.settings, "auth_token", OWNER)
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    monkeypatch.setattr(guest_users, "_state", None)
    auth.reset_backoff()
    internal_api.set_secret(SECRET)
    app = FastAPI()
    app.add_middleware(GuestUserGate)
    source = Path(__file__).resolve().parents[1] / "app" / "api.py"
    text = source.read_text()
    reference = getattr(request, "param", None)
    if reference == "original":
        run = subprocess.run

        def without_history(command, *args, **kwargs):
            program = command[0] if isinstance(command, (list, tuple)) else command.split()[0]
            if Path(program).name in {"git", "git.exe"}:
                pytest.fail("a referência original não pode depender do histórico Git")
            return run(command, *args, **kwargs)

        monkeypatch.setattr(subprocess, "run", without_history)
        fixture = json.loads((Path(__file__).parent / "fixtures/contract/remote-browser/url-extra.json").read_text())
        assert fixture["source_commit"] == "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
        text = fixture["original_get"]["source"]
        assert hashlib.sha256(text.encode("utf-8")).hexdigest() == fixture["original_get"]["sha256"]
    nodes = [node for node in ast.parse(text).body
             if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
             and node.name in {"_repasse_ao_rust", "navegador_da_sessao"}]
    scope = {"app": app, "Depends": Depends, "Request": Request, "JSONResponse": JSONResponse,
             "require_auth": auth.require_auth, "erro": erro, "HTTPException": __import__("fastapi").HTTPException,
             "Path": Path, "json": json, "urllib": urllib, "_log": logging.getLogger("app.api")}
    exec(compile(ast.fix_missing_locations(ast.Module(body=nodes, type_ignores=[])), str(source), "exec"), scope)
    yield app
    account_bridge.configure_preparation(None, None)
    internal_api.set_secret(None)
    auth.reset_backoff()


@pytest.fixture
def rust():
    calls = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            calls.append((self.path, dict(self.headers)))
            body = b'{"url":"https://rust.invalid/"}'
            self.send_response(202)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    account_bridge.configure_preparation(f"127.0.0.1:{server.server_port}", SECRET)
    yield calls
    server.shutdown()
    server.server_close()
    thread.join(timeout=3)
    assert not thread.is_alive()


@pytest.fixture
def border_http(border):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    server = uvicorn.Server(uvicorn.Config(border, log_level="critical", lifespan="off", proxy_headers=False))
    thread = threading.Thread(target=server.run, kwargs={"sockets": [listener]})
    thread.start()
    deadline = time.monotonic() + 3
    try:
        while not server.started and thread.is_alive() and time.monotonic() < deadline:
            threading.Event().wait(.01)
        assert server.started
        yield f"http://127.0.0.1:{listener.getsockname()[1]}"
    finally:
        server.should_exit = True
        thread.join(timeout=3)
        listener.close()
        assert not thread.is_alive()


def test_get_forwards_request_and_exact_response_without_reading_sidecar(border_http, rust):
    with urllib.request.urlopen(border_http + "/api/sessions/sess%C3%A3o%20%2520/navegador?token=" + OWNER + "&unused=%25") as response:
        assert response.status == 202
        assert json.loads(response.read()) == {"url": "https://rust.invalid/"}
        assert response.headers["content-type"] == "application/json"
    assert len(rust) == 1
    path, headers = rust[0]
    assert path == "/__hangar_server/forwarded?token=" + OWNER + "&unused=%25"
    assert headers["X-Hangar-Path"] == "/api/sessions/sess%C3%A3o%20%2520/navegador"
    assert headers["X-Hangar-Internal"] == SECRET
    assert "Authorization" not in headers and "Cookie" not in headers


def test_python_transport_keeps_emoji_name_accepted_by_original_get(border_http, rust):
    try:
        with urllib.request.urlopen(border_http + "/api/sessions/sess%C3%A3o-%F0%9F%9A%80/navegador?token=" + OWNER) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    assert status == 202
    assert len(rust) == 1
    assert rust[0][1]["X-Hangar-Path"] == "/api/sessions/sess%C3%A3o-%F0%9F%9A%80/navegador"


@pytest.mark.parametrize("border", ["original"], indirect=True)
def test_original_get_accepts_emoji_name_over_real_http(border_http):
    nav = Path.home() / ".hangar/nav"
    nav.mkdir(parents=True, exist_ok=True)
    (nav / "native.json").write_text(json.dumps({"chave": "sessão-🚀", "url": "https://example.invalid/ação"}))
    with urllib.request.urlopen(border_http + "/api/sessions/sess%C3%A3o-%F0%9F%9A%80/navegador?token=" + OWNER) as response:
        assert response.status == 200
        assert json.loads(response.read()) == {"url": "https://example.invalid/ação"}


@pytest.mark.parametrize("mode,configured,code", [
    ("python", True, "browser_need_rust_server"),
    ("pending", False, "account_bridge_unavailable"),
])
def test_no_python_fallback_when_rust_is_unavailable(border, rust, monkeypatch, mode, configured, code):
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode=mode))
    if not configured:
        account_bridge.configure_preparation(None, None)
    with TestClient(border) as client:
        response = client.get("/api/sessions/session/navegador", headers={"Authorization": "Bearer " + OWNER})
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == code
    assert rust == []


def test_auth_precedence_and_rate_limit_stop_before_forwarding(border, rust):
    with TestClient(border, client=("198.51.100.9", 1234)) as client:
        for _ in range(8):
            response = client.get("/api/sessions/session/navegador?token=" + OWNER,
                                  headers={"Authorization": "Bearer wrong"})
            assert response.status_code == 401
        response = client.get("/api/sessions/session/navegador?token=" + OWNER)
        assert response.status_code == 429
        assert response.headers["retry-after"] == "30"
    assert rust == []


def test_login_guest_gate_blocks_before_auth_counter_or_transport(border, rust, tmp_path):
    _, token = guest_users.create("contract-guest", str(tmp_path), False, True)
    with TestClient(border, client=("198.51.100.9", 1234)) as client:
        response = client.get("/api/sessions/session/navegador", headers={"Authorization": "Bearer " + token})
    fixture = json.loads((Path(__file__).parent / "fixtures/contract/remote-browser/url-commands.json").read_text())
    want = next(case for case in fixture["cases"] if case["name"] == "get_login_guest_is_blocked")["expected"]["responses"][0]
    assert response.status_code == want["status"]
    assert response.json() == want["body"]
    assert rust == []
    assert auth._fails == {}


def test_signed_share_uses_existing_authentication_and_transport(border, rust):
    target = "/api/sessions/session/navegador"
    payload = base64.urlsafe_b64encode(json.dumps({"s": {"session": "share"}, "exp": 4102444800}).encode()).rstrip(b"=")
    mac = hmac.new(SECRET.encode(), b"GET\n" + target.encode() + b"\n" + payload, hashlib.sha256).hexdigest()
    with TestClient(border, client=("127.0.0.1", 1234)) as client:
        response = client.get(target, headers={"x-hangar-guest": payload.decode() + "." + mac})
    assert response.status_code == 202
    assert response.json() == {"url": "https://rust.invalid/"}
    assert len(rust) == 1
