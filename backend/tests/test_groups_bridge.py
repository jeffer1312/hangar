"""No modo `rust`/`pending` o Rust é o único que grava `.hangar-pair`: o Python pede pela ponte
`/__hangar_server/groups`, e as primitivas de escrita do `pair.py` recusam alto."""
import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import api, external_pair_api, groups_bridge, pair, registry, runtime_coordinator
from app.config import settings
from app.models import SessionInfo

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


def _rust(monkeypatch, reply=None, error=None):
    """Modo Rust com a ponte trocada por uma lista de chamadas."""
    calls = []
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)

    def call(op, **args):
        calls.append((op, args))
        if error is not None:
            raise groups_bridge.GroupsBridgeError(error)
        return reply(op, args) if callable(reply) else reply
    monkeypatch.setattr(groups_bridge, "call", call)
    return calls


def test_write_primitives_refuse_in_rust_mode(monkeypatch, tmp_path):
    pair.PairLink("a").set(["b"], "", "g1", {})
    (pair._pair_dir() / "grupo-g2.md").write_text("perdedor", encoding="utf-8")
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    with pytest.raises(pair.GroupsOwnedByRust):
        pair.PairLink("a").set(["b"], "", "g1", {})
    with pytest.raises(pair.GroupsOwnedByRust):
        pair.PairLink("a").clear()
    with pytest.raises(pair.GroupsOwnedByRust):
        pair._merge_contract("g2", "g1")
    with pytest.raises(pair.GroupsOwnedByRust):
        pair._arquivar_contratos("g1")
    # A leitura continua do Python.
    assert pair.PairLink("a").get()["peers"] == ["b"]
    assert (pair._pair_dir() / "grupo-g2.md").exists()


def test_ownership_follows_mode_and_health(monkeypatch):
    monkeypatch.setattr(groups_bridge, "_capable", True)
    for mode, owned in (("rust", True), ("pending", True), ("python", False)):
        monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode=mode))
        assert groups_bridge.rust_owns_groups() is owned, mode
    monkeypatch.setattr(groups_bridge, "_capable", False)
    assert groups_bridge.rust_owns_groups() is False, "Rust sem grupos na saúde: o Python segue dono"
    monkeypatch.setattr(runtime_coordinator, "_current", None)
    monkeypatch.setattr(groups_bridge, "_capable", True)
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


def test_call_in_python_mode_refuses(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="python"))
    with pytest.raises(groups_bridge.GroupsBridgeError) as e:
        groups_bridge.call("group.leave", name="a")
    assert e.value.code == "groups_runtime_python"


def test_kill_in_rust_mode_asks_rust(monkeypatch):
    calls = _rust(monkeypatch, reply={"ex_peers": ["b"], "warnings": []})
    registry.SessionRegistry._clear_pair("a")
    assert calls == [("group.leave", {"name": "a"})]


def test_kill_in_rust_mode_reports_bridge_failure(monkeypatch, caplog):
    _rust(monkeypatch, error="groups_bridge_unavailable")
    # A sessão viva sai da lista e a varredura do Rust não a vê: o aviso precisa chegar ao kill.
    [warning] = registry.SessionRegistry._clear_pair("a")
    assert warning["sessao"] == "a" and warning["erro"]["code"] == "erro_grupo_indisponivel"
    assert warning["erro"]["params"] == {"detalhe": "groups_bridge_unavailable"}
    assert "groups_bridge_unavailable" in caplog.text


def test_create_cleanup_logs_how_many_leave_warnings(monkeypatch, caplog):
    _rust(monkeypatch, reply={"ex_peers": ["srv::x"], "warnings": [_NOT_NOTIFIED]})
    monkeypatch.setattr(registry.tmux, "has_session", lambda name: False)
    for mod in (registry.headless_sessions, registry.codex_sessions):
        monkeypatch.setattr(mod, "exists", lambda name: False)
    registry.SessionRegistry.__new__(registry.SessionRegistry)._leave_old_group("a")
    # Só a contagem: o aviso pode trazer texto da outra máquina.
    assert "1 aviso" in caplog.text and "srv inacessível" not in caplog.text


