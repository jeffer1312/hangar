"""O Rust é o único que grava `.hangar-pair`: o que chega às portas do Python segue pela ponte
`/__hangar_server/groups`, num sentido; no modo `python` as rotas de grupo recusam com código."""
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


def _sidecar(name, peers, gid="g1", orq=False):
    """Sidecar como o Rust o grava."""
    body = {"peers": peers, "task": "", "gid": gid, "harness": {}, **({"orq": True} if orq else {})}
    (pair._pair_dir() / f"{name}.json").write_text(json.dumps(body), encoding="utf-8")


def test_ownership_follows_mode(monkeypatch):
    for mode, owned in (("rust", True), ("pending", True), ("python", False)):
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


def test_call_after_pending_falls_to_python_never_touches_the_bridge(monkeypatch):
    groups_bridge.configure("127.0.0.1:9", "s")
    owner = _pending_owner("python")
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    try:
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.leave", name="a")
        assert e.value.code == "groups_runtime_python"
    finally:
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


def _close_stubs(monkeypatch):
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)


@pytest.mark.parametrize("content", ["{torto", '{"peers": 5}', '{"peers": 0}', '{"peers": null}'])
def test_close_with_an_unreadable_sidecar_still_leaves_and_journals(monkeypatch, client, content):
    """Sidecar que existe e não se lê não é "sem grupo": a saída vai pela ponte e a falha vai ao diário."""
    (pair._pair_dir() / "a.json").write_text(content, encoding="utf-8")
    calls = _rust(monkeypatch, reply={"ex_peers": [], "warnings": []})
    _close_stubs(monkeypatch)
    journal = []
    monkeypatch.setattr(api.diag, "registrar", lambda evento, nivel="ok", **c: journal.append((evento, nivel, c)))
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    assert calls == [("group.leave", {"name": "a", "phases": {}})]
    assert ("grupos.sidecar_ilegivel", "erro", {"sessao": "a"}) in journal


def test_close_of_a_session_without_group_never_asks_the_bridge(monkeypatch, client):
    """Sem sidecar não há grupo a deixar: ponte fora do ar não vira aviso falso no fechamento."""
    calls = _rust(monkeypatch, error="groups_bridge_off")
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)
    r = client.delete("/api/sessions/solo", headers=H)
    assert (r.status_code, r.json()["warning"], calls) == (200, None, [])


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


def test_task_suggestion_reads_only_sessions_with_a_conversation(monkeypatch, client):
    """BRG-33: sess\u00e3o sem conversa ou de outro servidor (sem linha aqui) fica de fora; nenhuma \u2192 422 sem o LLM."""
    from app.models import SessionInfo
    infos = {"a": SessionInfo(name="a", jsonl="/t/a.jsonl"), "b": SessionInfo(name="b", jsonl="/t/b.jsonl")}

    async def cached(name):
        return infos.get(name)
    monkeypatch.setattr(api, "_cached_info", cached)
    monkeypatch.setattr(api, "_conversa_para_resumo", lambda name, info: "fim da conversa" if name == "a" else "")
    asked = []
    monkeypatch.setattr(api.narrar, "sugerir_tarefa_grupo", lambda conversas: asked.append(conversas) or "T-1")
    r = client.post("/api/pair/task-suggestion", json={"sessions": ["a", "b", "lab::x", "a"]}, headers=H)
    assert (r.status_code, r.json()) == (200, {"task": "T-1"})
    assert asked == [{"a": "fim da conversa"}]
    r = client.post("/api/pair/task-suggestion", json={"sessions": ["b", "lab::x"]}, headers=H)
    assert (r.status_code, r.json()["detail"]["code"]) == (422, "erro_grupo_sem_conversa")
    assert len(asked) == 1


