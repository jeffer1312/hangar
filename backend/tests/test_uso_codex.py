import json
from pathlib import Path

import pytest

from app import costs_sources as cs, uso_areas, uso_codex, uso_report

SID = "019a0000-0000-7000-8000-000000000001"
T0 = "2026-09-10T12:00:00Z"


@pytest.fixture(autouse=True)
def _mapa_limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(uso_areas, "_arquivo", lambda: tmp_path / "uso-areas.json")
    uso_areas.recarregar()
    yield
    uso_areas.recarregar()


def _r(tipo, payload, ts=T0):
    return {"type": tipo, "timestamp": ts, "payload": payload}


def _uso(turno, rid, i, o=0, cache=0):
    return _r("token_usage_record", {"thread_id": SID, "turn_id": turno, "response_id": rid,
                                     "usage": {"input_tokens": i, "cached_input_tokens": cache,
                                               "output_tokens": o}})


def _contador(total):
    return _r("event_msg", {"type": "token_count", "info": {"total_token_usage": {"input_tokens": total}}})


def _exec(call_id, js):
    return _r("response_item", {"type": "custom_tool_call", "name": "exec", "call_id": call_id, "input": js})


def _saida(call_id, texto):
    return _r("response_item", {"type": "custom_tool_call_output", "call_id": call_id,
                                "output": [{"type": "input_text", "text": texto}]})


def _escrever(p: Path, linhas):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) for x in linhas) + "\n", encoding="utf-8")


def test_rollout_codex_tools_skill_e_areas(tmp_path):
    skill = "/h/.codex/plugins/cache/mkt/superpowers/6.3.0/skills/brainstorming/SKILL.md"
    arq = tmp_path / "rollout-2026-09-10T12-00-00-x.jsonl"
    _escrever(arq, [
        _r("session_meta", {"id": SID, "cwd": "/repo", "model_provider": "openai"}),
        _r("turn_context", {"turn_id": "t1", "cwd": "/repo", "model": "gpt-5.6-sol"}),
        _exec("c1", 'text(await tools.exec_command({cmd:"sed -n \'1,240p\' ' + skill + '",workdir:"/repo"}));'),
        _saida("c1", "b" * 1000),
        _uso("t1", "r1", 100, 10),
        _contador(100),
        _exec("c2", 'await tools.apply_patch("*** Begin Patch\\n*** Update File: /repo/frontend/src/A.svelte\\n@@\\n-x\\n+y\\n*** End Patch");\n'
                    'text(await tools.exec_command({cmd:"uv run pytest backend/tests/test_x.py",workdir:"/repo"}));'),
        _saida("c2", "o" * 60),
        _uso("t1", "r2", 300, 30, cache=200),
        _contador(400),
        _r("turn_context", {"turn_id": "t2", "cwd": "/repo", "model": "gpt-5.6-sol"}),
        _uso("t2", "r3", 50, 5),
        _contador(450),
        _r("compacted", {"message": "resumo"}),
        _uso("t2", "r4", 10),
        _contador(460),
    ])
    uso = uso_codex.ler_rollout(arq, cs.respostas_por_turno_codex(arq, "codex:x"))
    assert all(l.fonte == "codex" and l.session_id == SID for l in uso)
    r = uso_report.montar(uso, [], "all")
    sk = {b.key: b for b in r.by_skill}["superpowers:brainstorming"]
    # Carga em r1; no contexto em r2 e r3; a compactação tira antes de r4.
    assert (sk.chamadas, sk.respostas, sk.plugin) == (1, 3, "superpowers")
    assert sk.ocupados_tokens_est == int(1000 * 3 / 2.5)
    tools = {b.key: b.chamadas for b in r.by_tool}
    assert tools == {"exec_command": 2, "apply_patch": 1}
    assert {b.key for b in r.by_bash} == {"sed", "uv"}
