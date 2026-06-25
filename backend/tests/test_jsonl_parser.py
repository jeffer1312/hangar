import json
from app.transcript import parse_line
from app.models import ChatEvent


def _line(obj) -> str:
    return json.dumps(obj)


def test_user_text_message():
    ev = parse_line(_line({
        "type": "user", "uuid": "u1", "parentUuid": None,
        "message": {"role": "user", "content": "corrige o bug"},
    }))
    assert ev == ChatEvent(kind="user_msg", id="u1", parent_id=None, text="corrige o bug")


def test_assistant_text_message():
    ev = parse_line(_line({
        "type": "assistant", "uuid": "a1", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [{"type": "text", "text": "vou olhar"}]},
    }))
    assert ev.kind == "assistant_msg"
    assert ev.text == "vou olhar"
    assert ev.parent_id == "u1"


def test_assistant_tool_use():
    ev = parse_line(_line({
        "type": "assistant", "uuid": "a2", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_9", "name": "Bash", "input": {"command": "ls"}},
        ]},
    }))
    assert ev.kind == "tool_use"
    assert ev.tool_name == "Bash"
    assert ev.tool_use_id == "toolu_9"
    assert ev.tool_input == {"command": "ls"}


def test_user_tool_result_is_not_a_bubble():
    ev = parse_line(_line({
        "type": "user", "uuid": "u2", "parentUuid": "a2",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_9", "content": "file.txt", "is_error": False},
        ]},
    }))
    assert ev.kind == "tool_result"
    assert ev.tool_use_id == "toolu_9"
    assert ev.result == "file.txt"
    assert ev.is_error is False


def test_attachment_returns_none():
    assert parse_line(_line({"type": "attachment", "uuid": "x"})) is None


def test_blank_or_bad_line_returns_none():
    assert parse_line("") is None
    assert parse_line("{not json") is None


def test_real_fixture_lines_parse():
    from pathlib import Path
    p = Path(__file__).parent / "fixtures" / "jsonl_samples.jsonl"
    events = []
    for line in p.read_text().splitlines():
        ev = parse_line(line)  # must not raise
        if ev is not None:
            events.append(ev)
    assert any(ev.kind == "assistant_msg" for ev in events)