def test_close_that_skipped_the_rust_leaves_with_the_orq_facts(monkeypatch, client):
    """Connect (8768) e convidado não passam pelo `delete` do Rust: a saída vai pela ponte, com a fase."""
    from app import internal_api
    from app.adapters.orq import runs
    internal_api.set_secret("s3gredo")
    monkeypatch.setattr(runs, "group_phase", lambda gid: "live")
    _sidecar("a", ["b"], orq=True)
    not_notified = {"sessao": "srv::x", "erro": {"code": "erro_peer_nao_avisado", "params": {}, "msg": "srv inacessível"}}
    calls = _rust(monkeypatch, reply={"ex_peers": ["srv::x"], "warnings": [not_notified]})
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)
    try:
        r = client.delete("/api/sessions/a", headers=H)
        assert r.status_code == 200, r.text
        assert calls == [("group.leave", {"name": "a", "phases": {"g1": "live"}})]
        assert r.json()["warning"]["params"]["avisos"] == [not_notified]
        # Repassado pelo Rust, quem sai do grupo é ele: nada pela ponte.
        calls.clear()
        r = client.delete("/api/sessions/a", headers={**H, "x-hangar-rust-leaves": "s3gredo"})
        assert (r.status_code, calls) == (200, [])
    finally:
        internal_api.set_secret(None)


def test_close_delivers_what_the_rust_left_for_python(monkeypatch, client):
    """A saída pela ponte devolve o aviso a sessão que só o Python atende (Pi/omp/Kimi): ele entrega
    depois que a ponte respondeu, e a falha vira aviso do fechamento."""
    _sidecar("a", ["b"])
    left = [{"name": "b", "text": "saiu", "steer": False}, {"name": "c", "text": "saiu", "steer": False}]
    _rust(monkeypatch, reply={"ex_peers": ["b"], "warnings": [], "deliveries": left})
    sent = []

    async def enviar(name, text):
        sent.append((name, text))
        return {"ok": name == "b", "error": {"code": "erro_x", "params": {}, "msg": "x"}}
    monkeypatch.setattr(api, "_enviar", enviar)
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    assert sent == [("b", "saiu"), ("c", "saiu")]
    assert [w["sessao"] for w in r.json()["warning"]["params"]["avisos"]] == ["c"]


def test_pair_route_forwards_status_and_body(monkeypatch, client):
    calls = _rust(monkeypatch, reply={"status": 409, "body": {"detail": {"code": "erro_pareamento_tarefa_existente"}}})
    r = client.post("/api/sessions/a/pair", json={"peer": "b", "task": "y"}, headers=H)
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_pareamento_tarefa_existente"
    assert calls == [("group.route", {"method": "POST", "name": "a", "route": "pair", "body": {
        "peer": "b", "peers": [], "task": "y", "replace_task": False, "notify_members": True, "orq": False},
        "facts": {"orchestrators": [], "phases": {}}})]


GROUP_ROUTES = [
    ("post", "/api/sessions/a/pair", "pair", {}),
    ("delete", "/api/sessions/a/pair", "pair", None),
    ("post", "/api/sessions/a/group-message", "group-message", {"text": "oi"}),
    ("get", "/api/sessions/a/pair/contract", "contract", None),
    ("put", "/api/sessions/a/pair/contract", "contract", {"content": "x", "mtime": None}),
    ("post", "/api/sessions/a/pair-remote", "pair-remote", {"initiator": "srv::x"}),
    ("post", "/api/sessions/a/unpair-remote", "unpair-remote", {"peer": "srv::x"}),
]


@pytest.mark.parametrize("method,path,route,body", GROUP_ROUTES)
def test_connect_group_route_is_one_way(monkeypatch, client, method, path, route, body):
    """Pela 8768 a rota só repassa: status e corpo do Rust voltam como vieram, sem lógica aqui."""
    calls = _rust(monkeypatch, reply={"status": 418, "body": {"de": "rust"}})
    kw = {"json": body} if body is not None else {}
    r = getattr(client, method)(path, headers=H, **kw)
    assert (r.status_code, r.json()) == (418, {"de": "rust"})
    assert [(op, a["method"], a["route"]) for op, a in calls] == [("group.route", method.upper(), route)]
    assert not (pair._pair_dir() / "a.json").exists()


@pytest.mark.parametrize("mode", ["python", None])
@pytest.mark.parametrize("method,path,route,body", GROUP_ROUTES)
def test_python_mode_group_routes_refuse(monkeypatch, client, mode, method, path, route, body):
    monkeypatch.setattr(runtime_coordinator, "_current", mode and SimpleNamespace(mode=mode))
    monkeypatch.setattr(groups_bridge, "call", lambda *a, **k: pytest.fail("sem Rust não há ponte"))
    kw = {"json": body} if body is not None else {}
    r = getattr(client, method)(path, headers=H, **kw)
    assert r.status_code == 503, r.text
    assert r.json()["detail"]["code"] == "groups_need_rust_server"
    assert list(pair._pair_dir().iterdir()) == []


