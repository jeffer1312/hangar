"""Repasse num sentido, à ponte do Rust, do que entra pelo Python para um Codex com terminal dele."""
import asyncio
import http.server
import logging
import threading
import urllib.parse

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


def test_bridge_failures_become_a_coded_failure_never_an_exception(monkeypatch):
    import json
    import socket

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            status, body = (404, b"nope") if "/text/" in self.path else (409, b'{"detail": {"code": "close_kill_survived"}}')
            self.send_response(status)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            body = b'[{"name": "compact"}]'
            self.send_response(200)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    def code(reply):
        return json.loads(reply[1])["detail"]["code"]

    _owner(monkeypatch)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{server.server_port}", "s"))
        refused = runtime_coordinator.rust_codex_terminal("cx", "kill")
        assert refused[0] == 409 and code(refused) == "close_kill_survived", "o envelope do Rust passa como veio"
        text = runtime_coordinator.rust_codex_terminal("text", "input", body=b"{}")
        assert text[0] == 502 and code(text) == "codex_bridge_failed", "corpo que não é JSON vira código"
        assert runtime_coordinator.rust_codex_terminal("cx", "commands", "GET") == (200, b'[{"name": "compact"}]'), \
            "sucesso em lista (o /commands) passa intacto"
    finally:
        server.shutdown()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        closed = probe.getsockname()[1]
    monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{closed}", "s"))
    down = runtime_coordinator.rust_codex_terminal("cx", "input", body=b"{}")
    assert down[0] == 503 and code(down) == "codex_bridge_failed", "Rust fora do ar"
    monkeypatch.setattr(list_bridge, "endpoint", lambda: None)
    unannounced = runtime_coordinator.rust_codex_terminal("cx", "kill")
    assert unannounced[0] == 503 and code(unannounced) == "codex_bridge_failed", "ponte sem endereço: o Python não assume"
    monkeypatch.setattr(list_bridge, "endpoint", lambda: (f"127.0.0.1:{closed}", "s"))
    assert asyncio.run(api_mod._send_one_codex("cx", "oi"))["error"]["code"] == "codex_bridge_failed", \
        "o envio do broadcast/grupo/MCP devolve a falha, sem levantar"

    from http import client as http_client

    class Slow:
        sent = True

        def __init__(self, *args, **kwargs):
            pass

        def request(self, *args, **kwargs):
            if not Slow.sent:
                raise TimeoutError("timed out")

        def getresponse(self):
            raise TimeoutError("timed out")

        def close(self):
            pass
    monkeypatch.setattr(http_client, "HTTPConnection", Slow)
    late = runtime_coordinator.rust_codex_terminal("cx", "input", body=b"{}")
    assert late[0] == 504 and code(late) == "codex_bridge_uncertain", "escrita enviada sem resposta: incerto, não repetir"
    assert code(runtime_coordinator.rust_codex_terminal("cx", "commands", "GET")) == "codex_bridge_failed", \
        "leitura sem resposta pode repetir"
    Slow.sent = False
    early = runtime_coordinator.rust_codex_terminal("cx", "input", body=b"{}")
    assert early[0] == 503 and code(early) == "codex_bridge_failed", "prazo antes de enviar: pode repetir"


def test_delete_answers_the_rust_code_when_the_close_fails(monkeypatch):
    from fastapi import HTTPException
    from app.registry import KillFailed

    def refuse(name, closed_by_rust=False):
        raise KillFailed(name, {"code": "codex_bridge_failed", "params": {}, "msg": "fora"})
    monkeypatch.setattr(api_mod.registry, "kill", refuse)
    monkeypatch.setattr(api_mod, "_invalidate_lists", lambda: None)
    with pytest.raises(HTTPException) as failed:
        asyncio.run(api_mod.kill_session("cx"))
    assert failed.value.status_code == 500 and failed.value.detail["code"] == "codex_bridge_failed"


