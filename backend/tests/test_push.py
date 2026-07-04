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


def _mock_webpush(monkeypatch, tmp_path):
    """Prepara 1 inscricao viva + VAPID configurado, e captura o payload que _send_one manda pro
    pywebpush (import local em _send_one -> monkeypatch no atributo do modulo funciona igual)."""
    import pywebpush
    f = tmp_path / "subs.json"
    monkeypatch.setattr(push, "_file", lambda: f)
    push.add_subscription({"endpoint": "https://x/1", "keys": {"p256dh": "a", "auth": "b"}}, "Casa", "srv1")
    monkeypatch.setattr(push.settings, "vapid_private", "priv")
    monkeypatch.setattr(push.settings, "vapid_public", "pub")
    sent = {}
    def _fake_webpush(subscription_info, data, vapid_private_key, vapid_claims):
        sent["data"] = json.loads(data)
    monkeypatch.setattr(pywebpush, "webpush", _fake_webpush)
    return sent


def test_notify_finished_payload(tmp_path, monkeypatch):
    sent = _mock_webpush(monkeypatch, tmp_path)
    push.notify_finished("minha-sessao")
    assert sent["data"]["body"] == "Terminou"
    assert sent["data"]["session"] == "minha-sessao"
    assert "minha-sessao" in sent["data"]["title"]


def test_notify_dead_payload(tmp_path, monkeypatch):
    sent = _mock_webpush(monkeypatch, tmp_path)
    push.notify_dead("minha-sessao")
    assert sent["data"]["body"] == "Caiu"
    assert sent["data"]["session"] == "minha-sessao"


def test_hookstate_load_existing_does_not_fire(tmp_path):
    hs = HookState()
    fired: list[str] = []
    hs.on_awaiting = fired.append
    p = tmp_path / "u.json"
    p.write_text(json.dumps({"state": "awaiting_input", "ts": 1.0}))
    hs._apply(p, notify=False)  # caminho do load_existing (boot) — nao deve notificar
    assert fired == []
