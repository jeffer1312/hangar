import json

from app import push
from app.hook_state import HookState


def test_add_subscription_upsert(tmp_path, monkeypatch):
    f = tmp_path / "subs.json"
    monkeypatch.setattr(push, "_file", lambda: f)
    sub = {"endpoint": "https://x/1", "keys": {"p256dh": "a", "auth": "b"}}
    push.add_subscription(sub, "Casa", "srv1")
    push.add_subscription(sub, "Casa-renomeada", "srv1")  # mesmo endpoint -> upsert, nao duplica
    data = json.loads(f.read_text())
    assert len(data) == 1
    assert data[0]["label"] == "Casa-renomeada"


def test_notify_no_vapid_is_noop(tmp_path, monkeypatch):
    f = tmp_path / "subs.json"
    monkeypatch.setattr(push, "_file", lambda: f)
    push.add_subscription({"endpoint": "https://x/1", "keys": {}}, "Casa", "s")
    monkeypatch.setattr(push.settings, "vapid_private", "")
    monkeypatch.setattr(push.settings, "vapid_public", "")
    push.notify_awaiting("sessao")  # sem chaves: nao levanta, nao tenta enviar


def test_hookstate_fires_only_on_transition(tmp_path):
    hs = HookState()
    fired: list[str] = []
    hs.on_awaiting = fired.append
    p = tmp_path / "uuid1.json"

    p.write_text(json.dumps({"state": "working", "ts": 1.0}))
    hs._apply(p, notify=True)
    assert fired == []  # working nao dispara

    p.write_text(json.dumps({"state": "awaiting_input", "ts": 2.0}))
    hs._apply(p, notify=True)
    assert fired == ["uuid1"]  # working -> awaiting dispara

    p.write_text(json.dumps({"state": "awaiting_input", "ts": 3.0}))
    hs._apply(p, notify=True)
    assert fired == ["uuid1"]  # awaiting -> awaiting (so ts) nao re-dispara


def test_hookstate_load_existing_does_not_fire(tmp_path):
    hs = HookState()
    fired: list[str] = []
    hs.on_awaiting = fired.append
    p = tmp_path / "u.json"
    p.write_text(json.dumps({"state": "awaiting_input", "ts": 1.0}))
    hs._apply(p, notify=False)  # caminho do load_existing (boot) — nao deve notificar
    assert fired == []
