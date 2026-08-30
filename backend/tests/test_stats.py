"""stats.Accumulator: fold incremental por provider (shapes copiados dos arquivos reais)."""
import json
from pathlib import Path

from app.stats import Accumulator


def _w(path, objs, mode="a"):
    with open(path, mode) as f:
        for o in objs:
            f.write(json.dumps(o) + "\n")


# -- Claude -------------------------------------------------------------------

def _claude_user(ts, text="oi"):
    return {"type": "user", "timestamp": ts, "message": {"role": "user", "content": text}}


def _claude_assistant(ts, mid, out=100, blocks=None):
    return {"type": "assistant", "timestamp": ts,
            "message": {"id": mid, "role": "assistant",
                        "content": blocks or [{"type": "text", "text": "resp"}],
                        "usage": {"input_tokens": 10, "output_tokens": out,
                                  "cache_read_input_tokens": 80,
                                  "cache_creation_input_tokens": 10}}}


def test_claude_dedup_por_message_id_e_ttft(tmp_path):
    p = tmp_path / "s.jsonl"
    # mesma mensagem em 2 linhas (1 por bloco) -> usage conta UMA vez
    _w(p, [
        _claude_user("2026-08-17T12:00:00Z"),
        _claude_assistant("2026-08-17T12:00:02Z", "m1"),
        _claude_assistant("2026-08-17T12:00:03Z", "m1"),
    ])
    snap = Accumulator("claude", str(p)).collect()
    assert snap["turns"] == 1
    assert snap["steps"] == 1
    assert snap["in_tok"] == 100 and snap["out_tok"] == 100
    assert snap["cache_pct"] == 80
    assert snap["ttft_ms"] == 2000


