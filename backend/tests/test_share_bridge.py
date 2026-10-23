"""Depois da virada o Rust é o único dono de `shares.json` e `external_pairs.json`: o Python pede
pela ponte `/__hangar_server/share` o que só ele vê acontecer, e nunca grava os arquivos. Falha da
ponte não some calada: fechar e renomear seguem, a falha vai ao diário e volta como aviso."""
import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import api, diag, list_bridge, share_bridge
from app.config import settings

H = {"Authorization": "Bearer secret"}


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "secret")
    return TestClient(api.app)


@pytest.fixture
def events(monkeypatch):
    seen = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **campos: seen.append((evento, nivel, campos)))
    return seen


@pytest.fixture
def bridge(monkeypatch):
    """A ponte trocada por uma lista de chamadas; `reply` responde por op."""
    calls, replies = [], {}

    def call(op, **args):
        calls.append((op, args))
        reply = replies.get(op, {})
        if isinstance(reply, share_bridge.ShareBridgeError):
            share_bridge._failed(op, reply.code)
            raise reply
        return reply
    monkeypatch.setattr(share_bridge, "call", call)
    return SimpleNamespace(calls=calls, replies=replies)


class _Rust(BaseHTTPRequestHandler):
    seen: list = []
    reply: dict = {}

    def do_POST(self):
        body = self.rfile.read(int(self.headers["content-length"]))
        type(self).seen.append((self.path, self.headers["x-hangar-internal"], json.loads(body)))
        out = json.dumps(type(self).reply).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *a):
        pass


def test_call_posts_the_op_to_the_private_route(monkeypatch, events):
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Rust)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Rust.seen, _Rust.reply = [], {"ok": True, "result": {"revoked": 2}}
    list_bridge.configure(f"127.0.0.1:{server.server_port}", "s3gredo")
    try:
        assert share_bridge.call("revoke_session", name="a") == {"revoked": 2}
        _Rust.reply = {"ok": False, "error": {"code": "erro_par_inexistente", "detail": "este par externo não existe"}}
        with pytest.raises(share_bridge.ShareBridgeError) as e:
            share_bridge.call("send", sender="a", target="ana::b", text="oi")
        assert (e.value.code, e.value.detail) == ("erro_par_inexistente", "este par externo não existe")
    finally:
        list_bridge.configure(None, None)
        server.shutdown()
    assert _Rust.seen == [("/__hangar_server/share", "s3gredo", {"op": "revoke_session", "args": {"name": "a"}}),
                          ("/__hangar_server/share", "s3gredo",
                           {"op": "send", "args": {"sender": "a", "target": "ana::b", "text": "oi"}})]
    with pytest.raises(share_bridge.ShareBridgeError) as e:
        share_bridge.call("active_sessions")
    assert e.value.code == "share_bridge_off"
    assert ("convites.ponte", "aviso", {"operacao_rust": "active_sessions", "motivo": "share_bridge_off"}) in events


def test_call_refuses_to_block_the_event_loop():
    async def on_loop():
        with pytest.raises(share_bridge.ShareBridgeError) as e:
            share_bridge.call("active_sessions")
        return e.value.code
    assert asyncio.run(on_loop()) == "share_wait_on_loop"


def _kill_stubs(monkeypatch):
    # Saída do grupo pela ponte tem teste próprio (`test_groups_bridge.py`); aqui só os convites.
    monkeypatch.setattr(api, "_rust_leave", lambda name, phases: [])
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api, "_jsonl_of_session", lambda n: None)


def test_kill_session_asks_rust_to_revoke_every_invite(monkeypatch, client, bridge):
    _kill_stubs(monkeypatch)
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    assert r.json() == {"ok": True, "warning": None}
    assert bridge.calls == [("revoke_session", {"name": "a"})]


def test_kill_session_with_rust_down_still_closes_and_warns(monkeypatch, client, bridge, events):
    _kill_stubs(monkeypatch)
    bridge.replies["revoke_session"] = share_bridge.ShareBridgeError("share_bridge_unavailable", "URLError")
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    warning = r.json()["warning"]
    assert warning["code"] == "erro_convite_nao_acompanhou"
    assert warning["params"]["codigo"] == "share_bridge_unavailable"
    assert ("convites.ponte", "aviso", {"operacao_rust": "revoke_session", "motivo": "share_bridge_unavailable"}) in events


