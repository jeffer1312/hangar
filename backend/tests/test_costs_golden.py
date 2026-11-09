"""Prende o contrato Rust aos leitores Python sem reescrever os golden versionados."""
import json
import runpy
from pathlib import Path, PurePosixPath, PureWindowsPath

import pytest

CONTRACT = Path(__file__).parent / "fixtures" / "contract"


def _golden(name):
    return json.loads((CONTRACT / "golden" / name).read_text(encoding="utf-8"))


@pytest.mark.parametrize("base", [PurePosixPath("/tmp/fixture/inteiro"),
                                 PureWindowsPath(r"C:\Temp\fixture\inteiro"),
                                 PureWindowsPath(r"D:\Temporário\outra-fixture\inteiro")])
def test_costs_reports_normalize_codex_identity_across_platforms(base):
    normalize = runpy.run_path(str(CONTRACT / "gen_costs.py"))["_normalize_reports"]
    identity = "codex:" + str(base / "codex")
    session = ["codex", identity, "c1", False]
    nested = json.dumps({"session_ids": [json.dumps(session)]}, ensure_ascii=False)
    report = {"provider": identity, "buckets": [{"key": identity, "session_ids": [json.dumps(session)]}],
              "nested": nested, "amounts": [1.25, 0, None], "text": "Texto sintético sem identidade."}
    expected_session = ["codex", "codex:__BASE__/codex", "c1", False]
    expected = {"provider": "codex:__BASE__/codex",
                "buckets": [{"key": "codex:__BASE__/codex", "session_ids": [json.dumps(expected_session)]}],
                "nested": json.dumps({"session_ids": [json.dumps(expected_session)]}, ensure_ascii=False),
                "amounts": [1.25, 0, None], "text": "Texto sintético sem identidade."}
    original = json.dumps(report, ensure_ascii=False)
    assert normalize(report, base) == expected
    assert json.dumps(report, ensure_ascii=False) == original




def test_costs_index_exercises_named_cases():
    from app import costs, costs_cache  # noqa: F401

    index = _golden("costs_index.json")
    resumed = index.pop("__resumed__")
    assert resumed == index
    assert len(index) == 11

    def rows(path, kind):
        fields = costs_cache.CAMPOS_CUSTO if kind == "custo" else costs_cache.CAMPOS_USO
        return [dict(zip(fields, row, strict=True)) for row in index[path][kind]]

    claude = "claude/projects/-repo-a/"
    first, compacted = rows(claude + "s1.jsonl", "custo")
    assert (first["input"], first["output"], first["cache_write"], first["cache_read"]) == (140, 35, 200, 1500)
    assert first["ts"] == "2026-09-29T23:30:06-03:00"
    assert (first["cache_write_1h"], first["fast"], compacted["regravado"]) == (80, 1, 0)
    assert first["project"] == compacted["project"] == "/repo/a"
    usage = rows(claude + "s1.jsonl", "uso")
    skills = [row for row in usage if row["tipo"] == "skill"]
    assert {(row["nome"], row["origem"]) for row in skills} == {
        ("brainstorming", "voce"), ("brainstorming", "compactacao"), ("minha-skill", "leitura"), ("database-x", "modelo")}
    assert next(row for row in skills if row["nome"] == "minha-skill")["ctx_chars"] == 254
    assert next(row for row in skills if row["origem"] == "compactacao")["respostas"] == 1
    hooks = [row for row in usage if row["tipo"] == "contexto"]
    assert len(hooks) == 1 and hooks[0]["chamadas"] == 1 and hooks[0]["ctx_chars"] == 44
    assert next(row for row in usage if row["tipo"] == "imagem")["tokens_est"] == 240
    assert next(row for row in usage if row["tipo"] == "bash")["nome"] == "cat"
    assert any(row["tipo"] == "agente" and row["detalhe"] == "ab12" for row in usage)
    first_areas = rows(claude + "s1.jsonl", "areas")
    assert {row["nome"] for row in first_areas if row["dia"] == "2026-09-29"} == {"outros", "back", "front"}
    assert {row["nome"] for row in first_areas if row["dia"] == "2026-09-30"} == {"back", "banco"}
    child = rows(claude + "s1/subagents/agent-ab12.jsonl", "custo")[0]
    assert child["subagente"] == 1 and child["input"] == 12
    assert all(row["subagente"] == 1 for row in rows(claude + "s1/subagents/agent-ab12.jsonl", "uso"))
    expired, after = rows(claude + "s2.jsonl", "custo")
    assert (expired["regravado"], expired["regravado_1h"], after["regravado"]) == (800, 400, 0)
    second_usage = rows(claude + "s2.jsonl", "uso")
    assert any(row["tipo"] == "mcp" and row["detalhe"] == "mcp__srv__tool" for row in second_usage)
    assert any(row["tipo"] == "agente" and row["origem"] == "pedido" for row in second_usage)
    assert not any(row["nome"] == "IgnoredSynthetic" for row in second_usage)

    damaged_path = "claude/projects/-repo-b/s3.jsonl"
    damaged = (CONTRACT / "costs" / damaged_path).read_bytes()
    assert b"\\ud800" in damaged and b"\xff" in damaged and b"\nnull\n[1]\n" in damaged
    assert damaged.endswith(b'{"type":"assistant","message":{"usage":')
    engine = rows(damaged_path, "custo")
    assert len(engine) == 1
    assert (engine[0]["input"], engine[0]["output"], engine[0]["model"], engine[0]["project"]) == (100, 20, "gpt-5.6-sol", "/repo/b")

    codex = "codex/sessions/2026/09/30/"
    normal = rows(codex + "rollout-c1.jsonl", "custo")[0]
    assert (normal["input"], normal["output"], normal["cache_write"], normal["cache_read"]) == (1000, 140, 100, 300)
    codex_usage = rows(codex + "rollout-c1.jsonl", "uso")
    assert {row["nome"] for row in codex_usage if row["tipo"] == "tool"} == {
        "exec_command", "apply_patch", "view_image", "spawn_agent"}
    assert next(row for row in codex_usage if row["tipo"] == "skill")["respostas"] == 1
    assert next(row for row in codex_usage if row["tipo"] == "imagem")["nome"] == "lida:view_image"
    assert {row["nome"] for row in rows(codex + "rollout-c1.jsonl", "areas")} == {"back", "front"}
    fork = rows(codex + "rollout-c2.jsonl", "custo")[0]
    assert (fork["input"], fork["output"], fork["cache_read"], fork["project"], fork["session_id"]) == (120, 15, 30, "/repo/fork", "c2")
    assert not any(row["nome"] == "InheritedTool" for row in rows(codex + "rollout-c2.jsonl", "uso"))
    assert all(row["cwd"] == "/repo/fork" for row in rows(codex + "rollout-c2.jsonl", "areas"))
    long = rows(codex + "rollout-c3.jsonl", "custo")[0]
    assert (long["input"], long["cache_write"], long["cache_read"], long["codex_long_context"], long["subagente"]) == (275000, 5000, 20000, 1, 1)

    pi = rows("pi/--repo--/2026-09-30_s.jsonl", "custo")[0]
    assert (pi["input"], pi["output"], pi["cache_write"], pi["cache_read"], pi["provider"]) == (30, 10, 15, 50, "moonshotai")
    pi_child_path = "pi/--repo--/2026-09-30_s/t1/run-1/session.jsonl"
    pi_child = rows(pi_child_path, "custo")[0]
    assert pi_child["session_id"] == "--repo--/2026-09-30_s/t1/run-1/session"
    assert (pi_child["model"], pi_child["provider"], pi_child["input"]) == ("deepseek/deepseek-v4-flash", "openrouter", 50)
    kimi = "kimi/sessions/wd_x/session_k1/agents/"
    main = rows(kimi + "main/wire.jsonl", "custo")[0]
    kimi_child = rows(kimi + "agent-1/wire.jsonl", "custo")[0]
    assert (main["input"], main["output"], main["cache_write"], main["cache_read"]) == (40, 15, 15, 80)
    assert main["ts"] == "2026-09-30T09:00:01.123000-03:00"
    assert (kimi_child["subagente"], kimi_child["input"], kimi_child["model"]) == (1, 8, "apikey/k3")
    for path in ("pi/--repo--/2026-09-30_s.jsonl", pi_child_path, kimi + "main/wire.jsonl", kimi + "agent-1/wire.jsonl"):
        assert index[path]["uso"] == index[path]["areas"] == []


