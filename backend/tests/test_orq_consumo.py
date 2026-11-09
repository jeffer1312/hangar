"""Medição por intervalo usa os acumuladores reais, em transcripts descartáveis."""
import copy
import json
import subprocess
import sys

import pytest

from app.orq_consumo import report, snapshot
from app.stats import Accumulator


def _write(path, rows):
    with path.open("a", encoding="utf-8") as stream:
        for row in rows:
            stream.write(json.dumps(row) + "\n")


def _usage(inp, cached, out):
    return {"type": "event_msg", "payload": {"type": "token_count", "info": {
        "total_token_usage": {"input_tokens": inp, "cached_input_tokens": cached,
                              "output_tokens": out}}}}


def _pair(tmp_path):
    path = tmp_path / "rollout.jsonl"
    _write(path, [_usage(100, 80, 10)])
    start = snapshot("codex", str(path), "revisor", "rev-t1")
    _write(path, [_usage(150, 110, 30), _usage(150, 110, 30)])
    end = snapshot("codex", str(path), "revisor", "rev-t1")
    return path, start, end


def test_cli_exclui_historico_e_deduplica_codex(tmp_path):
    path = tmp_path / "rollout.jsonl"
    _write(path, [_usage(100, 80, 10)])
    command = [sys.executable, "-m", "app.orq_consumo"]
    capture = [*command, "snapshot", "--provider", "codex", "--transcript", str(path),
               "--role", "revisor", "--session", "rev-t1"]
    start = subprocess.run(capture, text=True, capture_output=True, check=True)
    _write(path, [_usage(150, 110, 30), _usage(150, 110, 30)])
    end = subprocess.run(capture, text=True, capture_output=True, check=True)
    inputs = []
    for name, result in (("inicio", start), ("fim", end)):
        artifact = tmp_path / f"{name}.json"
        artifact.write_text(result.stdout, encoding="utf-8")
        inputs.append(str(artifact))
    compared = subprocess.run([*command, "report", "--pair", *inputs],
                              text=True, capture_output=True, check=True)
    result = json.loads(compared.stdout)
    assert result["by_role"]["revisor"]["tokens"] == {
        "input": 20, "cache_read": 30, "cache_write": 0, "output": 20}
    assert result["coverage"] == "somente_fontes_registradas"
    missing = subprocess.run([*capture[:-1], "rev-t2", "--transcript", str(tmp_path / "ausente")],
                             text=True, capture_output=True)
    assert missing.returncode == 1 and not missing.stdout
    assert "Consumo não medido" in missing.stderr


def test_baseline_sem_uso_e_cache_exato_das_fontes(tmp_path):
    examples = {
        "claude": {"type": "assistant", "message": {"id": "m1", "usage": {
            "input_tokens": 10, "output_tokens": 7, "cache_read_input_tokens": 80,
            "cache_creation_input_tokens": 5}}},
    }
    for provider, usage in examples.items():
        path = tmp_path / f"{provider}.jsonl"
        path.touch()
        start = snapshot(provider, str(path), "executor", provider)
        assert start["coverage"] == "sem_uso_observado"
        _write(path, [usage, usage] if provider == "claude" else [usage])
        end = snapshot(provider, str(path), "executor", provider)
        result = report([(start, end)])
        assert result["by_role"]["executor"]["tokens"] == {
            "input": 10, "output": 7, "cache_read": 80, "cache_write": 5}


def test_recusa_sobreposicao_inclusive_outro_provider(tmp_path):
    _, start, end = _pair(tmp_path)
    with pytest.raises(ValueError, match="sobrepostos"):
        report([(start, end), (start, end)])


@pytest.mark.parametrize("change", ["reescrita", "truncada", "substituida", "ausente"])
def test_fonte_alterada_nao_vira_consumo(tmp_path, change):
    path, start, end = _pair(tmp_path)
    if change == "reescrita":
        text = path.read_text()
        path.write_text(text.replace('100', '101', 1))
    elif change == "truncada":
        path.write_text("")
    elif change == "substituida":
        replacement = tmp_path / "replacement"
        replacement.write_bytes(path.read_bytes())
        replacement.replace(path)
    else:
        path.unlink()
    with pytest.raises((ValueError, OSError)):
        report([(start, end)])


def test_contador_regressivo_e_vinculo_incoerente(tmp_path):
    _, start, end = _pair(tmp_path)
    for key, value in (("role", "executor"), ("session", "outro")):
        altered = copy.deepcopy(end)
        altered[key] = value
        with pytest.raises(ValueError, match="vínculos diferentes"):
            report([(start, altered)])
    altered = copy.deepcopy(end)
    altered["totals"]["output"] = 0
    with pytest.raises(ValueError, match="regrediram"):
        report([(start, altered)])


def test_sem_uso_novo_e_linha_parcial_nao_viram_zero_comprovado(tmp_path):
    path, _, start = _pair(tmp_path)
    end = snapshot("codex", str(path), "revisor", "rev-t1")
    with pytest.raises(ValueError, match="Sem uso novo"):
        report([(start, end)])
    with path.open("ab") as stream:
        stream.write(b'{"type":"event_msg"')
    with pytest.raises(ValueError, match="linha parcial"):
        snapshot("codex", str(path), "revisor", "rev-t1")


@pytest.mark.parametrize("invalid", ["{quebrado}", "null", "[]"])
def test_linha_invalida_bloqueia_medicao_mas_preserva_sse(tmp_path, invalid):
    path = tmp_path / "rollout.jsonl"
    _write(path, [_usage(100, 80, 10)])
    with path.open("a") as stream:
        stream.write(invalid + "\n")
    _write(path, [_usage(150, 110, 30)])
    accumulator = Accumulator("codex", str(path))
    assert accumulator.collect()["out_tok"] == 30
    with pytest.raises(ValueError, match="linhas inválidas"):
        accumulator.usage_totals()
    with pytest.raises(ValueError, match="linhas inválidas"):
        snapshot("codex", str(path), "revisor", "rev-t1")
    path.write_text("")
    _write(path, [_usage(20, 10, 5)])
    assert accumulator.usage_totals()["output"] == 5


def test_intervalos_seguidos_na_mesma_fonte_nao_duplicam(tmp_path):
    path, start, middle = _pair(tmp_path)
    _write(path, [_usage(160, 110, 40)])
    end = snapshot("codex", str(path), "revisor", "rev-t1")
    result = report([(start, middle), (middle, end)])
    assert result["by_role"]["revisor"]["tokens"] == {
        "input": 30, "cache_read": 30, "cache_write": 0, "output": 30}


def test_reset_do_total_codex_nao_produz_medicao_exata(tmp_path):
    path = tmp_path / "rollout.jsonl"
    _write(path, [_usage(100, 80, 10), _usage(50, 40, 5), _usage(160, 110, 30)])
    accumulator = Accumulator("codex", str(path))
    assert accumulator.collect()["in_tok"] == 210
    with pytest.raises(ValueError, match="regrediram"):
        accumulator.usage_totals()