def test_delete_entering_the_connect_port_is_closed_by_the_rust(monkeypatch):
    killed, recorded, cleaned, bridged = [], [], [], []
    monkeypatch.setattr(api_mod.registry, "kill", lambda name, closed_by_rust=False: killed.append(closed_by_rust) or [])
    monkeypatch.setattr(api_mod, "_jsonl_of_session", lambda name: None)
    monkeypatch.setattr(api_mod, "_invalidate_lists", lambda: cleaned.append("invalidate"))
    monkeypatch.setattr(api_mod.share_bridge, "revoke_session", lambda name: cleaned.append("revoke"))
    monkeypatch.setattr(api_mod.closed_names, "record", lambda jsonl, name: recorded.append((jsonl, name)))
    replies = iter([(200, b'{"closed": true, "killed": true, "transcript": "/t/a.jsonl"}'), None,
                    (409, b'{"detail": {"code": "session_transfer_busy", "params": {}}}')])
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args, **kw: bridged.append((args, kw)) or next(replies))
    _provider(monkeypatch, ct="claude", pi="pi")

    def delete(port, name="ct"):
        request = _request(port, f"/api/sessions/{name}", body=b"", method="DELETE")
        request.scope["headers"] = [(b"user-agent", b"Hangar/iPad"), (b"x-hangar-req", b"r-1"), (b"x-forwarded-for", b"100.64.0.7")]
        return asyncio.run(api_mod.kill_session(name, request=request))
    assert delete(api_mod.CONNECT_PORT)["ok"] is True
    assert (killed, recorded, cleaned) == ([True], [("/t/a.jsonl", "ct")], []), "fechada pelo Rust: aqui só as lojas daqui"
    delete(api_mod.CONNECT_PORT)
    assert killed[1:] == [False] and cleaned == ["invalidate", "revoke"], "421: o Python fecha pelo nome, como antes"
    with pytest.raises(api_mod._RustReply) as refused:
        delete(api_mod.CONNECT_PORT)
    assert refused.value.args[0] == 409 and len(killed) == 2, "recusa do Rust volta como veio, sem fechar aqui"
    # Quem pediu chega ao diário do Rust: cliente, pedido e origem.
    who = {"user-agent": "Hangar/iPad", "x-hangar-req": "r-1", "x-forwarded-for": "100.64.0.7"}
    assert bridged == [(("ct", "close", "POST", b""), {"headers": who})] * 3
    delete(GUEST_PORT)
    assert len(bridged) == 3, "só o que entrou pelo Connect vai à ponte"
    # Pi/omp/Kimi não são do Rust: com ele em `pending` a ponte responderia 503 em vez de fechar.
    delete(api_mod.CONNECT_PORT, "pi")
    assert len(bridged) == 3 and killed[3:] == [False], "Pi fecha pelo nome, sem passar pela ponte"


@pytest.mark.parametrize("failure", [api_mod.list_bridge.ListBridgeError("list_bridge_unavailable"),
                                     api_mod.tmux.MuxIndisponivel("mux")])
def test_connect_delete_without_the_list_closes_here(monkeypatch, failure):
    """Lista indisponível não descobre o provedor: o DELETE pelo Connect fecha pelo nome, sem 503."""
    killed, bridged = [], []
    monkeypatch.setattr(api_mod.registry, "kill", lambda name, closed_by_rust=False: killed.append(closed_by_rust) or [])
    monkeypatch.setattr(api_mod, "_jsonl_of_session", lambda name: None)
    monkeypatch.setattr(api_mod, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api_mod, "_sidecar_state", lambda name: "absent")

    def cached(name):
        raise failure
    monkeypatch.setattr(api_mod, "_cached_info_sync", cached)
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args, **kw: bridged.append(args))
    request = _request(api_mod.CONNECT_PORT, "/api/sessions/ct", body=b"", method="DELETE")
    assert asyncio.run(api_mod.kill_session("ct", request=request))["ok"] is True
    assert (killed, bridged) == ([False], [])


def test_connect_close_of_a_grouped_claude_leaves_the_group_from_here(monkeypatch):
    """O Rust fecha o Claude do Connect; a saída do grupo vai pela ponte e lê a fase no Rust."""
    left = []
    monkeypatch.setattr(api_mod.registry, "kill", lambda name, closed_by_rust=False: [])
    monkeypatch.setattr(api_mod.closed_names, "record", lambda jsonl, name: None)
    monkeypatch.setattr(api_mod, "_sidecar_state", lambda name: "present")
    monkeypatch.setattr(api_mod.groups_bridge, "rust_owns_groups", lambda: True)
    monkeypatch.setattr(api_mod.groups_bridge, "call", lambda op, **kw: left.append((op, kw)) or {"warnings": []})
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args, **kw: (200, b'{"closed": true, "transcript": ""}'))
    _provider(monkeypatch, ct="claude")
    request = _request(api_mod.CONNECT_PORT, "/api/sessions/ct", body=b"", method="DELETE")
    assert asyncio.run(api_mod.kill_session("ct", request=request)) == {"ok": True, "warning": None}
    assert left == [("group.leave", {"name": "ct", "phases": {}})]


