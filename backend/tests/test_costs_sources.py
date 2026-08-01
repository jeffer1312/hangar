import json
import os
from datetime import timezone

import pytest

from app import costs_sources as cs
from app import pricing


@pytest.fixture(autouse=True)
def _pricing_isolado(tmp_path, monkeypatch):
    """Os leitores resolvem PROVEDOR via pricing, então sem isto o teste lê o cache real em
    ~/.claude/.claude-pocket-pricing/ e passa ou falha conforme o estado da máquina — não do
    código. Mesma fixture do test_pricing.py."""
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    # getattr: `cs.invalidar_cache` só nasce na Task 6; até lá o no-op mantém a fixture válida
    # desde a Task 3, sem precisar reescrevê-la depois.
    limpar = lambda: getattr(cs, "invalidar_cache", lambda: None)()  # noqa: E731
    pricing.invalidar_cache(); limpar()
    yield
    pricing.invalidar_cache(); limpar()


def _escrever(p, linhas):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) if isinstance(x, (dict, list)) else x for x in linhas))


def test_ler_jsonl_pula_linha_invalida_e_nao_dict(tmp_path):
    p = tmp_path / "x.jsonl"
    _escrever(p, [{"a": 1}, "{quebrado", "null", "[1,2]", {"b": 2}])
    assert list(cs._ler_jsonl(p)) == [{"a": 1}, {"b": 2}]


def test_cwd_sai_de_dentro_do_transcript(tmp_path):
    # O nome do diretório do Claude não é invertível ('Á' e espaço viram '-', igual à barra).
    # O cwd real está DENTRO do arquivo.
    t = tmp_path / "-home-jeff--rea-de-trabalho-x" / "s.jsonl"
    _escrever(t, [{"type": "last-prompt"}, {"type": "x"},
                  {"type": "user", "cwd": "/home/jeff/Área de trabalho/x"}])
    assert cs.cwd_do_transcript(str(t)) == "/home/jeff/Área de trabalho/x"


def test_cwd_ausente_vira_string_vazia(tmp_path):
    t = tmp_path / "s.jsonl"
    _escrever(t, [{"type": "last-prompt"}])
    assert cs.cwd_do_transcript(str(t)) == ""


def test_claude_pega_a_ultima_linha_por_sessao(tmp_path):
    # O costs.jsonl é CUMULATIVO: o hook grava um snapshot do total a cada turno.
    cfg = tmp_path / ".claude"
    t = cfg / "projects" / "-p" / "s1.jsonl"
    _escrever(t, [{"type": "user", "cwd": "/repo/um"}])
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s1", "transcript_path": str(t),
         "model": "claude-opus-5", "input_tokens": 10, "output_tokens": 1,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
        {"timestamp": "2026-08-01T11:00:00Z", "session_id": "s1", "transcript_path": str(t),
         "model": "claude-opus-5", "input_tokens": 99, "output_tokens": 9,
         "cache_write_tokens": 2, "cache_read_tokens": 3},
    ])
    linhas = cs.linhas_claude(cfg, "conta-x")
    assert len(linhas) == 1
    r = linhas[0]
    assert (r.input, r.output, r.cache_write, r.cache_read) == (99, 9, 2, 3)
    assert r.source == "claude"
    assert r.project == "/repo/um"
    assert r.provider == "conta-x"


def test_claude_ignora_synthetic(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "<synthetic>",
         "input_tokens": 1, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "conta-x") == []


def test_sessao_de_motor_ganha_provedor_do_modelo(tmp_path):
    # CP_ENGINE só existe em /proc de sessão VIVA. Numa linha de ontem, quem entrega o
    # provedor é o modelo: 'k3' só existe na Moonshot.
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "k3",
         "input_tokens": 5, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "conta-x")[0].provider == "moonshotai"


def test_transcript_sem_cwd_vira_projeto_desconhecido(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "transcript_path": "/nao/existe",
         "model": "claude-opus-5", "input_tokens": 1, "output_tokens": 1,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "c")[0].project == cs.PROJETO_DESCONHECIDO


def test_timestamp_vira_tz_aware(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "claude-opus-5",
         "input_tokens": 1, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    ts = cs.linhas_claude(cfg, "c")[0].ts
    assert ts.tzinfo is not None
    assert ts.astimezone(timezone.utc).hour == 10
