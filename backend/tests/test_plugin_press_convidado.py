"""O convidado aciona os mods como o dono (issue #74).

O Rust repassa ao Python todo pedido de app que não é do dono; o convite entra pela porta 8766 do
Rust, que o decide e o repassa assinado (`x-hangar-guest`). Na sessão que o Rust atende o Python
devolve a operação pela ponte privada (`/__hangar_server/mods/{name}/{op}`).
"""
import base64
import hashlib
import hmac
import http.server
import json
import threading
import time

import pytest
from fastapi.testclient import TestClient

from app import api, internal_api, list_bridge, runtime_coordinator
from app.config import settings
from app.runtime_coordinator import Binding, Phase, RuntimeCoordinator

OWNER = "segredo-do-dono"
INVITE = "token-do-convite"
SECRET = "segredo-interno-do-teste"
BODY = {"site": "above-prompt", "key": "mr-a", "plugin": "pm-mock"}


def _register(coordinator, tmp_path, *, headless):
    meta = {"key": "key"} if headless else {"key": "key", "terminal": {}}
    slot = coordinator.register(Binding(name="t", key="key", provider="claude", headless=headless, meta=meta,
        jsonl=str(tmp_path / "chat.jsonl"), projection_dir=tmp_path / "projection",
        state_path=tmp_path / "key.json", lock_path=tmp_path / "key.lock", generation=1))
    slot.lease.close()
    return slot


@pytest.fixture
def env(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", OWNER)
    guest_token = "login-forwarded"
    # Convite da sessão `t`, vivo: o Rust o decidiu e assina o repasse.
    internal_api.set_secret(SECRET)
    # O clique é do Rust: o Python não clica nada, em modo nenhum.
    pressed = []
    api.app.dependency_overrides[api._transfer_guard] = lambda: None
    coordinator = RuntimeCoordinator()
    coordinator.instance = "instance-1"
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    yield coordinator, guest_token, pressed
    api.app.dependency_overrides.pop(api._transfer_guard, None)
    internal_api.set_secret(None)


def _signed(method: str, path: str) -> str:
    payload = base64.urlsafe_b64encode(json.dumps({"s": {"t": "share"}, "exp": int(time.time()) + 30},
                                                  separators=(",", ":")).encode()).rstrip(b"=").decode()
    mac = hmac.new(SECRET.encode(), f"{method}\n{path}\n{payload}".encode(), hashlib.sha256).hexdigest()
    return f"{payload}.{mac}"


def _press(token, *, invite_port=False, route="press", body=BODY):
    path = f"/api/sessions/t/plugin/{route}"
    if invite_port or token == "login-forwarded":
        # O convite chega pelo Rust: já decidido, assinado, de loopback e sem o token dele.
        return TestClient(api.app, client=("127.0.0.1", 5)).post(
            path, json=body, headers={"x-hangar-guest": _signed("POST", path)})
    return TestClient(api.app).post(path, json=body, headers={"Authorization": f"Bearer {token}"})


def test_terminal_in_rust_needs_rust_phase_and_a_claude_terminal(env, tmp_path):
    coordinator, _, _ = env
    slot = _register(coordinator, tmp_path, headless=False)
    assert not coordinator.terminal_in_rust("t"), "terminal ainda na posse do Python"
    slot.phase = Phase.Rust
    assert coordinator.terminal_in_rust("t")
    assert not coordinator.terminal_in_rust("outra")


def _needs_rust(response):
    return (response.status_code, response.json()["detail"]["code"]) == (503, "mods_need_rust_server")


def test_without_the_rust_the_press_answers_with_a_code(env, monkeypatch):
    # Modo `python`: o clique dos mods é do Rust, sem reserva aqui; o corpo continua conferido.
    _, guest_token, _ = env
    assert _press(OWNER, body={"site": "above-prompt", "key": "mr-a", "plugin": ""}).status_code == 422
    assert _press(OWNER, route="close", body={"site": "pm-mock-mr", "key": "x"}).status_code == 422
    for token, port in [(OWNER, False), (guest_token, False), (INVITE, True)]:
        assert _needs_rust(_press(token, invite_port=port))
        assert _needs_rust(_press(token, invite_port=port, body={"site": "pm-mock-mr", "key": "__close__"}))
        assert _needs_rust(_press(token, invite_port=port, route="close", body={"site": "pm-mock-mr"}))
    monkeypatch.setattr(runtime_coordinator, "_current", None)
    assert _needs_rust(_press(guest_token))


def test_show_and_input_outside_the_rust_answer_with_a_code(env):
    # Nunca 405: a troca de aba fica no app (404, que ele lê como "sem a rota") e a digitação é recusada.
    _, guest_token, _ = env
    for token, port in [(OWNER, False), (guest_token, False), (INVITE, True)]:
        shown = _press(token, invite_port=port, route="show", body={"site": "p"})
        assert (shown.status_code, shown.json()["detail"]["code"]) == (404, "erro_mod_aba_no_app")
        typed = _press(token, invite_port=port, route="input",
                       body={"site": "p", "plugin": "m", "key": "k", "kind": "change", "value": "o"})
        assert (typed.status_code, typed.json()["detail"]["code"]) == (409, "erro_mod_sem_digitacao")


@pytest.fixture
def rust(env, monkeypatch):
    """Rust falso na porta privada: a sessão `t` é dele; as outras, 404 (o Python trata)."""
    calls = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["content-length"]))
            calls.append((self.path, self.headers["x-hangar-internal"], json.loads(body)))
            status, answer = (200, {"ok": True, "shown_id": "p"}) if self.path.startswith("/__hangar_server/mods/t/") else (404, {})
            raw = json.dumps(answer).encode()
            self.send_response(status)
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    coordinator, _, _ = env
    monkeypatch.setattr(coordinator, "mode", "rust")
    monkeypatch.setattr(list_bridge, "_config", (f"127.0.0.1:{server.server_port}", "segredo"))
    yield calls
    server.shutdown()