def test_claude_reopen_entering_the_connect_port_goes_to_the_rust(monkeypatch):
    calls = []
    _provider(monkeypatch, ct="claude")
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: None)
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args, **kw: calls.append((args, kw)) or (200, b'{"ok": true}'))
    request = _request(api_mod.CONNECT_PORT, "/api/sessions/ct/recarregar", body=b"")
    request.scope["headers"] = [(b"user-agent", b"Hangar/iPad"), (b"x-hangar-req", b"r-2"), (b"x-forwarded-for", b"100.64.0.7")]
    with pytest.raises(api_mod._RustReply) as reply:
        asyncio.run(api_mod._codex_rust_port("ct", request))
    assert reply.value.args == (200, b'{"ok": true}')
    # A reabertura espera o Claude subir por até 5 min: o prazo da ponte passa disso.
    (args, kw), = calls
    assert args[:4] == ("ct", "recarregar", "POST", b"") and args[4] > 300, args
    assert kw == {"headers": {"user-agent": "Hangar/iPad", "x-hangar-req": "r-2", "x-forwarded-for": "100.64.0.7"}}


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


def test_sends_from_python_paths_go_to_the_rust_and_return_its_code(monkeypatch):
    calls = []
    monkeypatch.setattr(runtime_coordinator, "rust_owns_codex_terminal", lambda: True)
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal",
                        lambda *args: calls.append(args) or (200, b'{"ok": true, "delivered": true}'))

    async def no_python(*args, **kwargs):
        raise AssertionError("o Python não entrega no Codex com terminal do Rust")
    monkeypatch.setattr(api_mod, "_send_managed", no_python)
    assert asyncio.run(api_mod._send_one_codex("cx", "oi")) == {"ok": True, "delivered": True}
    assert calls == [("cx", "input", "POST", b'{"text": "oi"}')]
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: (409, b'{"detail": {"code": "x"}}'))
    assert asyncio.run(api_mod._send_one_codex("cx", "oi")) == {"ok": False, "error": {"code": "x"}, "delivered": False}


def _request(port, path="/api/sessions/cx/keys", query=b"", body=b'{"key":"Up"}', method="POST"):
    async def receive():
        return {"type": "http.request", "body": body, "more_body": False}
    return Request({"type": "http", "method": method, "path": path, "query_string": query, "headers": [],
                    "server": ("127.0.0.1", port)}, receive)


def test_writes_entering_the_guest_or_connect_port_are_forwarded_whole(monkeypatch):
    calls = []
    monkeypatch.setattr(api_mod.codex_sessions, "exists", lambda name: name == "cx")
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: calls.append(args) or (202, b"{}"))
    with pytest.raises(api_mod._RustReply) as reply:
        asyncio.run(api_mod._codex_rust_port("cx", _request(GUEST_PORT)))
    assert reply.value.args == (202, b"{}")
    assert calls == [("cx", "keys", "POST", b'{"key":"Up"}')]
    with pytest.raises(api_mod._RustReply):
        asyncio.run(api_mod._codex_rust_port("cx", _request(api_mod.CONNECT_PORT, "/api/sessions/cx/limits", b"a=1", b"")))
    assert calls[-1] == ("cx", "limits?a=1", "POST", b"")
    # O que chegou pela porta de trás do Rust é dele já: nunca volta para lá.
    asyncio.run(api_mod._codex_rust_port("cx", _request(1)))
    assert len(calls) == 2
    # Não é Codex com terminal do Rust: a rota do Python segue.
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: None)
    asyncio.run(api_mod._codex_rust_port("cx", _request(GUEST_PORT)))


def test_guest_write_to_pi_does_not_wait_for_the_codex_bridge(monkeypatch):
    import json
    _owner(monkeypatch, mode="pending")
    monkeypatch.setattr(list_bridge, "endpoint", lambda: None)
    monkeypatch.setattr(api_mod.codex_sessions, "exists", lambda name: name == "cx")
    # Pi pela 8766 com a ponte ainda sem endereço: a rota Python segue.
    asyncio.run(api_mod._codex_rust_port("pi", _request(GUEST_PORT, "/api/sessions/pi/keys")))
    with pytest.raises(api_mod._RustReply) as reply:
        asyncio.run(api_mod._codex_rust_port("cx", _request(GUEST_PORT)))
    assert reply.value.args[0] == 503 and json.loads(reply.value.args[1])["detail"]["code"] == "codex_bridge_failed"


def test_relaunch_routes_entering_the_guest_or_connect_port_go_to_the_rust():
    for path in ("/api/sessions/{name}/modo-execucao", "/api/sessions/{name}/recarregar"):
        route = next(r for r in api_mod.app.routes if getattr(r, "path", None) == path and "POST" in r.methods)
        assert any(d.dependency is api_mod._codex_rust_port for d in route.dependencies), path


