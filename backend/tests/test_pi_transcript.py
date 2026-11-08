import json


def test_committed_user_lines_reads_pi_user_messages(tmp_path, monkeypatch):
    # FINDING 1: o oraculo de confirmacao de entrega so entendia o shape do Claude (`type: user` no
    # TOPO da linha). Pro Pi ele devolvia set() vazio -> o reconcile concluia "a TUI engoliu" e o
    # drain REDIGITAVA o mesmo prompt (evidencia no disco: pi-e2e.jsonl com attempts: 2 e o par
    # user/assistant duplicado no transcript).
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    texto = "reply with exactly the word: pong"
    f = tmp_path / "2026-01-01T00-00-00-000Z_sid.jsonl"
    f.write_text(json.dumps({"type": "message", "id": "n1", "message": {
        "role": "user", "timestamp": 100_000, "content": [{"type": "text", "text": texto}]}}) + "\n",
        encoding="utf-8")

    committed = pqueue.committed_user_lines(str(f), provider="pi")
    assert texto in committed

    q = pqueue.PromptQueue("pi-e2e")
    q.path.write_text(json.dumps({"id": "e1", "text": texto, "ts": 100.0, "delivered": True}) + "\n",
                      encoding="utf-8")
    assert q.reconcile_delivered(committed, min_ts=0.0, now=1000.0) == []   # nada de reentrega
    assert q.load()[0]["confirmed"] is True


def test_committed_user_lines_keeps_the_claude_shape(tmp_path):
    # O ramo novo nao pode mexer no provider mais usado do app.
    from app import pqueue
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "message": {"role": "user", "content": "oi"}}) + "\n",
                 encoding="utf-8")
    assert "oi" in pqueue.committed_user_lines(str(j))
    assert "oi" in pqueue.committed_user_lines(str(j), provider="claude")
