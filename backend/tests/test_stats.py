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


def test_collect_processa_linhas_sem_leitura_integral(tmp_path, monkeypatch):
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_user("2026-08-17T12:00:00Z"),
           _claude_assistant("2026-08-17T12:00:02Z", "m1")])
    original = Path.open

    class _SomenteLinhas:
        def __init__(self, arquivo):
            self._arquivo = arquivo

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return self._arquivo.__exit__(*args)

        def __getattr__(self, nome):
            return getattr(self._arquivo, nome)

        def read(self, *_args, **_kwargs):
            raise AssertionError("collect nao deve carregar o trecho inteiro")

    def abrir(self, *args, **kwargs):
        arquivo = original(self, *args, **kwargs)
        return _SomenteLinhas(arquivo) if self == p else arquivo

    monkeypatch.setattr(Path, "open", abrir)
    assert Accumulator("claude", str(p)).collect()["steps"] == 1


# -- Kimi ---------------------------------------------------------------------


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


def test_codex_recent_rates_exclude_tools_and_repeated_usage(tmp_path):
    p = tmp_path / "codex.jsonl"
    _w(p, [
        _codex_item("2026-10-06T12:00:00Z", {"type": "message", "role": "user",
                                            "content": [{"text": "check"}]}),
        _codex_item("2026-10-06T12:00:03Z", {"type": "function_call", "call_id": "c"}),
        _codex_item("2026-10-06T12:00:08Z", {"type": "function_call_output", "call_id": "c"}),
        _codex_token_count("2026-10-06T12:00:08Z", 100, 0, 60),
        _codex_token_count("2026-10-06T12:00:09Z", 100, 0, 60),
        _codex_item("2026-10-06T12:00:10Z", {"type": "message", "role": "assistant"}),
        _codex_token_count("2026-10-06T12:00:10Z", 200, 0, 140),
    ])
    snap = Accumulator("codex", str(p)).collect()
    assert snap["steps"] == 2 and snap["out_tok"] == 140
    assert snap["tool_ms"] == 5000 and snap["llm_ms"] == 5000
    assert snap["tok_s_now"] == 40.0
    assert snap["tok_s_recent"] == 28.0
    assert "tok_s_exact" not in snap


# -- Pi -----------------------------------------------------------------------


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


# -- Acumulador compartilhado entre conexoes ---------------------------------------------------

def test_compartilhado_nao_rele_o_transcript_para_a_segunda_conexao(tmp_path, monkeypatch):
    # Cada SSE criava um Accumulator novo e relia o arquivo INTEIRO (109-204ms num de 21 MiB, a
    # cada reconexao ou 2o aparelho). Compartilhado por (provider, path), a 2a conexao entra no
    # fold ja feito e so le o que chegou depois.
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_user("2026-08-17T12:00:00Z"), _claude_assistant("2026-08-17T12:00:02Z", "m1")])
    aberturas = []
    original = Path.open

    def contar(self, *a, **k):
        aberturas.append(str(self))
        return original(self, *a, **k)

    monkeypatch.setattr(Path, "open", contar)
    a = Accumulator.compartilhado("claude", str(p))
    assert a.collect()["steps"] == 1
    b = Accumulator.compartilhado("claude", str(p))
    assert b is a
    assert b.collect()["steps"] == 1
    assert len(aberturas) == 1, "a 2a conexao nao pode reabrir o arquivo do zero"
    a.soltar()
    c = Accumulator.compartilhado("claude", str(p))
    assert c is a                                              # b ainda segura
    b.soltar()
    c.soltar()
    d = Accumulator.compartilhado("claude", str(p))
    assert d is a
    assert d.collect()["steps"] == 1
    assert len(aberturas) == 1, "reabrir também deve aproveitar o fold anterior"
    d.soltar()


def test_soltar_acha_a_entrada_mesmo_com_caminho_nao_canonico(tmp_path):
    # `compartilhado` recebia a string crua e `soltar` procurava por `str(Path)`: com `./` ou
    # barra dupla no caminho as chaves divergiam, o refcount ficava negativo e a entrada nunca
    # saia do dicionario.
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_user("2026-08-17T12:00:00Z")])
    torto = f"{tmp_path}/./s.jsonl"
    a = Accumulator.compartilhado("claude", torto)
    a.soltar()
    assert a._donos == 0
    assert ("claude", str(p)) in Accumulator._inativos
    b = Accumulator.compartilhado("claude", str(p))
    assert b is a
    assert ("claude", str(p)) not in Accumulator._inativos
    b.soltar()


def test_retencao_limitada_preserva_ativos(tmp_path, monkeypatch):
    monkeypatch.setattr(Accumulator, "_compartilhados", {})
    monkeypatch.setattr(Accumulator, "_inativos", {})
    monkeypatch.setattr(Accumulator, "_INATIVOS_MAX", 2)
    ativo = Accumulator.compartilhado("claude", str(tmp_path / "ativo"))
    antigos = [Accumulator.compartilhado("claude", str(tmp_path / str(i))) for i in range(3)]
    for acc in antigos:
        acc.soltar()
    assert list(Accumulator._compartilhados.values()) == [ativo, *antigos[1:]]
    assert len(Accumulator._inativos) == 2


def test_reabrir_processa_append_e_substituicao(tmp_path):
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_assistant("2026-08-17T12:00:02Z", "m1")])
    acc = Accumulator.compartilhado("claude", str(p))
    assert acc.collect()["steps"] == 1
    acc.soltar()
    _w(p, [_claude_assistant("2026-08-17T12:00:05Z", "m2")])
    novo = Accumulator.compartilhado("claude", str(p))
    assert novo is acc
    assert novo.collect()["steps"] == 2
    novo.soltar()
    substituto = tmp_path / "novo.jsonl"
    _w(substituto, [_claude_assistant("2026-08-17T12:00:05Z", f"replacement-{i}")
                   for i in range(3)])
    substituto.replace(p)
    reaberto = Accumulator.compartilhado("claude", str(p))
    assert reaberto.collect()["steps"] == 3
    reaberto.soltar()


def test_compartilhado_por_provider_e_caminho(tmp_path):
    p = tmp_path / "s.jsonl"
    _w(p, [_claude_user("2026-08-17T12:00:00Z")])
    a = Accumulator.compartilhado("claude", str(p))
    assert Accumulator.compartilhado("claude", str(tmp_path / "outro.jsonl")) is not a
    assert Accumulator.compartilhado("inexistente", str(p)) is None
