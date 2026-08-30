"""Testes do parser do rollout JSONL do Codex CLI -> ChatEvent (mesmo shape do Claude)."""
import json
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


def test_user_recommended_plugins_wrapper_ignored():
    # O host anexa este bloco como role:user antes da mensagem real. Era o vazamento visto no app:
    # como nao comeca por environment_context, a bolha enorme (incluindo permissoes no fim) aparecia.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<recommended_plugins>\n...\n</recommended_plugins>"
                                            "<environment_context>...</environment_context>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_permissions_instructions_with_space_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<permissions instructions>\n...\n"
                                            "</permissions instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_agents_md_injection_ignored():
    # Quando o cwd tem AGENTS.md, o Codex injeta o conteudo dele como role:"user" gigante que COMECA
    # com "# AGENTS.md instructions for <path>" (com o environment_context concatenado no fim) -- nao
    # e chat do usuario. Sem este filtro, vazava como a 1a bolha da sessao (bug visto ao vivo).
    text = ("# AGENTS.md instructions for /home/jefferson/pessoal/advocacia\n\n<INSTRUCTIONS>\n"
            "# AGENTS.md\n...regras...\n</INSTRUCTIONS>\n<environment_context>\n<cwd>/x</cwd>\n"
            "</environment_context>")
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": text}]}}
    assert parse_rollout_obj(obj) == []


def test_user_message_mentioning_agents_md_kept():
    # Conservador: uma mensagem REAL do usuario que apenas MENCIONA AGENTS.md nao pode ser descartada.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "edita o AGENTS.md pra adicionar X"}]}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "user_msg"


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
    evs = [e for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip()
           for e in parse_rollout_line(ln)]
    kinds = {e.kind for e in evs}
    assert "user_msg" in kinds and "assistant_msg" in kinds


# -- Os dois pré-requisitos do estado ao vivo (ticket 02) ----------------------
# Ligar o marcador de estado do Codex faz o backend passar a rodar o gatilho de transição também
# nesse provider, e lá dentro moram dois caminhos que digitam no pane. Estes casos travam a
# regressão ANTES de ela existir.

def test_confirmacao_de_entrega_entende_o_rollout(tmp_path):
    # Sem o ramo do Codex o oráculo devolve set() vazio, e vazio ali significa "nada chegou" — que
    # é exatamente o que autoriza o reconcile a re-enfileirar e o drain a REDIGITAR a mensagem do
    # usuário na conversa (o incidente já registrado no Pi e no Kimi).
    from app import pqueue
    texto = "responda apenas: ok"
    f = tmp_path / "rollout-2026-08-30T10-00-00-019f5c00-5d7d-7dd2-b2cb-085ca6d76251.jsonl"
    f.write_text("\n".join([
        json.dumps({"timestamp": "2026-08-30T10:00:00.000Z", "type": "response_item", "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text",
                         "text": "<environment_context>x</environment_context>"}]}}),
        json.dumps({"timestamp": "2026-08-30T10:00:01.000Z", "type": "response_item", "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": texto}]}}),
    ]) + "\n", encoding="utf-8")

    committed = pqueue.committed_user_lines(str(f), provider="codex")
    assert texto in committed
    assert "<environment_context>x</environment_context>" not in committed   # não é fala do usuário


def test_confirmacao_de_entrega_no_rollout_real():
    from app import pqueue
    p = Path(__file__).parent / "fixtures/codex/rollout_sample.jsonl"
    assert "responda apenas: ok" in pqueue.committed_user_lines(str(p), provider="codex")
