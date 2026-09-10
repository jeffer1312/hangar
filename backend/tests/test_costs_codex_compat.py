import json

import pytest

from app.costs_sources import _linhas_rollout_codex


def event(kind, payload, timestamp="2026-09-10T12:00:01Z"):
    return {"type": kind, "timestamp": timestamp, "payload": payload}


def usage(tokens):
    return {"input_tokens": tokens, "cached_input_tokens": 0, "output_tokens": 0}


def count(tokens):
    return event("event_msg", {"type": "token_count", "info": {"total_token_usage": usage(tokens)}})


def record(tokens, response="r1"):
    return event("token_usage_record", {
        "thread_id": "child", "turn_id": "turn", "response_id": response, "usage": usage(tokens)})


def read(tmp_path, rows):
    path = tmp_path / "rollout-child.jsonl"
    path.write_text("\n".join(json.dumps(row) for row in rows), encoding="utf-8")
    return _linhas_rollout_codex(path, "account")


def start():
    return [event("session_meta", {"id": "child", "cwd": "/repo", "model_provider": "openai"},
                  "2026-09-10T12:00:00Z"),
            event("turn_context", {"turn_id": "turn", "model": "gpt-6-astra"})]


@pytest.mark.parametrize("before", [True, False])
def test_registro_parcial_preserva_saldo_e_nao_duplica_quando_chega(tmp_path, before):
    first = [count(100), record(100)] if before else [record(100), count(100)]
    rows = start() + first + [count(300), count(300)]
    assert sum(r.input for r in read(tmp_path, rows)) == 300
    rows.append(record(200, "r2"))
    assert sum(r.input for r in read(tmp_path, rows)) == 300


def test_saldo_preserva_modelo_da_resposta_sem_registro(tmp_path):
    rows = start() + [count(100), event("turn_context", {
        "turn_id": "turn", "model": "gpt-5.6-luna"}), count(300), record(200)]
    result = read(tmp_path, rows)
    assert {r.model: r.input for r in result} == {"gpt-6-astra": 100, "gpt-5.6-luna": 200}


def test_fork_legado_exclui_pai_comprovado_e_mantem_baseline(tmp_path):
    rows = start()[:1] + [
        event("session_meta", {"id": "parent"}, "2026-09-09T12:00:00Z"),
        event("turn_context", {"turn_id": "parent-turn", "model": "gpt-6-astra"},
              "2026-09-09T12:00:00Z"),
        event("event_msg", {"type": "token_count", "info": {"total_token_usage": usage(100)}},
              "2026-09-09T12:00:01Z"),
        *start()[1:], count(150),
    ]
    result = read(tmp_path, rows)
    assert sum(r.input for r in result) == 50
    assert all(r.session_id == "child" and r.ts.day == 10 for r in result)