def test_route_bridge_failure_is_coded(monkeypatch, client):
    _rust(monkeypatch, error="groups_runtime_starting")
    r = client.delete("/api/sessions/a/pair", headers=H)
    assert r.status_code == 503
    assert r.json()["detail"]["code"] == "erro_grupo_indisponivel"
    assert r.json()["detail"]["params"]["detalhe"] == "groups_runtime_starting"


def test_bridge_failure_is_an_explicit_error():
    exc = groups_bridge.GroupsBridgeError("groups_bridge_off")
    handler = api.app.exception_handlers[type(exc)]
    r = asyncio.run(handler(SimpleNamespace(method="POST", url=SimpleNamespace(path="/x")), exc))
    body = json.loads(r.body)
    assert r.status_code == 503 and body["detail"]["code"] == "erro_grupo_indisponivel"
    assert body["detail"]["params"]["detalhe"] == "groups_bridge_off"


def test_associate_in_rust_mode_sends_the_orq_facts(monkeypatch, client):
    calls = _rust(monkeypatch, reply={"status": 200, "body": {"ok": True, "gid": "g1", "grouped": True}})
    monkeypatch.setattr(api.orq_context, "associate_facts", lambda name, gid: {"draft_gid": name + gid})
    r = client.post("/api/sessions/arb/orq/grupo", headers=H, json={"gid": "g1", "mtime": 1.5})
    assert r.status_code == 200, r.text
    assert r.json() == {"ok": True, "gid": "g1", "grouped": True}
    assert calls == [("group.orq_associate", {"name": "arb", "gid": "g1", "mtime": 1.5, "facts": {"draft_gid": "arbg1"}})]


@pytest.mark.parametrize("mode", ["python", None])
def test_orq_grupo_in_python_mode_refuses(monkeypatch, client, mode):
    monkeypatch.setattr(runtime_coordinator, "_current", mode and SimpleNamespace(mode=mode))
    monkeypatch.setattr(groups_bridge, "call", lambda *a, **k: pytest.fail("sem Rust não há ponte"))
    _sidecar("arb", [], gid="g1", orq=True)
    before = sorted(p.name for p in pair._pair_dir().iterdir())
    r = client.post("/api/sessions/planner/orq/grupo", headers=H, json={"gid": "g1", "mtime": 1.5})
    assert r.status_code == 503, r.text
    assert r.json()["detail"]["code"] == "groups_need_rust_server"
    assert sorted(p.name for p in pair._pair_dir().iterdir()) == before


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
    assert rust_server.RUST_SERVER_PROTOCOL == 90


@pytest.mark.parametrize("mode", ["python", None])
def test_python_mode_close_of_a_grouped_session_closes_and_warns(monkeypatch, client, mode):
    """Fechar nunca é barrado; sem o Rust a saída do grupo não acontece e isso volta no aviso e no diário."""
    monkeypatch.setattr(runtime_coordinator, "_current", mode and SimpleNamespace(mode=mode))
    monkeypatch.setattr(groups_bridge, "call", lambda *a, **k: pytest.fail("sem Rust não há ponte"))
    _sidecar("a", ["b"])
    killed, recorded = [], []
    monkeypatch.setattr(api, "_recusa_orq", lambda n: None)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: killed.append(n) or [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api, "_jsonl_of_session", lambda n: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)
    monkeypatch.setattr(api.diag, "registrar", lambda evento, nivel="ok", **f: recorded.append((nivel, f.get("codigo"))))
    r = client.delete("/api/sessions/a", headers=H)
    assert r.status_code == 200, r.text
    assert killed == ["a"]
    assert [x["erro"]["code"] for x in r.json()["warning"]["params"]["avisos"]] == ["groups_need_rust_server"]
    assert ("erro", "groups_need_rust_server") in recorded
    # Fora de grupo: sem aviso.
    r = client.delete("/api/sessions/c", headers=H)
    assert (r.status_code, r.json()["warning"]) == (200, None)
