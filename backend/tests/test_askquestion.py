import json
from pathlib import Path
from app.askquestion import parse_ask_question


def _jsonl(tmp_path: Path, *lines: dict) -> str:
    p = tmp_path / "s.jsonl"
    p.write_text("".join(json.dumps(o) + "\n" for o in lines), encoding="utf-8")
    return str(p)


def _askq_line(questions):
    return {"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "name": "AskUserQuestion", "input": {"questions": questions}}]}}


def test_parse_returns_latest_askquestion(tmp_path):
    q = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
          "options": [{"label": "A", "description": "op A"}, {"label": "B", "description": ""}]}]
    j = _jsonl(tmp_path, {"type": "user", "message": {"content": "oi"}}, _askq_line(q))
    out = parse_ask_question(j)
    assert out is not None
    assert [it.header for it in out.questions] == ["Cor"]
    assert out.questions[0].options[0].label == "A"
    assert out.questions[0].multiSelect is False


def test_parse_none_when_no_askquestion(tmp_path):
    j = _jsonl(tmp_path, {"type": "assistant", "message": {"role": "assistant",
              "content": [{"type": "tool_use", "name": "Read", "input": {}}]}})
    assert parse_ask_question(j) is None
