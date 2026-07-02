import asyncio
import json

import pytest

from app.models import ChatEvent
from app.transcript import parse_line, TranscriptTailer


def _line(obj) -> str:
    return json.dumps(obj)


def test_user_text_message():
    evs = parse_line(_line({
        "type": "user", "uuid": "u1", "parentUuid": None,
        "message": {"role": "user", "content": "corrige o bug"},
    }))
    assert evs == [ChatEvent(kind="user_msg", id="u1", text="corrige o bug")]


def test_assistant_text_message():
    [ev] = parse_line(_line({
        "type": "assistant", "uuid": "a1", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [{"type": "text", "text": "vou olhar"}]},
    }))
    assert ev.kind == "assistant_msg"
    assert ev.text == "vou olhar"


def test_assistant_tool_use():
    [ev] = parse_line(_line({
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
    [ev] = parse_line(_line({
        "type": "user", "uuid": "u2", "parentUuid": "a2",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_9", "content": "file.txt", "is_error": False},
        ]},
    }))
    assert ev.kind == "tool_result"
    assert ev.tool_use_id == "toolu_9"
    assert ev.result == "file.txt"
    assert ev.is_error is False


def test_parallel_tool_results_all_emitted():
    # Tool calls PARALELAS gravam varios tool_result numa entrada user so; TODOS viram evento
    # (antes so o 1o -> o resultado dos demais nunca chegava na UI). Ids extras com sufixo
    # deterministico (o front deduplica e keia bubble por id).
    evs = parse_line(_line({
        "type": "user", "uuid": "u5",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "um"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "dois"},
        ]},
    }))
    assert [(e.kind, e.id, e.tool_use_id, e.result) for e in evs] == [
        ("tool_result", "u5", "t1", "um"),
        ("tool_result", "u5:1", "t2", "dois"),
    ]


def test_assistant_text_and_tool_use_same_entry():
    # text + tool_use na MESMA entrada assistant: os dois viram evento, na ordem do content
    # (antes o tool_use vencia e o texto sumia do chat). thinking e ignorado.
    evs = parse_line(_line({
        "type": "assistant", "uuid": "a9",
        "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "vou rodar"},
            {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}},
        ]},
    }))
    assert [(e.kind, e.id) for e in evs] == [("assistant_msg", "a9"), ("tool_use", "a9:1")]
    assert evs[0].text == "vou rodar"
    assert evs[1].tool_use_id == "toolu_1"


def test_command_meta_entries_are_skipped():
    # Claude Code logs slash-commands / local command I/O as synthetic "user" entries —
    # tooling meta, must not leak into the chat as bubbles.
    assert parse_line(_line({
        "type": "user", "uuid": "c1",
        "message": {"role": "user",
                    "content": "<command-name>/clear</command-name><command-message>clear</command-message>"},
    })) == []
    assert parse_line(_line({
        "type": "user", "uuid": "c2",
        "message": {"role": "user", "content": "<local-command-caveat>Caveat: ...</local-command-caveat>"},
    })) == []
    # a normal message that merely mentions such a tag mid-text is still a real message
    [ev] = parse_line(_line({
        "type": "user", "uuid": "c3",
        "message": {"role": "user", "content": "what does the <command-name> tag do?"},
    }))
    assert ev.kind == "user_msg"


def test_image_meta_entries_are_skipped():
    # Entradas user SINTETICAS cujo texto inteiro e "[Image...]" sao meta do harness (referencia de
    # imagem colada ou anotacao de leitura do modelo), nunca conversa real -> fora do chat.
    for text in (
        "[Image: source: /home/u/pic.png]",
        "[Image: original 1179x2556, displayed at 923x2000. Multiply coordinates by 1.28 to map to original image.]",
        "[Image]",
    ):
        assert parse_line(_line({
            "type": "user", "uuid": "i1",
            "message": {"role": "user", "content": text},
        })) == []
    # Mas uma msg real que MENCIONA a sintaxe nao pode ser engolida.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "i2",
        "message": {"role": "user", "content": "o que e [Image: foo] no log?"},
    }))
    assert ev.kind == "user_msg"


def test_system_reminder_only_message_is_skipped():
    # O harness injeta lembretes ("The user named this session…") como entrada "user" sintetica.
    # Quando a msg e SO o bloco <system-reminder>, e meta — nao pode virar bubble.
    assert parse_line(_line({
        "type": "user", "uuid": "r1",
        "message": {"role": "user",
                    "content": '<system-reminder>\nThe user named this session "corrigindo tmux".\n</system-reminder>'},
    })) == []


def test_system_reminder_stripped_from_real_message():
    # Lembrete ANEXADO a uma msg real: remove so o bloco, mantem o texto do usuario.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "r2",
        "message": {"role": "user",
                    "content": "roda o teste\n<system-reminder>tooling meta aqui</system-reminder>"},
    }))
    assert ev.kind == "user_msg" and ev.text == "roda o teste"


def test_attachment_returns_no_events():
    assert parse_line(_line({"type": "attachment", "uuid": "x"})) == []


def test_blank_or_bad_line_returns_no_events():
    assert parse_line("") == []
    assert parse_line("{not json") == []


def test_real_fixture_lines_parse():
    from pathlib import Path
    p = Path(__file__).parent / "fixtures" / "jsonl_samples.jsonl"
    events = []
    for line in p.read_text().splitlines():
        events.extend(parse_line(line))  # must not raise
    assert any(ev.kind == "assistant_msg" for ev in events)


@pytest.mark.asyncio
async def test_tailer_yields_existing_then_new(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(json.dumps({"type": "user", "uuid": "u1",
                             "message": {"role": "user", "content": "hi"}}) + "\n")
    tailer = TranscriptTailer(f)
    got = []

    async def consume():
        async for ev in tailer.follow():
            got.append(ev)
            if len(got) == 2:
                return

    async def append():
        await asyncio.sleep(0.2)
        with f.open("a") as fh:
            fh.write(json.dumps({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
                                 "message": {"role": "assistant",
                                             "content": [{"type": "text", "text": "yo"}]}}) + "\n")

    await asyncio.wait_for(asyncio.gather(consume(), append()), timeout=5)
    assert [e.id for e in got] == ["u1", "a1"]