def test_kill_session_without_rust_leaves_the_invite_to_its_sweep(monkeypatch, client, bridge, events):
    _kill_stubs(monkeypatch)
    bridge.replies["revoke_session"] = share_bridge.ShareBridgeError("share_bridge_off")
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200 and r.json()["warning"] is None
    assert ("convites.ponte", "aviso", {"operacao_rust": "revoke_session", "motivo": "share_bridge_off"}) in events


def test_mode_change_holds_the_invite_and_moves_its_life(monkeypatch, bridge):
    monkeypatch.setattr(share_bridge, "changing_mode", set())
    monkeypatch.setattr(api, "session_life", lambda name: "k:novo")
    monkeypatch.setattr(api.guest_users, "set_life", lambda *a: None)
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    seen = []

    async def troca():
        seen.append(("durante", "s" in share_bridge.changing_mode, list(bridge.calls)))
        return "feito"
    assert asyncio.run(api._durante_troca("s", troca())) == "feito"
    assert seen == [("durante", True, [("mode_change_begin", {"name": "s"})])]
    assert bridge.calls == [("mode_change_begin", {"name": "s"}), ("set_life", {"name": "s", "life": "k:novo"}),
                            ("mode_change_end", {"name": "s", "source_life": None})]
    assert "s" not in share_bridge.changing_mode


def test_mode_change_end_carries_the_incomplete_transfer(monkeypatch, bridge):
    from app import conversation_transfer
    monkeypatch.setattr(conversation_transfer, "list_incomplete",
                        lambda: [SimpleNamespace(name="t", source_life="k:origem")])
    share_bridge.mode_change("t", False)
    share_bridge.mode_change("u", False)
    assert bridge.calls == [("mode_change_end", {"name": "t", "source_life": "k:origem"}),
                            ("mode_change_end", {"name": "u", "source_life": None})]


def test_failed_set_life_is_replayed_before_the_next_snapshot(monkeypatch, bridge):
    from app import conversation_transfer
    monkeypatch.setattr(share_bridge, "changing_mode", set())
    monkeypatch.setattr(share_bridge, "_unsent_lives", {})
    monkeypatch.setattr(conversation_transfer, "list_incomplete", lambda: [])
    bridge.replies["set_life"] = share_bridge.ShareBridgeError("share_bridge_unavailable")
    assert share_bridge.set_life("s", "k:novo") is not None
    del bridge.replies["set_life"]
    bridge.calls.clear()
    assert share_bridge.send_snapshot()
    # O Rust novo aprende a vida nova antes de a fotografia soltar o veredito.
    assert bridge.calls == [("set_life", {"name": "s", "life": "k:novo"}),
                            ("facts_snapshot", {"lives": {}, "changing": []})]
    bridge.calls.clear()
    assert share_bridge.send_snapshot()
    assert bridge.calls == [("facts_snapshot", {"lives": {}, "changing": []})]


def test_snapshot_is_retried_until_rust_takes_it(monkeypatch):
    results = [False, False, True]
    sleeps = []
    monkeypatch.setattr(share_bridge, "send_snapshot", lambda: results.pop(0))
    monkeypatch.setattr(list_bridge, "endpoint", lambda: ("127.0.0.1:9", "s"))

    async def no_wait(delay):
        sleeps.append(delay)
    monkeypatch.setattr(share_bridge.asyncio, "sleep", no_wait)
    asyncio.run(share_bridge.send_snapshot_until_sent(lambda: False))
    assert results == [] and sleeps == [1.0, 2.0, 4.0]
    # Parando o backend, ou o Rust caiu (o próximo manda a sua), a repetição acaba.
    results[:] = [False] * 5
    asyncio.run(share_bridge.send_snapshot_until_sent(lambda: True))
    monkeypatch.setattr(list_bridge, "endpoint", lambda: None)
    asyncio.run(share_bridge.send_snapshot_until_sent(lambda: False))
    assert len(results) == 5


def test_snapshot_carries_transfers_and_mode_changes(monkeypatch, bridge):
    from app import conversation_transfer
    monkeypatch.setattr(share_bridge, "changing_mode", {"m"})
    monkeypatch.setattr(conversation_transfer, "list_incomplete",
                        lambda: [SimpleNamespace(name="t", source_life="k:origem")])
    monkeypatch.setattr(share_bridge, "_unsent_lives", {})
    assert share_bridge.send_snapshot()
    assert bridge.calls == [("facts_snapshot", {"lives": {"t": "k:origem"}, "changing": ["m"]})]