def test_claude_tool_time_e_llm_time(tmp_path):
    p = tmp_path / "s.jsonl"
    _w(p, [
        _claude_user("2026-08-17T12:00:00Z"),
        _claude_assistant("2026-08-17T12:00:02Z", "m1",
                          blocks=[{"type": "tool_use", "id": "t1", "name": "Bash"}]),
        {"type": "user", "timestamp": "2026-08-17T12:00:07Z",
         "message": {"role": "user",
                     "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}},
        _claude_assistant("2026-08-17T12:00:10Z", "m2"),
    ])
    snap = Accumulator("claude", str(p)).collect()
    assert snap["tool_ms"] == 5000          # 12:00:02 -> 12:00:07
    assert snap["llm_ms"] == 5000           # gap até m1 (2s) + gap tool_result->m2 (3s)
    assert snap["steps"] == 2


def test_claude_sidechain_conta_tokens_mas_nao_tempo(tmp_path):
    p = tmp_path / "s.jsonl"
    side = _claude_assistant("2026-08-17T12:00:01Z", "sub1")
    side["isSidechain"] = True
    _w(p, [_claude_user("2026-08-17T12:00:00Z"), side])
    snap = Accumulator("claude", str(p)).collect()
    assert snap["steps"] == 1 and snap["out_tok"] == 100
    assert "ttft_ms" not in snap and "llm_ms" not in snap


def test_incremental_e_truncamento(tmp_path):
    p = tmp_path / "s.jsonl"
    acc = Accumulator("claude", str(p))
    _w(p, [_claude_user("2026-08-17T12:00:00Z"), _claude_assistant("2026-08-17T12:00:02Z", "m1")])
    assert acc.collect()["steps"] == 1
    _w(p, [_claude_assistant("2026-08-17T12:00:05Z", "m2")])       # só a linha nova é lida
    assert acc.collect()["steps"] == 2
    _w(p, [_claude_user("2026-08-17T13:00:00Z"),                    # truncou -> refolda do zero
           _claude_assistant("2026-08-17T13:00:01Z", "x1")], mode="w")
    assert acc.collect()["steps"] == 1


def test_linha_parcial_fica_pro_proximo_collect(tmp_path):
    p = tmp_path / "s.jsonl"
    acc = Accumulator("claude", str(p))
    inteira = json.dumps(_claude_assistant("2026-08-17T12:00:02Z", "m1"))
    metade = json.dumps(_claude_assistant("2026-08-17T12:00:05Z", "m2"))
    with open(p, "w") as f:
        f.write(inteira + "\n" + metade[:40])
    assert acc.collect()["steps"] == 1
    with open(p, "a") as f:
        f.write(metade[40:] + "\n")
    assert acc.collect()["steps"] == 2


# -- Kimi ---------------------------------------------------------------------

def test_kimi_usage_llm_e_tool(tmp_path):
    p = tmp_path / "wire.jsonl"
    _w(p, [
        {"type": "turn.prompt", "time": 1000_000},
        {"type": "llm.request", "time": 1001_000},
        {"type": "context.append_loop_event", "time": 1002_000,
         "event": {"type": "content.part"}},
        {"type": "context.append_loop_event", "time": 1003_000,
         "event": {"type": "tool.call", "id": "c1"}},
        {"type": "context.append_loop_event", "time": 1007_000,
         "event": {"type": "tool.result", "id": "c1"}},
        {"type": "usage.record", "time": 1009_000, "model": "apikey/k3",
         "usage": {"inputOther": 100, "output": 50, "inputCacheRead": 300,
                   "inputCacheCreation": 0}},
    ])
    snap = Accumulator("kimi", str(p)).collect()
    assert snap == {"turns": 1, "steps": 1, "in_tok": 400, "out_tok": 50,
                    "llm_ms": 8000, "tok_s": 6.2, "tool_ms": 4000,
                    "cache_pct": 75, "ttft_ms": 2000}


# -- Codex --------------------------------------------------------------------

def _codex_item(ts, payload):
    return {"timestamp": ts, "type": "response_item", "payload": payload}


def _codex_token_count(ts, inp, cached, out, last=None):
    # Os números são o ACUMULADO da thread (`total_token_usage`); `input_tokens` já inclui cache.
    return {"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count", "info": {
        "total_token_usage": {"input_tokens": inp, "cached_input_tokens": cached,
                              "output_tokens": out},
        "last_token_usage": last or {"input_tokens": inp, "cached_input_tokens": cached,
                                     "output_tokens": out}}}}


def test_codex_usage_llm_e_tool(tmp_path):
    p = tmp_path / "rollout.jsonl"
    _w(p, [
        _codex_item("2026-08-30T12:00:00.000Z",
                    {"type": "message", "role": "user",
                     "content": [{"type": "input_text", "text": "oi"}]}),
        _codex_item("2026-08-30T12:00:02.000Z",
                    {"type": "message", "role": "assistant",
                     "content": [{"type": "output_text", "text": "vou olhar"}]}),
        _codex_item("2026-08-30T12:00:03.000Z",
                    {"type": "custom_tool_call", "call_id": "c1", "name": "exec"}),
        _codex_item("2026-08-30T12:00:08.000Z",
                    {"type": "custom_tool_call_output", "call_id": "c1", "output": "ok"}),
        _codex_token_count("2026-08-30T12:00:08.010Z", 400, 300, 50),
    ])
    snap = Accumulator("codex", str(p)).collect()
    assert snap["turns"] == 1 and snap["steps"] == 1
    assert snap["in_tok"] == 400 and snap["out_tok"] == 50
    assert snap["cache_pct"] == 75
    assert snap["ttft_ms"] == 2000        # user -> 1ª linha do assistente
    assert snap["llm_ms"] == 3000         # 2s até a resposta + 1s gerando a chamada
    assert snap["tool_ms"] == 5000        # 12:00:03 -> 12:00:08


def test_codex_contexto_injetado_nao_e_turno(tmp_path):
    # O Codex injeta role:"user" com contexto interno (environment_context, AGENTS.md) no
    # começo da thread — mesma regra do parser de chat, senão todo rollout nasce com 3 turnos.
    p = tmp_path / "rollout.jsonl"
    _w(p, [
        _codex_item("2026-08-30T12:00:00.000Z",
                    {"type": "message", "role": "user",
                     "content": [{"type": "input_text",
                                  "text": "<environment_context>cwd</environment_context>"}]}),
        _codex_item("2026-08-30T12:00:00.100Z",
                    {"type": "message", "role": "developer",
                     "content": [{"type": "input_text", "text": "instruções"}]}),
        _codex_item("2026-08-30T12:00:01.000Z",
                    {"type": "message", "role": "user",
                     "content": [{"type": "input_text", "text": "pedido de verdade"}]}),
        _codex_item("2026-08-30T12:00:04.000Z",
                    {"type": "message", "role": "assistant",
                     "content": [{"type": "output_text", "text": "ok"}]}),
        _codex_token_count("2026-08-30T12:00:04.010Z", 10, 0, 5),
    ])
    snap = Accumulator("codex", str(p)).collect()
    assert snap["turns"] == 1
    assert snap["ttft_ms"] == 3000        # medido do pedido real


def test_codex_reasoning_conta_tempo_e_token_count_nao(tmp_path):
    # O token_count é gravado junto com o item que o precede (mesmo instante): contar o gap
    # dele como LLM dobraria o tempo da última chamada.
    p = tmp_path / "rollout.jsonl"
    _w(p, [
        _codex_item("2026-08-30T12:00:00.000Z",
                    {"type": "message", "role": "user",
                     "content": [{"type": "input_text", "text": "oi"}]}),
        _codex_item("2026-08-30T12:00:04.000Z", {"type": "reasoning"}),
        _codex_token_count("2026-08-30T12:00:04.000Z", 10, 0, 5),
        _codex_token_count("2026-08-30T12:00:09.000Z", 30, 0, 9),
    ])
    snap = Accumulator("codex", str(p)).collect()
    assert snap["steps"] == 2             # um por chamada de LLM
    assert snap["llm_ms"] == 4000         # só o gap até o reasoning


def test_codex_token_count_repetido_nao_conta_duas_vezes(tmp_path):
    # Medido nos rollouts de 25/07/2026: um turno abortado grava duas linhas seguidas com o
    # MESMO `last_token_usage`, e uma compactação grava outra com o `last` zerado. Somar os
    # deltas estourava a entrada em exatamente o valor repetido; o acumulado não se move.
    p = tmp_path / "rollout.jsonl"
    _w(p, [
        _codex_item("2026-08-30T12:00:00.000Z",
                    {"type": "message", "role": "user",
                     "content": [{"type": "input_text", "text": "oi"}]}),
        _codex_token_count("2026-08-30T12:00:03.000Z", 1000, 800, 20),
        _codex_token_count("2026-08-30T12:00:04.000Z", 1000, 800, 20),       # turno abortado
        _codex_token_count("2026-08-30T12:00:05.000Z", 1000, 800, 20,
                           last={"input_tokens": 0, "cached_input_tokens": 0,
                                 "output_tokens": 0}),                        # compactação
    ])
    snap = Accumulator("codex", str(p)).collect()
    assert snap["steps"] == 1
    assert snap["in_tok"] == 1000 and snap["out_tok"] == 20


def test_codex_fixture_de_rollout_real():
    fixture = Path(__file__).parent / "fixtures" / "codex" / "rollout_sample.jsonl"
    snap = Accumulator("codex", str(fixture)).collect()
    assert snap["turns"] == 1 and snap["steps"] == 1
    assert snap["in_tok"] == 14371 and snap["out_tok"] == 18
    assert snap["cache_pct"] == 53        # 7552 / 14371
    assert snap["ttft_ms"] > 0 and snap["llm_ms"] > 0


# -- Pi -----------------------------------------------------------------------

def test_pi_usage_e_toolresult(tmp_path):
    p = tmp_path / "session.jsonl"
    _w(p, [
        {"type": "message", "message": {"role": "user", "timestamp": 1000_000,
                                        "content": [{"type": "text", "text": "oi"}]}},
        {"type": "message", "message": {"role": "assistant", "timestamp": 1003_000,
                                        "content": [{"type": "toolCall", "id": "t1"}],
                                        "usage": {"input": 20, "output": 40,
                                                  "cacheRead": 80, "cacheWrite": 0}}},
        {"type": "message", "message": {"role": "toolResult", "timestamp": 1005_000,
                                        "toolCallId": "t1", "content": []}},
    ])
    snap = Accumulator("pi", str(p)).collect()
    assert snap["turns"] == 1 and snap["steps"] == 1
    assert snap["in_tok"] == 100 and snap["cache_pct"] == 80
    assert snap["ttft_ms"] == 3000 and snap["llm_ms"] == 3000
    assert snap["tool_ms"] == 2000


def test_claude_linha_sintetica_avanca_o_cursor(tmp_path):
    # task-notification chegando com a sessão parada: sem avançar o cursor, a resposta
    # seguinte engolia a espera inteira como LLM (32min medidos numa sessão real).
    p = tmp_path / "s.jsonl"
    _w(p, [
        _claude_user("2026-08-18T08:00:00Z", "pedido"),
        _claude_assistant("2026-08-18T08:00:05Z", "m1"),
        _claude_user("2026-08-18T08:30:00Z", "<task-notification>terminou</task-notification>"),
        _claude_assistant("2026-08-18T08:30:04Z", "m2"),
    ])
    snap = Accumulator("claude", str(p)).collect()
    assert snap["llm_ms"] == 9000        # 5s + 4s — os 30min parados ficam de fora


def test_pi_duracao_vem_do_campo_e_tool_desconta_geracao(tmp_path):
    # O Pi grava a linha do assistente no COMEÇO da geração; a duração real vem em
    # _piClaudeStyleThinkingDurationMs. tool_ms começa no fim da geração.
    p = tmp_path / "session.jsonl"
    _w(p, [
        {"type": "message", "message": {"role": "user", "timestamp": 1000_000,
                                        "content": [{"type": "text", "text": "oi"}]}},
        {"type": "message", "message": {"role": "assistant", "timestamp": 1001_000,
                                        "_piClaudeStyleThinkingDurationMs": 7000,
                                        "content": [{"type": "toolCall", "id": "t1"}],
                                        "usage": {"input": 20, "output": 700,
                                                  "cacheRead": 80, "cacheWrite": 0}}},
        {"type": "message", "message": {"role": "toolResult", "timestamp": 1012_000,
                                        "toolCallId": "t1", "content": []}},
    ])
    snap = Accumulator("pi", str(p)).collect()
    assert snap["llm_ms"] == 7000        # do campo, não do gap de 1s
    assert snap["tok_s"] == 100.0        # 700 tok / 7s
    assert snap["tool_ms"] == 4000       # 1012.0 - (1001.0 + 7.0)


def test_provider_sem_fold_retorna_none():
    assert Accumulator.for_provider("opencode", "/x") is None
    assert Accumulator.for_provider("claude", "") is None


def test_sem_steps_snapshot_none(tmp_path):
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_user("2026-08-17T12:00:00Z")])
    assert Accumulator("claude", str(p)).collect() is None