def test_create_refuses_when_group_cleanup_fails(monkeypatch, tmp_path):
    pair.PairLink("a").set(["b"], "", "g1", {})
    calls = _rust(monkeypatch, error="groups_bridge_unavailable")
    for mod in (registry.headless_sessions, registry.codex_sessions):
        monkeypatch.setattr(mod, "exists", lambda name: False)
    monkeypatch.setattr(registry.tmux, "has_session", lambda name: False)
    monkeypatch.setattr("app.conversation_transfer.require_available", lambda name: None)
    with pytest.raises(ValueError) as e:
        registry.SessionRegistry.__new__(registry.SessionRegistry).create(
            "a", str(tmp_path), headless=True, permission_mode="default")
    assert getattr(e.value, "code", None) == "erro_grupo_limpeza_falhou"
    assert calls == [("group.leave", {"name": "a"})]


def test_create_cleanup_skips_live_name_and_tolerates_missing_sidecar(monkeypatch):
    calls = _rust(monkeypatch, error="groups_bridge_unavailable")
    monkeypatch.setattr(registry.tmux, "has_session", lambda name: name == "viva")
    for mod in (registry.headless_sessions, registry.codex_sessions):
        monkeypatch.setattr(mod, "exists", lambda name: False)
    reg = registry.SessionRegistry.__new__(registry.SessionRegistry)
    reg._leave_old_group("viva")             # nome em uso: quem recusa é o ramo da criação
    reg._leave_old_group("solta")            # sem sidecar: nada a herdar, a falha só vai ao log
    assert calls == [("group.leave", {"name": "solta"})]


def test_create_error_code_reaches_the_api():
    e = registry.GroupCleanupFailed("a")
    assert isinstance(e, ValueError) and e.code == "erro_grupo_limpeza_falhou"


def test_pair_route_forwards_status_and_body(monkeypatch, client):
    calls = _rust(monkeypatch, reply={"status": 409, "body": {"detail": {"code": "erro_pareamento_tarefa_existente"}}})
    r = client.post("/api/sessions/a/pair", json={"peer": "b", "task": "y"}, headers=H)
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_pareamento_tarefa_existente"
    assert calls == [("group.route", {"method": "POST", "name": "a", "route": "pair", "body": {
        "peer": "b", "peers": [], "task": "y", "replace_task": False, "notify_members": True, "orq": False}})]


@pytest.mark.parametrize("method,path,route,body", [
    ("delete", "/api/sessions/a/pair", "pair", None),
    ("post", "/api/sessions/a/group-message", "group-message", {"text": "oi"}),
    ("get", "/api/sessions/a/pair/contract", "contract", None),
    ("post", "/api/sessions/a/pair-remote", "pair-remote", {"initiator": "srv::x"}),
    ("post", "/api/sessions/a/unpair-remote", "unpair-remote", {"peer": "srv::x"}),
])
def test_every_group_route_forwards(monkeypatch, client, method, path, route, body):
    calls = _rust(monkeypatch, reply={"status": 200, "body": {"ok": True, "de": "rust"}})
    kw = {"json": body} if body is not None else {}
    r = getattr(client, method)(path, headers=H, **kw)
    assert (r.status_code, r.json()) == (200, {"ok": True, "de": "rust"})
    assert [(op, a["method"], a["route"]) for op, a in calls] == [("group.route", method.upper(), route)]
    assert not (pair._pair_dir() / "a.json").exists()


def test_route_bridge_failure_is_coded(monkeypatch, client):
    _rust(monkeypatch, error="groups_runtime_starting")
    r = client.delete("/api/sessions/a/pair", headers=H)
    assert r.status_code == 503
    assert r.json()["detail"]["code"] == "erro_grupo_indisponivel"
    assert r.json()["detail"]["params"]["detalhe"] == "groups_runtime_starting"


def test_mcp_path_forwards_in_process(monkeypatch):
    calls = _rust(monkeypatch, reply={"status": 200, "body": {"ok": True, "warning": None}})
    assert asyncio.run(api.unpair_session("a")) == {"ok": True, "warning": None}
    assert calls[0][0] == "group.route"


