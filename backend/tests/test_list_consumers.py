"""Task 16: com o Rust dono (ou esperado), os consumidores Python leem a lista pela ponte e a
descoberta Python (`registry.list`, `resolve_tracked`, `list_with_state`, o produtor da lista) não
roda em caminho nenhum. Erro da ponte levanta; nunca vira lista vazia nem a lista do Python."""
import asyncio
import json
import os
import threading
import time

import pytest

from app import list_bridge, prune, registry, runtime_coordinator, sse
from app.models import SessionInfo
from app.registry import SessionRegistry


class _Coordinator:
    def __init__(self, mode):
        self.mode, self.loop = mode, None


def _no_python(*_a, **_k):
    raise AssertionError("descoberta Python no modo rust")


async def _no_python_async(*_a, **_k):
    raise AssertionError("descoberta Python no modo rust")


def _down(*_a, **_k):
    raise list_bridge.ListBridgeError("list_bridge_unavailable")


def _rows(*names):
    return [SessionInfo(name=n, cwd="/p") for n in names]


def _names(rows):
    return [r.name if isinstance(r, SessionInfo) else r["name"] for r in rows]


@pytest.fixture
def rust(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "_current", _Coordinator("rust"))
    registry.PYTHON_DISCOVERY.clear()
    # A descoberta Python inteira passa por estes dois: trocados por uma que falha.
    monkeypatch.setattr(registry.tmux, "list_panes_all", _no_python)
    monkeypatch.setattr(registry, "_proc_children_map", _no_python)
    yield
    assert not registry.PYTHON_DISCOVERY, dict(registry.PYTHON_DISCOVERY)


def test_registry_list_uses_bridge_in_rust_mode(rust, monkeypatch):
    calls = []
    monkeypatch.setattr(list_bridge, "discover", lambda newer_than=None: calls.append(newer_than) or _rows("alfa"))
    monkeypatch.setattr(list_bridge, "snapshot", lambda: _rows("alfa", "beta"))
    reg = SessionRegistry()
    assert _names(reg.list()) == ["alfa"]
    assert _names(reg.list(newer_than=5.0)) == ["alfa"]
    assert calls == [None, 5.0]
    assert _names(asyncio.run(reg.list_with_state())) == ["alfa", "beta"]
    # Quem já tinha a lista crua recebe o retrato só dessas sessões.
    assert _names(asyncio.run(reg.list_with_state(_rows("beta")))) == ["beta"]
    monkeypatch.setattr(list_bridge, "discover", _down)
    with pytest.raises(list_bridge.ListBridgeError):
        reg.list()


def test_resolve_tracked_uses_bridge_in_rust_mode(rust, monkeypatch):
    seen = []
    monkeypatch.setattr(list_bridge, "resolve",
                        lambda name, cwd, pid=None: seen.append((name, cwd, pid)) or ("/x/a.jsonl", True))
    reg = SessionRegistry()
    assert reg.resolve_tracked("alfa", "/p", pid=42) == ("/x/a.jsonl", True)
    assert reg.resolve("alfa", "/p") == "/x/a.jsonl"
    assert seen == [("alfa", "/p", 42), ("alfa", "/p", None)]


