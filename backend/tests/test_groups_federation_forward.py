"""Grupo entre máquinas pelas portas do Python. Na 8768 as rotas `/api/groups/*` e `GET …/group` só
repassam ao Rust (nunca um 404 sem código, que a outra máquina leria como versão antiga). E a rota de
grupo que vai pela ponte devolve ao Python somente as entregas que ele ainda atende."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, api, groups_bridge, guest_users, runtime_coordinator
from app.config import settings

TOKEN = "t-grupos"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


class _Rust(BaseHTTPRequestHandler):
    seen: list = []
    extra: dict = {}

    def _answer(self):
        length = int(self.headers.get("content-length") or 0)
        _Rust.seen.append({"method": self.command, "path": self.path, "original": self.headers.get("x-hangar-path"),
                           "body": self.rfile.read(length).decode() if length else ""})
        # Rota que o Rust não atende: o 404 com código do `forwarded::private`.
        unknown = _Rust.seen[-1]["original"] == "/api/groups/x/y"
        data = json.dumps({"code": "forwarded_route_not_owned"} if unknown else {"server_id": "casa", "de": "rust", **_Rust.extra}).encode()
        self.send_response(404 if unknown else 200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_PUT = do_POST = _answer

    def log_message(self, format, *args):
        pass


@pytest.fixture
def rust(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Rust)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Rust.seen = []
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    monkeypatch.setattr(account_bridge, "_preparation_transport", (f"127.0.0.1:{server.server_address[1]}", "segredo"))
    yield
    server.shutdown()


ROUTES = [("get", "/api/groups/g1", None), ("put", "/api/groups/g1", {"protocol": 1, "owner": "lab", "version": 2}),
          ("post", "/api/groups/g1/ops", {"op": "join"}), ("get", "/api/sessions/a/group", None)]


def test_connect_forwards_groups_routes(rust):
    client = TestClient(api.app)
    for method, path, body in ROUTES:
        kw = {"json": body} if body is not None else {}
        r = getattr(client, method)(path, headers=AUTH, **kw)
        assert (r.status_code, r.json()["de"]) == (200, "rust"), path
    assert [(s["method"], s["path"], s["original"]) for s in _Rust.seen] == [
        (m.upper(), "/__hangar_server/forwarded", p) for m, p, _ in ROUTES]
    # O corpo do PUT e do POST chega inteiro ao Rust.
    assert json.loads(_Rust.seen[1]["body"]) == ROUTES[1][2] and json.loads(_Rust.seen[2]["body"]) == ROUTES[2][2]
    # Caminho que o Rust não conhece também segue a ele, e volta com código.
    r = client.post("/api/groups/x/y", headers=AUTH, json={})
    assert (r.status_code, r.json()) == (404, {"code": "forwarded_route_not_owned"})
    assert _Rust.seen[-1]["original"] == "/api/groups/x/y"


def test_connect_forward_delivers_what_the_rust_deferred_before_answering(rust, monkeypatch):
    """Recado a sessão que só o Python atende volta no corpo do Rust: o Python o tira da resposta e
    entrega ANTES de responder, num prazo, sem o Rust chamá-lo dentro do pedido; quem não recebeu
    (falha ou prazo) vai em `failed`, como as sessões que o Rust entregou."""
    import asyncio
    sent = []

    async def enviar(name, text):
        if name == "lenta":
            await asyncio.sleep(5)
        sent.append((name, text))
        return {"ok": name == "pi2", "error": {"code": "erro_x", "params": {}, "msg": "x"}}
    monkeypatch.setattr(api, "_enviar", enviar)
    monkeypatch.setattr(api, "_DEFERRED_WAIT", 0.2)
    monkeypatch.setattr(settings, "server_id", "casa")
    registered = []
    monkeypatch.setattr(api.diag, "registrar", lambda evento, nivel, **kw: registered.append((evento, kw)))
    line = "[grupo: casa::s0] oi"
    monkeypatch.setattr(_Rust, "extra", {"protocol": 1, "failed": ["casa::s9"], "hangar_deliveries": [
        {"name": n, "text": line, "steer": False} for n in ("pi1", "pi2", "lenta")]})
    body = {"protocol": 1, "origin": "casa", "sender": "s0", "text": "oi", "notice": "n1"}
    r = TestClient(api.app).post("/api/groups/g1/deliver", json=body, headers=AUTH)
    assert r.status_code == 200 and "hangar_deliveries" not in r.json()
    assert sorted(r.json()["failed"]) == ["casa::lenta", "casa::pi1", "casa::s9"]
    assert sorted(sent) == [("pi1", line), ("pi2", line)]
    assert "grupos.entrega_adiada_falhou" in [e for e, _ in registered]


def test_connect_forward_without_failed_list_answers_with_warning(rust, monkeypatch):
    """Rota sem lista `failed` (o `op` da dona): a falha da entrega vai no `warning` da resposta."""
    async def enviar(name, text):
        return {"ok": False, "error": {"code": "erro_x", "params": {}, "msg": "x"}}
    monkeypatch.setattr(api, "_enviar", enviar)
    monkeypatch.setattr(_Rust, "extra", {"hangar_deliveries": [{"name": "pi1", "text": "oi", "steer": False}]})
    r = TestClient(api.app).post("/api/groups/g1/ops", json={"op": "join"}, headers=AUTH)
    assert r.status_code == 200
    warning = r.json()["warning"]
    assert warning["code"] == "erro_pareamento_aviso_parcial" and warning["params"]["avisos"][0]["sessao"] == "pi1"


def test_groups_routes_refuse_guest_and_python_mode_with_code(rust, monkeypatch):
    client = TestClient(api.app)
    assert client.get("/api/groups/g1").status_code == 401
    marker = guest_users.current.set(object())
    try:
        r = client.get("/api/groups/g1", headers=AUTH)
        assert (r.status_code, r.json()["detail"]["code"]) == (403, "erro_fora_do_convite")
    finally:
        guest_users.current.reset(marker)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    r = client.get("/api/sessions/a/group", headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (503, "groups_need_rust_server")
    assert _Rust.seen == []


@pytest.fixture
def bridge(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    sent = []

    async def enviar(name, text):
        sent.append((name, text))
        return {"ok": name != "falha", "error": {"code": "erro_x", "params": {}, "msg": "x"}}
    monkeypatch.setattr(api, "_enviar", enviar)
    state = SimpleNamespace(calls=[], sent=sent, reply=None)

    def call(op, **args):
        state.calls.append((op, args))
        return state.reply if op == "group.route" else {}
    monkeypatch.setattr(groups_bridge, "call", call)
    return state


def test_bridged_route_reads_no_orq_facts_and_delivers_after(bridge):
    bridge.reply = {"status": 200, "body": {"ok": True, "warning": None},
                    "deliveries": [{"name": "b", "text": "oi", "steer": False}, {"name": "falha", "text": "oi", "steer": False}], "post": []}
    r = TestClient(api.app).post("/api/sessions/a/pair", json={"peer": "b"}, headers=AUTH)
    assert r.status_code == 200
    [(op, args)] = bridge.calls
    assert op == "group.route" and args["facts"] == {}
    assert bridge.sent == [("b", "oi"), ("falha", "oi")]
    warning = r.json()["warning"]
    assert warning["code"] == "erro_pareamento_aviso_parcial" and warning["params"]["avisos"][0]["sessao"] == "falha"


def test_deferred_deliveries_keep_order_per_session(monkeypatch):
    """Sessões diferentes em paralelo; a mesma sessão recebe os recados na ordem devolvida."""
    import asyncio
    sent = []

    async def enviar(name, text):
        await asyncio.sleep(0.02 if text == "1" else 0)
        sent.append((name, text))
        return {"ok": True}
    monkeypatch.setattr(api, "_enviar", enviar)
    failed = asyncio.run(api._deliver_deferred([{"name": "a", "text": "1"}, {"name": "b", "text": "x"}, {"name": "a", "text": "2"}]))
    assert failed == [] and [t for n, t in sent if n == "a"] == ["1", "2"] and sent[0] == ("b", "x")


def test_promote_conflict_comes_from_rust_without_a_second_request(bridge):
    bridge.reply = {"status": 409, "body": {"detail": {
        "code": "erro_orq_arquivo_mudou", "params": {}, "msg": "o grupo já possui outro contrato"}},
        "deliveries": [], "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_promote_os_error_preserves_who_did_not_leave_and_delivers(bridge):
    """A restauração parcial e seu diário são do Rust; a porta entrega o envelope inteiro."""
    bridge.reply = {"status": 409, "body": {"detail": {
        "code": "erro_orq_arquivo_mudou", "params": {"nao_sairam": ["b"]}, "msg": "disco cheio"}},
        "deliveries": [{"name": "pi", "text": "saiu", "steer": False}], "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")
    assert r.json()["detail"]["params"]["nao_sairam"] == ["b"]
    assert bridge.sent == [("pi", "saiu")]
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_promote_without_identity_keeps_the_group_and_warns(bridge):
    bridge.reply = {"status": 200, "body": {"ok": True, "gid": "g9", "warning": {
        "code": "erro_orq_promocao_sem_identidade", "params": {"sessao": "arb", "grupo": "g9"},
        "msg": "o grupo ficou, mas o time não foi promovido"}}, "deliveries": [], "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["warning"]["code"] == "erro_orq_promocao_sem_identidade"
    assert r.json()["gid"] == "g9"
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_promote_undo_reports_the_undo_notices_that_failed(bridge):
    bridge.reply = {"status": 409, "body": {"detail": {
        "code": "erro_orq_arquivo_mudou", "params": {}, "msg": "o grupo já possui outro contrato"}},
        "deliveries": [{"name": "falha", "text": "saiu", "steer": False}], "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")
    assert r.json()["warning"]["params"]["avisos"][0]["sessao"] == "falha"
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_bridged_failure_without_dict_body_goes_to_the_journal(bridge, monkeypatch):
    registered = []
    monkeypatch.setattr(api.diag, "registrar", lambda evento, nivel, **kw: registered.append((evento, kw)))
    bridge.reply = {"status": 200, "body": "texto", "deliveries": [{"name": "falha", "text": "oi", "steer": False}], "post": []}
    r = TestClient(api.app).post("/api/sessions/a/pair", json={"peer": "b"}, headers=AUTH)
    assert r.status_code == 200
    assert "grupos.falha_sem_corpo" in [e for e, _ in registered]
