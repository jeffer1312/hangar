"""O convidado aciona os mods como o dono (issue #74).

O Rust repassa ao Python todo pedido de app que não é do dono, e o convite entra pela porta 8766 sem
passar por ele: o Python autentica com os dois porteiros e, na sessão que o Rust atende, devolve a
operação pela ponte privada (`/__hangar_server/mods/{name}/{op}`).
"""
import dataclasses
import http.server
import json
import threading

import pytest
from fastapi.testclient import TestClient

from app import api, guest_users, list_bridge, plugin_click, runtime_coordinator, share_api, share_gate, share_store
from app.config import settings
from app.runtime_coordinator import Binding, Phase, RuntimeCoordinator
from app.share_tunnel import GUEST_PORT

OWNER = "segredo-do-dono"
INVITE = "token-do-convite"
SHARE = share_store.Share(
    id="s1", session="t", life="L1", created_at=0.0, code_expires_at=0.0, code_hash="c",
    token_hash="h", device="Pixel", redeemed_at=1.0, revoked_at=None)
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
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    monkeypatch.setattr(guest_users, "session_life", lambda name: "L1" if name == "t" else None)
    guest_users._reset()
    (tmp_path / "raiz").mkdir()
    guest, guest_token = guest_users.create("ana", str(tmp_path / "raiz"), False, True)
    guest_users.claim("t", guest.id)
    # Convite da sessão `t`, vivo.
    monkeypatch.setattr(share_store, "lookup_token", lambda token: share_store.Guest([SHARE]) if token == INVITE else None)
    monkeypatch.setattr(share_gate, "session_life", lambda name: "L1" if name == "t" else None)
    monkeypatch.setattr(share_api, "confirmed_absent", lambda name: False)
    share_gate._life_cache.clear()
    pressed = []

    async def press(name, site, key, plugin):
        pressed.append((name, site, key, plugin))
        return {"ok": True}

    async def close(name, site):
        pressed.append(("close", name, site))
        return {"ok": True}

    monkeypatch.setattr(plugin_click, "press", press)
    monkeypatch.setattr(plugin_click, "close", close)
    api.app.dependency_overrides[api._transfer_guard] = lambda: None
    coordinator = RuntimeCoordinator()
    coordinator.instance = "instance-1"
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    yield coordinator, guest_token, pressed
    api.app.dependency_overrides.pop(api._transfer_guard, None)
    guest_users._reset()


def _press(token, *, invite_port=False, route="press", body=BODY):
    base = f"http://testserver:{GUEST_PORT}" if invite_port else "http://testserver"
    return TestClient(api.app, base_url=base).post(f"/api/sessions/t/plugin/{route}", json=body,
                                                   headers={"Authorization": f"Bearer {token}"})


def test_terminal_in_rust_needs_rust_phase_and_a_claude_terminal(env, tmp_path):
    coordinator, _, _ = env
    slot = _register(coordinator, tmp_path, headless=False)
    assert not coordinator.terminal_in_rust("t"), "terminal ainda na posse do Python"
    slot.phase = Phase.Rust
    assert coordinator.terminal_in_rust("t")
    assert not coordinator.terminal_in_rust("outra")


def test_guests_still_press_without_a_runtime_registry(env, monkeypatch):
    _, guest_token, pressed = env
    monkeypatch.setattr(runtime_coordinator, "_current", None)
    assert _press(guest_token).status_code == 200
    assert pressed == [("t", "above-prompt", "mr-a", "pm-mock")]


def test_revoked_invite_never_reaches_the_refusal(env, tmp_path, monkeypatch):
    # O porteiro do convite responde antes: o 410 diz "encerrado", não a recusa do mod.
    coordinator, _, pressed = env
    _register(coordinator, tmp_path, headless=False).phase = Phase.Rust
    revoked = dataclasses.replace(SHARE, revoked_at=2.0)
    monkeypatch.setattr(share_store, "lookup_token", lambda token: share_store.Guest([revoked]) if token == INVITE else None)
    assert _press(INVITE, invite_port=True).status_code == 410
    assert pressed == []


def test_an_app_without_the_mod_is_still_served(env):
    # O app de antes desta versão não manda o mod: o clique segue sem ele (o `plugin_click` acha o único
    # botão com a `key`), e o `press` com `__close__` continua fechando o painel.
    _, _, pressed = env
    assert _press(OWNER, body={"site": "above-prompt", "key": "mr-a", "plugin": ""}).status_code == 422
    assert _press(OWNER, body={"site": "above-prompt", "key": "mr-a"}).status_code == 200
    assert _press(OWNER, body={"site": "pm-mock-mr", "key": "__close__"}).status_code == 200
    assert pressed == [("t", "above-prompt", "mr-a", None), ("close", "t", "pm-mock-mr")]


def test_closing_a_pane_has_its_own_route(env):
    _, guest_token, pressed = env
    close = {"site": "pm-mock-mr"}
    assert _press(OWNER, route="close", body=close).status_code == 200
    assert _press(OWNER, route="close", body={"site": "pm-mock-mr", "key": "x"}).status_code == 422
    assert _press(guest_token, route="close", body=close).status_code == 200
    assert pressed == [("close", "t", "pm-mock-mr")] * 2


@pytest.mark.parametrize("headless,phase", [(False, Phase.Rust), (False, Phase.Python), (True, Phase.Rust)])
def test_guests_press_like_the_owner(env, tmp_path, headless, phase):
    # Sem o Rust de pé o clique é o do Python, com o teclado emprestado quando o terminal é do Rust.
    coordinator, guest_token, pressed = env
    _register(coordinator, tmp_path, headless=headless).phase = phase
    assert _press(guest_token).status_code == 200
    assert _press(INVITE, invite_port=True).status_code == 200
    assert pressed == [("t", "above-prompt", "mr-a", "pm-mock")] * 2


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


def test_session_the_rust_does_not_serve_falls_back_to_python(env, rust):
    _, guest_token, pressed = env
    response = TestClient(api.app).post("/api/sessions/outra/plugin/press", json=BODY,
                                        headers={"Authorization": f"Bearer {OWNER}"})
    assert response.status_code == 200
    assert [path for path, _, _ in rust] == ["/__hangar_server/mods/outra/press"]
    assert pressed == [("outra", "above-prompt", "mr-a", "pm-mock")]


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
    # O Rust tem o terminal mas a ponte diz que os mods não são dele: o convidado não dirige o pane por
    # aqui; o dono segue com o clique do Python (teclado emprestado).
    coordinator, guest_token, pressed = env
    _register(coordinator, tmp_path, headless=False).phase = Phase.Rust
    _bridge_answers(monkeypatch, coordinator, 404)
    assert _press(guest_token).status_code == 503
    assert _press(INVITE, invite_port=True).status_code == 503
    assert _press(OWNER).status_code == 200
    assert pressed == [("t", "above-prompt", "mr-a", "pm-mock")]


def test_silent_rust_is_a_code_not_a_500(env, monkeypatch):
    coordinator, guest_token, pressed = env
    monkeypatch.setattr(coordinator, "mode", "rust")
    monkeypatch.setattr(list_bridge, "_config", ("127.0.0.1:1", "segredo"))
    response = _press(guest_token)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_mod_clique_sem_resposta")
    assert pressed == []
