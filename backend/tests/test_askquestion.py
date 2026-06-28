import json
from pathlib import Path
from app.askquestion import parse_ask_question
from app.models import StateEvent
from app.sse import _ask_question_event


def _state_json(state: str, overlay: bool) -> str:
    return StateEvent(session="s", state=state, overlay=overlay).model_dump_json()


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


# --- _ask_question_event ---

_Q = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
       "options": [{"label": "A", "description": ""}, {"label": "B", "description": ""}]}]


def test_ask_question_event_emits_when_awaiting_with_overlay(tmp_path):
    j = _jsonl(tmp_path, _askq_line(_Q))
    ev = _ask_question_event(_state_json("awaiting_input", overlay=True), j)
    assert ev is not None
    assert ev["event"] == "ask_question"
    parsed = json.loads(ev["data"])
    assert parsed["questions"][0]["header"] == "Cor"


def test_ask_question_event_none_when_working(tmp_path):
    j = _jsonl(tmp_path, _askq_line(_Q))
    assert _ask_question_event(_state_json("working", overlay=False), j) is None


def test_ask_question_event_none_when_no_overlay(tmp_path):
    # awaiting_input sem rodape de abas = menu nativo simples (nao AskUserQuestion tabulado)
    j = _jsonl(tmp_path, _askq_line(_Q))
    assert _ask_question_event(_state_json("awaiting_input", overlay=False), j) is None