def test_costs_pricing_areas_and_filtered_reports():
    prices = {row["model"]: row for row in _golden("costs_pricing.json")}
    assert len(prices) == 13
    assert prices["anthropic/claude-opus-5"]["canon"] == prices["Claude-Opus-5"]["canon"] == "claude-opus-5"
    assert prices["claude-haiku-4.5"]["canon"] == "claude-haiku-4-5-20251001"
    assert prices["k3"]["canon"] == prices["apikey/k3"]["canon"] == "kimi-k3"
    assert prices["openrouter/deepseek/deepseek-v4-flash"]["canon"] == "deepseek-v4-flash"
    assert prices["claude-opus-5"]["fast"]["input"] == 10
    assert prices["gpt-5.6-sol"]["codex_long"]["output"] == 22.5
    assert prices["gpt-5.5"]["rate"]["cache_write"] == 0
    assert prices["k3"]["rate"]["cache_estimado"] is True
    assert prices["modelo-caro"]["rate"]["origin"] == "override"
    for model in ("<synthetic>", "", "nao-existe", "openrouter/deepseek/deepseek-v4-flash"):
        assert prices[model]["rate"] is prices[model]["fast"] is prices[model]["codex_long"] is None
    areas = _golden("costs_areas.json")
    assert {key: len(value) for key, value in areas.items()} == {
        "comando_bash": 8, "candidatos": 8, "skill_do_caminho": 6, "repartir": 4, "area_do_alvo": 8}
    for value, _weights, result in areas["repartir"]:
        assert sum(result.values()) == value
    assert dict(areas["area_do_alvo"])["skill:database-x"] == "banco"
    reports = _golden("costs_reports.json")
    assert reports["now"] == "2026-10-01T12:00:00-03:00"
    assert reports["costs"]["all"]["totals"]["regravado"] == 800
    assert reports["costs"]["7d"]["totals"]["regravado"] == 0
    assert reports["costs"]["all"]["sem_tarifa"] == ["deepseek-v4-flash"]
    assert any(row["project"] == "/repo/k" for row in reports["costs"]["all"]["sessoes"] if row["source"] == "kimi")
    usage = reports["uso"]
    assert usage["conta"]["conta"] == ["anthropic:u-fixture"]
    assert usage["projeto"]["projeto"] == ["/repo/a"]
    assert usage["foco_skill"]["foco"] == "brainstorming"
    assert usage["foco_area"]["foco"] == "back"
    for report in [*reports["costs"].values(), *usage.values()]:
        assert report["usd_brl"] is None
    for key in ("all", "conta", "projeto", "foco_skill", "foco_area"):
        assert usage[key]["totals"]["sessions"] > 0