def test_forgotten_writer_is_an_explicit_error():
    for exc in (pair.GroupsOwnedByRust("a"), groups_bridge.GroupsBridgeError("groups_bridge_off")):
        handler = api.app.exception_handlers[type(exc)]
        r = asyncio.run(handler(SimpleNamespace(method="POST", url=SimpleNamespace(path="/x")), exc))
        body = json.loads(r.body)
        assert r.status_code == 503 and body["detail"]["code"] == "erro_grupo_indisponivel"


def test_sweep_loop_does_not_run_in_rust_mode(monkeypatch):
    swept = []
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    monkeypatch.setattr(api.registry, "sweep_pairs", lambda *a: swept.append(a))

    async def stop(_s):
        raise asyncio.CancelledError
    monkeypatch.setattr(api.asyncio, "sleep", stop)
    with pytest.raises(asyncio.CancelledError):
        asyncio.run(api._pair_sweep_loop())
    assert swept == []


_NOT_NOTIFIED = {"sessao": "srv::x", "erro": {"code": "erro_peer_nao_avisado", "params": {"peer": "srv::x"},
                                               "msg": "srv inacessível"}}


def _headless_kill_stubs(monkeypatch):
    from app import adapters
    monkeypatch.setattr("app.conversation_transfer.require_available", lambda n: None)
    monkeypatch.setattr(registry.headless_sessions, "exists", lambda n: True)
    monkeypatch.setattr(registry.headless_sessions, "load", lambda n: {})
    monkeypatch.setattr(registry.headless_sessions, "delete", lambda n: None)
    monkeypatch.setattr(adapters, "get_adapter", lambda kind: SimpleNamespace(close_sync=lambda n, m: None))
    monkeypatch.setattr(registry.shortcut_terminals, "close_all", lambda n: None)
    monkeypatch.setattr(registry.SessionRegistry, "_forget", lambda self, n: None)
    monkeypatch.setattr(registry, "PromptQueue", lambda n: SimpleNamespace(clear=lambda: None))
    monkeypatch.setattr(registry, "ThenLink", lambda n: SimpleNamespace(clear=lambda: None))


def test_registry_kill_passes_the_rust_leave_warnings_up(monkeypatch):
    _headless_kill_stubs(monkeypatch)
    calls = _rust(monkeypatch, reply={"ex_peers": ["srv::x"], "warnings": [_NOT_NOTIFIED]})
    assert registry.SessionRegistry().kill("a") == [_NOT_NOTIFIED]
    assert calls == [("group.leave", {"name": "a"})]


def test_kill_session_in_rust_mode_shows_the_leave_warning_once(monkeypatch, client):
    pair.PairLink("a").set(["srv::x"], "", "g1", {})
    _rust(monkeypatch, reply={"ex_peers": ["srv::x"], "warnings": [_NOT_NOTIFIED]})
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n: registry.SessionRegistry._clear_pair(n))
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_store, "revoke_session", lambda n: False)

    async def boom(*a):
        raise AssertionError("o Rust já avisou")
    monkeypatch.setattr(api, "_avisar_saida", boom)
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    assert r.json()["warning"] == {"code": "erro_pareamento_saida_falhou", "params": {"avisos": [_NOT_NOTIFIED]},
                                   "msg": "aviso de saída falhou: srv::x: srv inacessível"}


def test_capable_from_the_start_when_the_rust_is_expected():
    """Antes da primeira saúde o Rust já atende `/pair` e varre: o Python não pode ser dono nem um instante."""
    import subprocess
    import sys
    out = subprocess.run([sys.executable, "-c", "from app import groups_bridge; print(groups_bridge._capable)"],
                         capture_output=True, text=True, check=True)
    assert out.stdout.strip() == "True"