def test_registry_waits_pending_then_fails_with_code(monkeypatch):
    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    try:
        coordinator = runtime_coordinator.RuntimeCoordinator()
        coordinator.mode, coordinator.loop = "pending", loop
        monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
        monkeypatch.setattr(runtime_coordinator, "PENDING_WAIT_S", 0.2)
        monkeypatch.setattr(registry.tmux, "list_panes_all", _no_python)
        monkeypatch.setattr(list_bridge, "discover", _no_python)
        registry.PYTHON_DISCOVERY.clear()
        started = time.monotonic()
        with pytest.raises(list_bridge.ListBridgeError) as error:
            SessionRegistry().list()
        assert error.value.code == "list_runtime_starting"
        assert time.monotonic() - started >= 0.2
        # O Rust chega durante a espera: a mesma chamada vai à ponte.
        monkeypatch.setattr(runtime_coordinator, "PENDING_WAIT_S", 5.0)
        monkeypatch.setattr(list_bridge, "discover", lambda newer_than=None: _rows("alfa"))
        threading.Timer(0.1, lambda: loop.call_soon_threadsafe(coordinator._set_mode, "rust")).start()
        assert _names(SessionRegistry().list()) == ["alfa"]
        # A adoção das sessões pelo Rust que subiu roda com o modo ainda `pending` e sem esperar:
        # a resolução dela é do Rust, não do Python nem de uma espera que só ela destrava.
        coordinator.mode = "pending"
        token = runtime_coordinator._mode_bypass.set(True)
        try:
            monkeypatch.setattr(list_bridge, "resolve", lambda name, cwd, pid=None: ("/x/a.jsonl", True))
            assert SessionRegistry().resolve_tracked("alfa", "/p", pid=7) == ("/x/a.jsonl", True)
        finally:
            runtime_coordinator._mode_bypass.reset(token)
        assert not registry.PYTHON_DISCOVERY
    finally:
        loop.call_soon_threadsafe(loop.stop)
        thread.join(2)
        loop.close()


def _first_list_event(**kwargs):
    async def run():
        gen = sse.list_events(ping_secs=60, **kwargs)
        try:
            return await asyncio.wait_for(gen.__anext__(), 2)
        finally:
            await gen.aclose()
    return asyncio.run(run())


def test_list_refresher_never_starts_in_rust_mode(rust, monkeypatch):
    monkeypatch.setattr(sse, "_shortcuts_snapshot", lambda: None)
    monkeypatch.setattr(sse, "_cached_list", _no_python_async)
    monkeypatch.setattr(sse._list_registry, "list_with_state", _no_python_async)
    monkeypatch.setattr(list_bridge, "snapshot", lambda: _rows("alfa"))
    event = _first_list_event()
    assert event["event"] == "sessions"
    assert _names(json.loads(event["data"])) == ["alfa"]
    # A lista do Rust não é a "lista que o Python serviu" (a sombra compararia o Rust com ele mesmo).
    assert sse.recent_list(60) is None
    monkeypatch.setattr(list_bridge, "snapshot", _down)
    assert _first_list_event()["event"] == "list_error"




def test_prune_keeps_files_on_bridge_error(rust, tmp_path, monkeypatch):
    monkeypatch.setattr(list_bridge, "discover", _down)
    status = tmp_path / ".claude" / ".hangar-status"
    status.mkdir(parents=True)
    old = status / "morta.json"
    old.write_text("{}")
    os.utime(old, (1, 1))
    with pytest.raises(list_bridge.ListBridgeError):
        prune.prune_sidecars(bases=[tmp_path / ".claude"], agora=time.time())
    assert old.exists()


@pytest.fixture
def guest_ana(tmp_path, monkeypatch):
    from app import guest_users
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    monkeypatch.setattr(guest_users, "session_life", lambda n: {"alfa": "t:1", "dono": "t:2"}.get(n))
    guest_users._reset()
    (tmp_path / "p").mkdir()
    guest, token = guest_users.create("ana", str(tmp_path / "p"), False, False)
    guest_users.claim("alfa", guest.id)
    yield guest, token
    guest_users._reset()


def test_guest_list_filters_bridge_snapshot(rust, guest_ana, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api
    guest, token = guest_ana
    monkeypatch.setattr(sse, "_shortcuts_snapshot", lambda: None)
    monkeypatch.setattr(api.settings, "auth_token", "owner-token")
    monkeypatch.setattr(api, "_guardar_snap", _no_python)
    monkeypatch.setattr(api.registry, "list_with_state", _no_python_async)
    monkeypatch.setattr(list_bridge, "snapshot", lambda: _rows("alfa", "dono"))
    client = TestClient(api.app)
    response = client.get("/api/sessions", headers={"Authorization": f"Bearer {token}"})
    assert response.status_code == 200, response.text
    assert _names(response.json()) == ["alfa"]
    assert _names(json.loads(_first_list_event(viewer=guest)["data"])) == ["alfa"]
    monkeypatch.setattr(list_bridge, "snapshot", _down)
    response = client.get("/api/sessions", headers={"Authorization": f"Bearer {token}"})
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "erro_lista_indisponivel"
