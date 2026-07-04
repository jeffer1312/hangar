import json
from datetime import time as dtime

import pytest

from app import push
from app.hook_state import HookState


@pytest.fixture(autouse=True)
def _reset_coalesce():
    """O buffer/timer de coalescing sao globais do modulo -> sem isto, um Timer real de 2s deixado
    por um teste (notify_awaiting sem flush) vazaria pro proximo teste."""
    yield
    if push._coalesce_timer is not None:
        push._coalesce_timer.cancel()
    push._coalesce_buf.clear()
    push._coalesce_timer = None


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


# ---------------------------------------------------------------------------
# Feature #5: corpo rico + coalescing + mute + quiet hours + deep-link
# ---------------------------------------------------------------------------

def _mock_webpush_multi(monkeypatch, tmp_path):
    """Como _mock_webpush, mas ACUMULA todo envio numa lista (em vez de sobrescrever) — pro teste de
    coalescing contar quantas notificacoes de verdade saíram."""
    import pywebpush
    f = tmp_path / "subs.json"
    monkeypatch.setattr(push, "_file", lambda: f)
    push.add_subscription({"endpoint": "https://x/1", "keys": {"p256dh": "a", "auth": "b"}}, "Casa", "srv1")
    monkeypatch.setattr(push.settings, "vapid_private", "priv")
    monkeypatch.setattr(push.settings, "vapid_public", "pub")
    sent: list[dict] = []
    def _fake_webpush(subscription_info, data, vapid_private_key, vapid_claims):
        sent.append(json.loads(data))
    monkeypatch.setattr(pywebpush, "webpush", _fake_webpush)
    return sent


def test_notify_awaiting_uses_rich_body_and_deep_link(tmp_path, monkeypatch):
    sent = _mock_webpush(monkeypatch, tmp_path)
    push.notify_awaiting("minha-sessao", "Qual branch usar?")
    push._flush_coalesce()  # dispara o buffer agora (sem esperar o debounce real)
    assert sent["data"]["body"] == "Qual branch usar?"
    assert sent["data"]["session"] == "minha-sessao"
    assert "minha-sessao" in sent["data"]["title"]
    assert sent["data"]["url"] == "/?server=srv1&session=minha-sessao"
    assert "tag" not in sent["data"]  # single: SW cai no default (tag=session), sem override


def test_notify_awaiting_coalesces_within_window(tmp_path, monkeypatch):
    sent = _mock_webpush_multi(monkeypatch, tmp_path)
    push.notify_awaiting("a", "pergunta a")
    push.notify_awaiting("b", "pergunta b")  # 2a chegou dentro da janela -> colapsa com a 1a
    push._flush_coalesce()
    assert len(sent) == 1  # UMA notificacao so, nao 2 empilhadas
    payload = sent[0]
    assert payload["tag"] == push._COALESCE_TAG
    assert "a" in payload["body"] and "b" in payload["body"]
    assert "2 sessões aguardando" in payload["title"]
    assert payload["url"] == "/"


def test_notify_awaiting_single_after_flush_not_coalesced_with_next(tmp_path, monkeypatch):
    # Um awaiting isolado (buffer ja esvaziado) manda o push RICO de sempre, sem tag agregada.
    sent = _mock_webpush_multi(monkeypatch, tmp_path)
    push.notify_awaiting("so-uma", "pergunta unica")
    push._flush_coalesce()
    assert len(sent) == 1
    assert sent[0]["body"] == "pergunta unica"
    assert "tag" not in sent[0]


def test_notify_awaiting_respects_mute(tmp_path, monkeypatch):
    sent = _mock_webpush(monkeypatch, tmp_path)
    push.set_muted("minha-sessao", True)
    push.notify_awaiting("minha-sessao", "pergunta")
    push._flush_coalesce()
    assert sent == {}  # muted: nem entrou no buffer


def test_notify_awaiting_respects_quiet_hours(tmp_path, monkeypatch):
    sent = _mock_webpush(monkeypatch, tmp_path)
    monkeypatch.setattr(push, "_in_quiet_hours", lambda: True)
    push.notify_awaiting("s", "pergunta")
    push._flush_coalesce()
    assert sent == {}


def test_set_muted_toggle(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    assert push.is_muted("s1") is False
    push.set_muted("s1", True)
    assert push.is_muted("s1") is True
    push.set_muted("s1", False)
    assert push.is_muted("s1") is False


def test_quiet_hours_same_day_window(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    push.set_quiet_hours("13:00", "18:00")
    assert push._in_quiet_hours(dtime(14, 0)) is True
    assert push._in_quiet_hours(dtime(19, 0)) is False


def test_quiet_hours_crosses_midnight(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    push.set_quiet_hours("22:00", "07:00")
    assert push._in_quiet_hours(dtime(23, 30)) is True
    assert push._in_quiet_hours(dtime(3, 0)) is True
    assert push._in_quiet_hours(dtime(12, 0)) is False


def test_quiet_hours_disabled_by_default(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    assert push._in_quiet_hours() is False


def test_quiet_hours_cleared_by_none(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    push.set_quiet_hours("22:00", "07:00")
    push.set_quiet_hours(None, None)
    assert push.get_push_prefs()["quiet_hours"] is None


def test_set_quiet_hours_rejects_bad_format(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    with pytest.raises(ValueError):
        push.set_quiet_hours("25:99", "07:00")


def test_get_push_prefs(tmp_path, monkeypatch):
    monkeypatch.setattr(push, "_file", lambda: tmp_path / "subs.json")
    push.set_muted("s1", True)
    push.set_quiet_hours("22:00", "07:00")
    prefs = push.get_push_prefs()
    assert prefs["muted"] == ["s1"]
    assert prefs["quiet_hours"] == {"start": "22:00", "end": "07:00"}
