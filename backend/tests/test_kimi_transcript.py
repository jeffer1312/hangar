import json

from app.adapters.kimi import transcript as kt


# Linhas com o formato MEDIDO num wire.jsonl real (Kimi 0.34.0): envelope {"type",...,"time":ms}.

def _append_user(text, msg_id="msg_1", origin_kind="user", ts=1786453187986):
    return {"type": "context.append_message",
            "message": {"role": "user", "content": [{"type": "text", "text": text}],
                        "toolCalls": [], "origin": {"kind": origin_kind}, "id": msg_id},
            "time": ts}


def _loop(ev, ts=1786453004651):
    return {"type": "context.append_loop_event", "event": ev, "time": ts}


def test_user_message_becomes_user_msg():
    ev = kt.parse_obj(_append_user("oi"))
    assert len(ev) == 1
    assert ev[0].kind == "user_msg"
    assert ev[0].text == "oi"
    assert ev[0].id == "msg_1"
    assert ev[0].ts == 1786453187.986  # envelope em MILISSEGUNDOS -> segundos


def test_injection_message_never_becomes_a_bubble():
    # Medido: cada prompt gera um SEGUNDO append_message com origin.kind=="injection" (o
    # system-reminder de permission mode). Sem o filtro ele virava bolha fantasma no chat.
    ev = kt.parse_obj(_append_user("<system-reminder>Auto permission mode...", origin_kind="injection"))
    assert ev == []


def test_text_part_becomes_assistant_msg():
    obj = _loop({"type": "content.part", "uuid": "u1", "turnId": "0", "step": 1,
                 "part": {"type": "text", "text": "ok"}})
    ev = kt.parse_obj(obj)
    assert [e.kind for e in ev] == ["assistant_msg"]
    assert ev[0].text == "ok"
    assert ev[0].id == "u1"


def test_think_part_is_dropped():
    # Rascunho interno, igual ao thinking do Claude/Pi: nunca vira bolha.
    obj = _loop({"type": "content.part", "uuid": "u2",
                 "part": {"type": "think", "think": "hmm"}})
    assert kt.parse_obj(obj) == []


def test_tool_call_becomes_tool_use():
    obj = _loop({"type": "tool.call", "uuid": "u3", "toolCallId": "tool_1",
                 "name": "Read", "args": {"path": "/tmp/x"}})
    ev = kt.parse_obj(obj)
    assert len(ev) == 1
    assert ev[0].kind == "tool_use"
    assert ev[0].tool_name == "Read"
    assert ev[0].tool_input == {"path": "/tmp/x"}
    assert ev[0].tool_use_id == "tool_1"


def test_tool_result_links_by_tool_call_id():
    obj = _loop({"type": "tool.result", "uuid": "u4", "toolCallId": "tool_1",
                 "result": {"output": "conteudo"}})
    ev = kt.parse_obj(obj)
    assert [e.kind for e in ev] == ["tool_result"]
    assert ev[0].tool_use_id == "tool_1"
    assert ev[0].result == "conteudo"
    assert ev[0].is_error is False


def test_non_conversation_lines_are_ignored():
    for t in ({"type": "metadata", "protocol_version": "1.5"},
              {"type": "turn.prompt", "input": [{"type": "text", "text": "dup"}]},
              {"type": "usage.record", "usage": {"output": 1}},
              {"type": "turn.ended", "turnId": 0},
              {"type": "profile.bind", "modelAlias": "apikey/k3"},
              _loop({"type": "step.begin", "uuid": "u5"})):
        assert kt.parse_obj(t) == [], t


def test_parse_line_tolerates_truncated_json():
    # O tailer le enquanto o Kimi escreve: linha pela metade e rotina, nao erro.
    assert kt.parse_line('{"type":"context.append_mes') == []
    assert kt.parse_line("") == []
    assert kt.parse_line(json.dumps(_append_user("oi")))[0].text == "oi"


def _write(tmp_path, lines):
    p = tmp_path / "wire.jsonl"
    p.write_text("\n".join(json.dumps(l) for l in lines) + "\n", encoding="utf-8")
    return str(p)


def test_pending_question_detected_until_answered(tmp_path):
    call = _loop({"type": "tool.call", "uuid": "u6", "toolCallId": "tool_q",
                  "name": "AskUserQuestion",
                  "args": {"questions": [{"question": "Seguir?", "header": "Confirma",
                                          "options": [{"label": "Sim"}, {"label": "Nao"}]}]}})
    wire = _write(tmp_path, [_append_user("oi"), call])
    q = kt.read_pending_question(wire)
    assert q is not None and q["questions"][0]["question"] == "Seguir?"

    # Respondida (tool.result com o MESMO toolCallId depois) -> some.
    wire = _write(tmp_path, [_append_user("oi"), call,
                             _loop({"type": "tool.result", "uuid": "u7", "toolCallId": "tool_q",
                                    "result": {"output": "Sim"}})])
    assert kt.read_pending_question(wire) is None


def test_pending_question_ignores_other_tools(tmp_path):
    wire = _write(tmp_path, [_loop({"type": "tool.call", "uuid": "u8", "toolCallId": "tool_r",
                                    "name": "Read", "args": {"path": "/x"}})])
    assert kt.read_pending_question(wire) is None
    assert kt.read_pending_question(str(tmp_path / "nope.jsonl")) is None