def test_pending_before_health_does_not_write_and_waits(monkeypatch, tmp_path):
    pair.PairLink("a").set(["b"], "", "g1", {})
    pair.PairLink("b").set(["a"], "", "g1", {})
    loop = asyncio.new_event_loop()
    threading.Thread(target=loop.run_forever, daemon=True).start()
    waited = []

    async def await_mode():
        waited.append(True)
        raise runtime_coordinator.RuntimeStarting("subindo")
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="pending", loop=loop, await_mode=await_mode))
    monkeypatch.setattr(groups_bridge, "_capable", True)
    groups_bridge.configure(None, None)
    try:
        assert groups_bridge.rust_owns_groups() is True
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.leave", name="a")
        assert e.value.code == "groups_runtime_starting" and waited
        assert [w["erro"]["params"] for w in registry.SessionRegistry._clear_pair("a")] == [
            {"detalhe": "groups_runtime_starting"}]
        with pytest.raises(pair.GroupsOwnedByRust):
            pair.leave("a")
        swept = []
        monkeypatch.setattr(api.registry, "sweep_pairs", lambda *a: swept.append(a))

        async def stop(_s):
            raise asyncio.CancelledError
        monkeypatch.setattr(api.asyncio, "sleep", stop)
        with pytest.raises(asyncio.CancelledError):
            asyncio.run(api._pair_sweep_loop())
        assert swept == []
    finally:
        loop.call_soon_threadsafe(loop.stop)
    assert pair.PairLink("a").get()["peers"] == ["b"] and pair.PairLink("b").get()["peers"] == ["a"]


def test_associate_in_rust_mode_goes_through_bridge(monkeypatch, client):
    calls = _rust(monkeypatch, reply={"status": 200, "body": {"ok": True, "gid": "g1", "grouped": True}})

    def no_local(*a):
        raise AssertionError("no modo Rust o lock de grupo é do Rust")
    monkeypatch.setattr(api.orq_context, "associate", no_local)
    r = client.post("/api/sessions/arb/orq/grupo", headers=H, json={"gid": "g1", "mtime": 1.5})
    assert r.status_code == 200, r.text
    assert r.json() == {"ok": True, "gid": "g1", "grouped": True}
    assert calls == [("group.orq_associate", {"name": "arb", "gid": "g1", "mtime": 1.5})]


def test_rename_in_rust_mode_asks_rust_before_tmux(monkeypatch, client):
    order = []
    _rust(monkeypatch, reply=lambda op, a: order.append((op, a)) or {})
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api, "_headless", lambda n: False)
    monkeypatch.setattr(api, "_codex_sem_terminal", lambda n: False)
    monkeypatch.setattr(api.headless_sessions, "exists", lambda n: False)
    monkeypatch.setattr("app.tmux.has_session", lambda n: n == "a")
    monkeypatch.setattr("app.tmux.rename_session", lambda a, b: order.append(("tmux", a, b)) or False)
    r = client.post("/api/sessions/a/rename", headers=H, json={"new": "b"})
    assert r.status_code == 500
    # O grupo foi renomeado antes do tmux e desfeito quando o tmux falhou.
    assert order == [("group.rename", {"old": "a", "new": "b"}), ("tmux", "a", "b"),
                     ("group.rename", {"old": "b", "new": "a"})]


def test_rename_in_rust_mode_aborts_when_rust_fails(monkeypatch, client):
    _rust(monkeypatch, error="groups_store_failed")
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api, "_headless", lambda n: False)
    monkeypatch.setattr(api, "_codex_sem_terminal", lambda n: False)
    monkeypatch.setattr(api.headless_sessions, "exists", lambda n: False)
    monkeypatch.setattr("app.tmux.has_session", lambda n: n == "a")
    monkeypatch.setattr("app.tmux.rename_session", lambda a, b: pytest.fail("tmux não pode ser tocado"))
    r = client.post("/api/sessions/a/rename", headers=H, json={"new": "b"})
    assert r.status_code == 503
    assert r.json()["detail"]["code"] == "erro_grupo_indisponivel"


