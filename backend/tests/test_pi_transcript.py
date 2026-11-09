import json


def test_committed_user_lines_keeps_the_claude_shape(tmp_path):
    # O ramo novo nao pode mexer no provider mais usado do app.
    from app import pqueue
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "message": {"role": "user", "content": "oi"}}) + "\n",
                 encoding="utf-8")
    assert "oi" in pqueue.committed_user_lines(str(j))
    assert "oi" in pqueue.committed_user_lines(str(j), provider="claude")