def test_delete_marked_by_the_rust_only_cleans_python_stores(monkeypatch):
    from app import internal_api
    monkeypatch.setattr(internal_api, "_secret", "segredo")
    calls = []
    monkeypatch.setattr(api_mod.registry, "kill", lambda name, closed_by_rust=False: calls.append(closed_by_rust) or [])
    monkeypatch.setattr(api_mod, "_invalidate_lists", lambda: calls.append("invalidate"))
    monkeypatch.setattr(api_mod.share_bridge, "revoke_session", lambda name: calls.append("revoke"))

    def request(mark):
        return Request({"type": "http", "method": "DELETE", "path": "/api/sessions/cx", "query_string": b"",
                        "headers": [(b"x-hangar-rust-closed", mark)]})
    assert asyncio.run(api_mod.kill_session("cx", request=request(b"segredo")))["ok"] is True
    assert calls == [True], "marcado pelo Rust: só as lojas daqui, sem invalidar a lista nem pedir a revogação a ele"
    asyncio.run(api_mod.kill_session("cx", request=request(b"forjado")))
    assert calls[1:] == [False, "invalidate", "revoke"], "marca que não é o segredo não vale"
    asyncio.run(api_mod.kill_session("cx"))
    assert calls[4:] == [False, "invalidate", "revoke"], "chamada em processo (MCP) encerra como sempre"


def test_delete_closed_by_the_rust_records_the_closed_name_from_its_transcript(monkeypatch, caplog):
    from app import internal_api
    monkeypatch.setattr(internal_api, "_secret", "segredo")
    killed, recorded = [], []
    monkeypatch.setattr(api_mod.registry, "kill", lambda name, closed_by_rust=False: killed.append(name) or [])
    monkeypatch.setattr(api_mod.registry, "list", lambda: pytest.fail("fechada pelo Rust: não procura na lista"))
    monkeypatch.setattr(api_mod.share_bridge, "revoke_session", lambda name: None)
    monkeypatch.setattr(api_mod.closed_names, "record", lambda jsonl, name: recorded.append((jsonl, name)))

    def request(*extra):
        return Request({"type": "http", "method": "DELETE", "path": "/api/sessions/cx", "query_string": b"",
                        "headers": [(b"x-hangar-rust-closed", b"segredo"), *extra]})
    path = "/home/u/Área de trabalho/rollout-1.jsonl"
    encoded = urllib.parse.quote(path, safe="").encode()
    asyncio.run(api_mod.kill_session("cx", request=request((b"x-hangar-closed-transcript", encoded))))
    assert recorded == [(path, "cx")]
    with caplog.at_level(logging.WARNING, logger=api_mod._log.name):
        assert asyncio.run(api_mod.kill_session("cx", request=request()))["ok"] is True
    assert recorded == [(path, "cx")] and killed == ["cx", "cx"], "sem transcript a limpeza segue"
    assert "Rust close sent no transcript" in caplog.text


def test_published_terminal_with_the_rust_owner_is_left_to_the_watcher(monkeypatch):
    from types import SimpleNamespace
    from app.adapters.codex import sessions as codex_sessions
    from app.adapters.codex.adapter import CodexAdapter
    adapter = CodexAdapter()
    closed = []

    class Client:
        async def close(self):
            closed.append(True)

    async def verified(client, meta):
        return {}
    adapter._adoptions = {"cx": ("tid", Client(), "default")}
    monkeypatch.setattr(adapter, "_verify_imported_runtime", verified)
    monkeypatch.setattr(codex_sessions, "load_prepared", lambda name, transfer: {})
    monkeypatch.setattr(codex_sessions, "publish_prepared", lambda name, transfer: {"headless": False, "thread_id": "t"})
    monkeypatch.setattr(runtime_coordinator, "rust_owns_codex_terminal", lambda: True)
    asyncio.run(adapter.publish_imported(SimpleNamespace(name="cx", id="tid")))
    assert closed == [True], "o cliente da conferência sai"
    assert "cx" not in adapter._sessions and "cx" not in adapter._adoptions


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


