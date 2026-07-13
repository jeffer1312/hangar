"""Testes do parser do rollout JSONL do Codex CLI -> ChatEvent (mesmo shape do Claude)."""
from pathlib import Path

from app.adapters.codex.rollout import parse_rollout_line, parse_rollout_obj


def test_session_meta_ignored():
    obj = {"timestamp": "t", "type": "session_meta", "payload": {"id": "u", "cwd": "."}}
    assert parse_rollout_obj(obj) == []


def test_developer_message_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "developer",
                       "content": [{"type": "input_text", "text": "<permissions instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_message():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "oi"}]}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "user_msg" and "oi" in evs[0].text


def test_user_environment_context_wrapper_ignored():
    # Fix 1: o Codex injeta um response_item/message/role:"user" no inicio de toda thread cujo
    # unico bloco input_text e um wrapper de contexto interno -- nao e chat do usuario, nao pode
    # virar a 1a bolha da sessao.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<environment_context>\ncwd: /x\n</environment_context>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_generic_instructions_wrapper_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "<user_instructions>seja util</user_instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_assistant_message_output_text():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "assistant",
                       "content": [{"type": "output_text", "text": "ok"}], "phase": "final_answer"}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "assistant_msg" and evs[0].text == "ok"


def test_reasoning_encrypted_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "reasoning", "summary": [], "encrypted_content": "gAAAA..."}}
    assert parse_rollout_obj(obj) == []  # opaco no rollout; v1 ignora


def test_tool_call_and_result():
    call = {"timestamp": "t", "type": "response_item",
            "payload": {"type": "function_call", "name": "shell", "arguments": "{}", "call_id": "c1"}}
    out = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "function_call_output", "call_id": "c1", "output": "done"}}
    assert parse_rollout_obj(call)[0].kind == "tool_use"
    assert parse_rollout_obj(out)[0].kind == "tool_result"


def test_real_fixture_parses():
    p = Path(__file__).parent / "fixtures/codex/rollout_sample.jsonl"
    evs = [e for ln in p.read_text().splitlines() if ln.strip()
           for e in parse_rollout_line(ln)]
    kinds = {e.kind for e in evs}
    assert "user_msg" in kinds and "assistant_msg" in kinds
