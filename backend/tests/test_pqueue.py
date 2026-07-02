"""Cobertura do sidecar de fila durável (pqueue): append/load, clear, e rename (move preservando
entradas). Isola o queue dir apontando settings.projects_dir pra um tmp."""
import pytest

from app import pqueue
from app.pqueue import PromptQueue


@pytest.fixture(autouse=True)
def _tmp_queue_dir(tmp_path, monkeypatch):
    # _queue_dir() = settings.projects_dir.parent / ".claude-pocket-queue" -> redireciona pro tmp.
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_append_and_load_roundtrip():
    q = PromptQueue("s")
    q.append("um")
    q.append("dois")
    assert [e["text"] for e in PromptQueue("s").load()] == ["um", "dois"]


def test_clear_removes_sidecar():
    q = PromptQueue("s")
    q.append("x")
    q.clear()
    assert PromptQueue("s").load() == []


def test_rename_moves_entries_and_drops_old():
    PromptQueue("old").append("msg um")
    PromptQueue("old").append("msg dois")
    PromptQueue("old").rename("new")
    assert PromptQueue("old").load() == []  # nome velho ficou vazio
    assert [e["text"] for e in PromptQueue("new").load()] == ["msg um", "msg dois"]


def test_rename_without_queue_is_noop():
    # Sessao sem fila: rename nao deve criar nada nem estourar.
    PromptQueue("sem-fila").rename("destino")
    assert PromptQueue("destino").load() == []


def test_append_default_pending_and_eager_delivered():
    PromptQueue("s").append("pendente")
    PromptQueue("s").append("eager", delivered=True)
    rows = PromptQueue("s").load()
    assert rows[0]["delivered"] is False
    assert rows[1]["delivered"] is True


def test_claim_undelivered_flips_and_is_idempotent():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=True)
    claimed = PromptQueue("s").claim_undelivered()
    assert [c["text"] for c in claimed] == ["a"]              # so a pendente
    assert all(r["delivered"] for r in PromptQueue("s").load())
    assert PromptQueue("s").claim_undelivered() == []          # 2a vez: nada (idempotente)


def test_claim_limit_one():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=False)
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["a"]
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["b"]


def test_claim_respects_min_ts():
    e = PromptQueue("s").append("antiga", delivered=False)
    assert PromptQueue("s").claim_undelivered(min_ts=e["ts"] + 1000) == []
    assert PromptQueue("s").load()[0]["delivered"] is False     # nao reivindicada


def test_claim_ignores_legacy_entry_without_key():
    # Entrada legada (escrita antes do campo): `is False` ESTRITO -> NAO reivindicada (senao um
    # upgrade re-enviaria todo prompt antigo ja entregue).
    p = PromptQueue("s")
    p.path.write_text('{"id":"old1","text":"legada","ts":1.0}\n', encoding="utf-8")
    assert p.claim_undelivered() == []
    assert "delivered" not in p.load()[0]


def test_set_delivered_reverts():
    e = PromptQueue("s").append("x", delivered=True)
    PromptQueue("s").set_delivered(e["id"], False)
    assert PromptQueue("s").load()[0]["delivered"] is False


def test_merged_history_dedup_is_ts_aware(tmp_path):
    # Texto REPETIDO: entrada enfileirada DEPOIS do commit de um texto igual NAO e absorvida por ele
    # (senao o 2o "ok" sumia do historico); a entrada anterior ao commit e absorvida (fluxo normal).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                    "message": {"role": "user", "content": "inicio"}}) + "\n" +
        json.dumps({"type": "user", "uuid": "u1", "timestamp": "2026-01-01T00:01:40Z",
                    "message": {"role": "user", "content": "ok"}}) + "\n",
        encoding="utf-8")
    tc = pqueue._ts_of_line(j.read_text(encoding="utf-8").splitlines()[1])  # epoch do commit de "ok"
    q = PromptQueue("s")
    q.path.write_text(
        json.dumps({"id": "e1", "text": "ok", "ts": tc - 5, "delivered": True}) + "\n" +
        json.dumps({"id": "e2", "text": "ok", "ts": tc + 5, "delivered": False}) + "\n",
        encoding="utf-8")
    ids = [e.id for e in pqueue.merged_history("s", str(j))]
    assert "queued-e1" not in ids      # anterior ao commit -> absorvida pelo user_msg real
    assert "queued-e2" in ids          # posterior ao commit -> ainda pendente, nao some


def test_merged_history_ignores_delivered_flag(tmp_path):
    # delivered NAO afeta exibicao: entrada entregue mas ainda nao gravada no transcript continua
    # aparecendo como bubble queued- (o dedup por texto so a remove quando o user_msg real cai).
    j = tmp_path / "t.jsonl"
    j.write_text("", encoding="utf-8")
    PromptQueue("s").append("oi claude", delivered=True)
    hist = pqueue.merged_history("s", str(j))
    assert any(e.id.startswith("queued-") and e.text == "oi claude" for e in hist)


def test_prune_before_drops_previous_session_entries():
    q = PromptQueue("s")
    q.path.write_text(
        '{"id":"a","text":"velha","ts":10.0,"delivered":false}\n'
        '{"id":"b","text":"nova","ts":100.0,"delivered":false}\n', encoding="utf-8")
    q.prune_before(50.0)
    assert [r["id"] for r in q.load()] == ["b"]
    q.prune_before(0.0)                       # sem ts no transcript -> no-op seguro
    assert [r["id"] for r in q.load()] == ["b"]


def test_reconcile_confirms_requeues_and_silences_old():
    import json
    q = PromptQueue("s")
    rows = [
        {"id": "ok1", "text": "chegou", "ts": 900.0, "delivered": True},
        {"id": "gone", "text": "engolida", "ts": 900.0, "delivered": True},
        {"id": "fresh", "text": "recente", "ts": 999.0, "delivered": True},
        {"id": "old", "text": "pre-clear", "ts": 10.0, "delivered": True},
    ]
    q.path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    req = q.reconcile_delivered({"chegou"}, min_ts=100.0, now=1000.0)
    assert [r["id"] for r in req] == ["gone"]
    got = {r["id"]: r for r in q.load()}
    assert got["ok1"]["confirmed"] is True                       # no transcript -> confirmada
    assert got["gone"]["delivered"] is False and got["gone"]["attempts"] == 1  # re-drena
    assert "confirmed" not in got["fresh"]                       # dentro do grace: checa depois
    assert got["old"]["confirmed"] is True                       # sessao anterior: silenciada


def test_reconcile_gives_up_after_max_attempts():
    q = PromptQueue("s")
    q.path.write_text('{"id":"x","text":"t","ts":900.0,"delivered":true,"attempts":2}\n', encoding="utf-8")
    assert q.reconcile_delivered(set(), min_ts=100.0, now=1000.0) == []
    assert q.load()[0]["confirmed"] is True    # desiste: fica visivel, sem loop de redigitacao


def test_reconcile_strips_attachment_marker():
    import json
    q = PromptQueue("s")
    q.path.write_text(json.dumps(
        {"id": "i", "text": "legenda — 📎 imagem: /x.png", "ts": 900.0, "delivered": True}
    ) + "\n", encoding="utf-8")
    assert q.reconcile_delivered({"legenda"}, min_ts=100.0, now=1000.0) == []
    assert q.load()[0]["confirmed"] is True    # transcript grava so a legenda -> casa sem o 📎