@pytest.mark.parametrize("route", ["input", "answer", "select", "select/submit", "model-effort"])
def test_claude_question_from_guest_or_connect_goes_to_the_rust(monkeypatch, route):
    calls = []
    _provider(monkeypatch, ct="claude")
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: None)
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args: calls.append(args) or (200, b'{"ok": true}'))
    body = b'{"text":"oi","jev_task":"t","jev_message":"m"}' if route == "input" else b'{"option": 1, "interaction_id": "perm:t1"}'
    for port in (GUEST_PORT, api_mod.CONNECT_PORT):
        with pytest.raises(api_mod._RustReply) as reply:
            asyncio.run(api_mod._codex_rust_port("ct", _request(port, f"/api/sessions/ct/{route}", body=body)))
        assert reply.value.args == (200, b'{"ok": true}')
    assert calls == [("ct", route, "POST", body)] * 2
    asyncio.run(api_mod._codex_rust_port("ct", _request(GUEST_PORT, "/api/sessions/ct/keys")))
    assert len(calls) == 2, "fora das rotas migradas o Claude não vai a esta ponte"


@pytest.mark.parametrize(("method", "route", "query", "body"), [
    ("PATCH", "queue/a", b"", b'{"text": "novo"}'),
    ("DELETE", "queue/a", b"only_waiting=true", b""),
    ("POST", "queue/reorder", b"", b'{"id": "a", "before_id": null}'),
    ("POST", "queue/a/steer", b"", b""),
])
def test_queue_panel_from_guest_or_connect_goes_to_the_rust_whole(monkeypatch, method, route, query, body):
    claude, codex = [], []
    _provider(monkeypatch, ct="claude", cx="codex")
    monkeypatch.setattr(api_mod.codex_sessions, "exists", lambda name: name == "cx")
    monkeypatch.setattr(runtime_coordinator, "rust_claude", lambda *args: claude.append(args) or (200, b'{"ok": true}'))
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: codex.append(args) or (200, b'{"ok": true}'))
    full = route + (f"?{query.decode()}" if query else "")
    for port in (GUEST_PORT, api_mod.CONNECT_PORT):
        for name in ("ct", "cx"):
            with pytest.raises(api_mod._RustReply) as reply:
                asyncio.run(api_mod._codex_rust_port(name, _request(port, f"/api/sessions/{name}/{route}", query=query, body=body, method=method)))
            assert reply.value.args == (200, b'{"ok": true}')
    # Método, query e corpo chegam como vieram, por uma ponte só para cada provedor.
    assert claude == [("ct", full, method, body)] * 2
    assert codex == [("cx", full, method, body)] * 2


def test_claude_bridge_down_is_a_visible_503_without_local_answer(monkeypatch):
    import json
    import socket
    _owner(monkeypatch)
    _provider(monkeypatch, ct="claude")
    monkeypatch.setattr(runtime_coordinator, "rust_codex_terminal", lambda *args: None)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        closed = probe.getsockname()[1]
    for endpoint in ((f"127.0.0.1:{closed}", "s"), None):
        monkeypatch.setattr(list_bridge, "endpoint", lambda endpoint=endpoint: endpoint)
        with pytest.raises(api_mod._RustReply) as reply:
            asyncio.run(api_mod._codex_rust_port("ct", _request(GUEST_PORT, "/api/sessions/ct/select", body=b'{"option": 1}')))
        assert reply.value.args[0] == 503
        assert json.loads(reply.value.args[1])["detail"]["code"] == "claude_bridge_failed"


@pytest.mark.parametrize("route", ["answer", "select", "select/submit"])
def test_guest_question_to_pi_does_not_wait_for_the_claude_bridge(monkeypatch, route):
    import json
    _owner(monkeypatch, mode="pending")
    _provider(monkeypatch, pi="pi", ct="claude")
    monkeypatch.setattr(list_bridge, "endpoint", lambda: None)
    monkeypatch.setattr(api_mod.codex_sessions, "exists", lambda name: False)
    # Pi pela 8766 com a ponte ainda sem endereço: a rota Python segue.
    asyncio.run(api_mod._codex_rust_port("pi", _request(GUEST_PORT, f"/api/sessions/pi/{route}", body=b'{"option": 1}')))
    with pytest.raises(api_mod._RustReply) as reply:
        asyncio.run(api_mod._codex_rust_port("ct", _request(GUEST_PORT, f"/api/sessions/ct/{route}", body=b'{"option": 1}')))
    assert reply.value.args[0] == 503 and json.loads(reply.value.args[1])["detail"]["code"] == "claude_bridge_failed"


def test_question_bodies_keep_the_interaction_id():
    assert api_mod.SelectBody(option=1, interaction_id="perm:t1").interaction_id == "perm:t1"
    assert api_mod.AnswerBody(answers=[], interaction_id="ask:1").interaction_id == "ask:1"
    assert api_mod.SkipQuestionBody(request_id="r", interaction_id="ask:2").interaction_id == "ask:2"