def test_snapshot_is_not_sent_when_transfers_are_unreadable(monkeypatch, bridge, events):
    from app import conversation_transfer

    def boom():
        raise OSError(13, "negado")
    monkeypatch.setattr(conversation_transfer, "list_incomplete", boom)
    assert not share_bridge.send_snapshot()
    # Sem fotografia o Rust segue no "não sei": nunca 410 por uma transferência que ele não viu.
    assert bridge.calls == []
    assert [e for e in events if e[0] == "convites.fotografia_falhou"]


def test_supervisor_sends_the_snapshot_when_bridges_come_up(monkeypatch):
    from app import internal_api, rust_server
    sent = []
    monkeypatch.setattr(share_bridge, "send_snapshot", lambda: sent.append(list_bridge.endpoint()) or len(sent) > 1)

    async def no_wait(delay):
        pass
    monkeypatch.setattr(share_bridge.asyncio, "sleep", no_wait)
    supervisor = rust_server.Supervisor(__import__("pathlib").Path("/bin/true"), "127.0.0.1", 1, 2, "t",
                                        "127.0.0.1", lambda: False)
    monkeypatch.setattr(rust_server, "_spawn", lambda binary, env: SimpleNamespace(poll=lambda: None, pid=1))
    monkeypatch.setattr(rust_server, "_runtime_ready", lambda proc, instance: {
        "type": "runtime_ready", "protocol": rust_server.RUST_SERVER_PROTOCOL, "instance": instance, "port": 1})
    monkeypatch.setattr(rust_server, "_health", lambda host, port: {
        "protocol": rust_server.RUST_SERVER_PROTOCOL, "terminal_address": "127.0.0.1:9", "terminal_panel": True,
        "owns": [{"provider": "claude", "headless": True}]})
    monkeypatch.setattr(supervisor, "configure_runtime", lambda *a: None)

    async def start_and_retry():
        state = await supervisor._start()
        await supervisor._snapshot_retry
        return state
    try:
        assert asyncio.run(start_and_retry()) == "up"
    finally:
        internal_api.set_secret(None)
        list_bridge.configure(None, None)
    assert sent[0][0] == "127.0.0.1:9", "a fotografia sai com as pontes já ligadas"
    assert len(sent) == 2, "a primeira falhou: repete até o Rust aceitar"


def test_connect_owner_invite_goes_to_the_explicit_private_route(monkeypatch, client):
    from app import voicesock
    seen = []

    async def forward(request, target, **kw):
        seen.append((request.method, target))
        from fastapi.responses import JSONResponse
        return JSONResponse({"id": "x"})
    monkeypatch.setattr(voicesock, "forward_settings", forward)
    r = client.post("/api/sessions/a b/share?local=true", headers=H)
    assert r.status_code == 200 and r.json() == {"id": "x"}
    assert seen == [("POST", "/__hangar_server/share/owner/create/a%20b?local=true")]
    assert client.post("/api/sessions/a/share").status_code == 401, "só o dono"


def test_connect_pair_access_and_undo_go_to_their_private_routes(monkeypatch, client):
    from app import voicesock
    from fastapi.responses import JSONResponse
    seen = []

    async def forward(request, target, **kw):
        seen.append((request.method, target))
        return JSONResponse({"ok": True})
    monkeypatch.setattr(voicesock, "forward_settings", forward)
    assert client.post("/api/sessions/a b/pair-access", json={"alias": "ana", "level": "full"}, headers=H).json() == {"ok": True}
    assert client.delete("/api/external-pairs/ana?session=han gar", headers=H).json() == {"ok": True}
    assert seen == [("POST", "/__hangar_server/share/owner/pair_access/a%20b"),
                    ("DELETE", "/__hangar_server/share/owner/pairs_undo/ana?session=han%20gar")]
    assert client.post("/api/sessions/a/pair-access", json={}).status_code == 401, "só o dono"
    assert client.delete("/api/external-pairs/ana?session=x").status_code == 401


def test_connect_owner_invite_without_rust_is_404(client):
    list_bridge.configure(None, None)
    r = client.post("/api/sessions/a/share", headers=H)
    assert r.status_code == 404
    assert r.json()["detail"]["code"] == "share_needs_rust_server"


def test_python_has_no_share_or_pair_writer_routes(client):
    from app import internal_api
    for path in ("/api/guest/redeem", "/api/guest/attach", "/api/pair/redeem", "/api/pair/message"):
        assert client.post(path, json={}, headers=H).status_code in (404, 405), path
    assert client.delete("/api/pair", headers=H).status_code in (404, 405)
    assert "/external-pairs/end" not in [getattr(r, "path", "") for r in internal_api.router.routes]
