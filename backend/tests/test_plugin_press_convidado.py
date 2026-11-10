"""O convidado aciona os mods como o dono (issue #74).

O Rust repassa ao Python todo pedido de app que não é do dono; o convite entra pela porta 8766 do
Rust, que o decide e o repassa assinado (`x-hangar-guest`).
"""
import base64
import hashlib
import hmac
import json
import time

import pytest
from fastapi.testclient import TestClient

from app import api, internal_api, runtime_coordinator
from app.config import settings
from app.runtime_coordinator import RuntimeCoordinator

OWNER = "segredo-do-dono"
INVITE = "token-do-convite"
SECRET = "segredo-interno-do-teste"
BODY = {"site": "above-prompt", "key": "mr-a", "plugin": "pm-mock"}


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