def test_registry_rename_does_not_write_groups_in_rust_mode(monkeypatch):
    pair.PairLink("a").set(["b"], "", "g1", {})
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    monkeypatch.setattr("app.conversation_transfer.require_available", lambda n: None)
    for mod in (registry.headless_sessions, registry.codex_sessions):
        monkeypatch.setattr(mod, "exists", lambda name: False)
    reg = registry.SessionRegistry()
    monkeypatch.setattr(reg, "_rename_rust", lambda *a: None)
    monkeypatch.setattr(registry, "PromptQueue", lambda n: SimpleNamespace(rename=lambda new: None))
    monkeypatch.setattr(registry, "ThenLink", lambda n: SimpleNamespace(rename=lambda new: None))
    monkeypatch.setattr(registry.shortcut_terminals, "rename_owner", lambda *a: None)
    monkeypatch.setattr("app.conversation_transfer.rename_transfer", lambda *a: None)
    monkeypatch.setattr(registry.tmux, "is_hidden", lambda *a, **k: False, raising=False)
    # O rename chega à etapa do grupo e ela não grava: quem renomeou o grupo foi o Rust.
    reached = []
    real = registry._rename_pair_python
    monkeypatch.setattr(registry, "_rename_pair_python", lambda old, new: reached.append((old, new)) or real(old, new))
    monkeypatch.setattr(registry, "rename_pair", lambda *a: pytest.fail("o rename do registry não pode gravar grupo no modo Rust"))
    reg.rename("a", "c")
    assert reached == [("a", "c")]
    assert pair.PairLink("a").get()["peers"] == ["b"]


def test_external_pair_link_and_unlink_in_rust_mode(monkeypatch):
    calls = _rust(monkeypatch, reply=lambda op, a: {"gid": "g9"} if op == "group.external_link" else {})
    undo = asyncio.run(external_pair_api._join_external("a", "casa::b", {"a": "claude", "outra": "codex"}))
    external_pair_api._restore_external(undo)
    assert calls == [("group.external_link", {"local": "a", "address": "casa::b", "harness": {"a": "claude"}}),
                     ("group.external_unlink", {"local": "a", "address": "casa::b"})]


def test_external_pair_refusal_in_rust_mode(monkeypatch):
    _rust(monkeypatch, error="erro_pareamento_mistura_cross")
    with pytest.raises(pair.PairMixError):
        asyncio.run(external_pair_api._join_external("a", "casa::b", {}))


@pytest.mark.parametrize("code,undone", [("groups_bridge_unavailable", True), ("groups_bridge_invalid", True),
                                         ("groups_runtime_starting", False)])
def test_external_link_with_uncertain_outcome_is_undone(monkeypatch, code, undone):
    """Prazo ou resposta perdida depois do pedido: o Rust pode ter gravado o par sem registro nem token."""
    def reply(op, a):
        if op == "group.external_link":
            raise groups_bridge.GroupsBridgeError(code)
        return {}
    calls = _rust(monkeypatch, reply=reply)
    with pytest.raises(groups_bridge.GroupsBridgeError) as e:
        asyncio.run(external_pair_api._join_external("a", "casa::b", {}))
    assert e.value.code == code
    unlink = ("group.external_unlink", {"local": "a", "address": "casa::b"})
    assert (unlink in calls) is undone


def test_external_teardown_in_rust_mode_unlinks(monkeypatch):
    calls = _rust(monkeypatch, reply={})
    monkeypatch.setattr(external_pair_api.external_pairs, "remove", lambda sid: None)
    monkeypatch.setattr(external_pair_api.share_store, "revoke", lambda sid: None)
    rec = SimpleNamespace(share_id="s1", local_session="a", address="casa::b")
    asyncio.run(external_pair_api.teardown(rec, notify=False))
    assert calls == [("group.external_unlink", {"local": "a", "address": "casa::b"})]


def test_child_gets_group_env(monkeypatch, tmp_path):
    from app import list_bridge, peers, rust_server
    monkeypatch.setattr(list_bridge, "dirs_env", lambda: "{}")
    monkeypatch.setattr(settings, "server_id", "casa")
    sup = rust_server.Supervisor(tmp_path / "bin", "127.0.0.1", 1, 2, "t", "127.0.0.1", lambda: False)
    try:
        env = sup._env()
    finally:
        from app import internal_api
        internal_api.set_secret(None)
    assert env["HANGAR_SERVER_ID"] == "casa"
    assert env["HANGAR_PEERS_FILE"] == str(peers._PEERS_FILE)
    assert env["HANGAR_PAIR_ARCHIVE"] == str(pair._arquivo_dir())
    assert rust_server.RUST_SERVER_PROTOCOL == 42