def test_claude_linha_sintetica_nao_e_turno(tmp_path):
    # Eco de /comando, aviso de task e system-reminder chegam como `user` SEM isMeta —
    # não podem virar turno nem resetar o TTFT (achado do review de 17/08/2026).
    p = tmp_path / "s.jsonl"
    _w(p, [
        _claude_user("2026-08-17T12:00:00Z", "pedido de verdade"),
        _claude_user("2026-08-17T12:00:01Z", "<task-notification>agente x terminou</task-notification>"),
        _claude_user("2026-08-17T12:00:01Z", "<command-name>/model</command-name>"),
        _claude_user("2026-08-17T12:00:01Z", "<system-reminder>só lembrete</system-reminder>"),
        _claude_assistant("2026-08-17T12:00:03Z", "m1"),
    ])
    snap = Accumulator("claude", str(p)).collect()
    assert snap["turns"] == 1
    assert snap["ttft_ms"] == 3000       # medido do pedido real, não do sintético


def test_stat_permissao_propaga_arquivo_ausente_nao(tmp_path):
    import pytest
    p = tmp_path / "s.jsonl"
    acc = Accumulator("claude", str(p))
    assert acc.collect() is None         # FileNotFoundError = transitório, sem explodir
    _w(p, [_claude_user("2026-08-17T12:00:00Z"), _claude_assistant("2026-08-17T12:00:02Z", "m1")])
    assert acc.collect()["steps"] == 1
    acc._path = _PathPermissionError(p)
    with pytest.raises(PermissionError):  # OSError persistente PROPAGA (stats_pump loga e desliga)
        acc.collect()


class _PathPermissionError:
    def __init__(self, p):
        self._p = p

    def stat(self):
        raise PermissionError("negado")