def test_guests_act_on_a_rust_session_through_the_bridge(env, rust):
    _, guest_token, pressed = env
    typing = {"site": "p", "plugin": "m", "key": "k", "kind": "submit", "value": "olá"}
    assert _press(guest_token).json() == {"ok": True, "shown_id": "p"}
    assert _press(INVITE, invite_port=True, route="show", body={"site": "p"}).status_code == 200
    assert _press(guest_token, route="input", body=typing).status_code == 200
    assert _press(INVITE, invite_port=True, route="close", body={"site": "p"}).status_code == 200
    assert [(path, secret) for path, secret, _ in rust] == [
        (f"/__hangar_server/mods/t/{op}", "segredo") for op in ("press", "show", "input", "close")]
    assert rust[0][2] == BODY and rust[2][2] == typing, "o corpo vai como o app mandou"
    assert pressed == [], "o clique é do Rust, não do Python"


def test_session_the_rust_does_not_serve_is_a_code_not_a_python_click(env, rust):
    response = TestClient(api.app).post("/api/sessions/outra/plugin/press", json=BODY,
                                        headers={"Authorization": f"Bearer {OWNER}"})
    assert _needs_rust(response)
    assert [path for path, _, _ in rust] == ["/__hangar_server/mods/outra/press"]


def _bridge_answers(monkeypatch, coordinator, status):
    monkeypatch.setattr(coordinator, "mode", "rust")
    monkeypatch.setattr(list_bridge, "_config", ("127.0.0.1:1", "segredo"))

    def answer(self, req, timeout=None):
        raise api.urllib.error.HTTPError(req.full_url, status, "x", {}, None)

    monkeypatch.setattr(api.urllib.request.OpenerDirector, "open", answer)


@pytest.mark.parametrize("status", [403, 400])
def test_bridge_refusal_never_falls_back_to_python(env, monkeypatch, status):
    # Segredo divergente (403) ou operação desconhecida (400) não é "sessão fora do Rust": nada de clique daqui.
    coordinator, guest_token, pressed = env
    _bridge_answers(monkeypatch, coordinator, status)
    for token in (OWNER, guest_token):
        response = _press(token)
        assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_mod_clique_sem_resposta")
    assert pressed == []


def test_guest_never_drives_a_rust_terminal_from_python(env, monkeypatch, tmp_path):
    # O Rust tem o terminal mas a ponte diz que os mods não são dele: ninguém dirige o pane por aqui.
    coordinator, guest_token, _ = env
    _register(coordinator, tmp_path, headless=False).phase = Phase.Rust
    _bridge_answers(monkeypatch, coordinator, 404)
    assert _press(guest_token).status_code == 503
    assert _press(INVITE, invite_port=True).status_code == 503
    assert _needs_rust(_press(OWNER))


def test_silent_rust_is_a_code_not_a_500(env, monkeypatch):
    coordinator, guest_token, pressed = env
    monkeypatch.setattr(coordinator, "mode", "rust")
    monkeypatch.setattr(list_bridge, "_config", ("127.0.0.1:1", "segredo"))
    response = _press(guest_token)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_mod_clique_sem_resposta")
    assert pressed == []
