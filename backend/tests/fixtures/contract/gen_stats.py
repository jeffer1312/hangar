"""Caracterização da faixa `stats` (app/stats.py) para o acumulador do Rust (`crates/hangar-server/src/stats.rs`).

Cada caso é uma sequência de passos sobre um arquivo; depois de cada passo o `Accumulator` do Python
roda `collect()` e o retrato vai para a fixture. As entradas são os transcripts já versionados no repo e
os formatos de `tests/test_stats.py`.

Uso, de backend/: uv run python tests/fixtures/contract/gen_stats.py
"""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "stats" / "cases.json"
TESTS = HERE.parents[1]
sys.path.insert(0, str(TESTS.parent))

from app.stats import Accumulator  # noqa: E402


def _line(obj) -> str:
    return json.dumps(obj) + "\n"


def _cu(ts, text="oi", **extra):
    return {"type": "user", "timestamp": ts, "message": {"role": "user", "content": text}, **extra}


def _ca(ts, mid, out=100, blocks=None, **extra):
    return {"type": "assistant", "timestamp": ts, **extra,
            "message": {"id": mid, "role": "assistant", "content": blocks or [{"type": "text", "text": "resp"}],
                        "usage": {"input_tokens": 10, "output_tokens": out, "cache_read_input_tokens": 80,
                                  "cache_creation_input_tokens": 10}}}


def _cr(ts, tool_id):
    return {"type": "user", "timestamp": ts,
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_id, "content": "ok"}]}}


def _xi(ts, payload):
    return {"timestamp": ts, "type": "response_item", "payload": payload}


def _xt(ts, inp, cached, out):
    return {"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count", "info": {
        "total_token_usage": {"input_tokens": inp, "cached_input_tokens": cached, "output_tokens": out}}}}


def _xu(ts, text):
    return _xi(ts, {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})


def _w(*objs) -> dict:
    return {"op": "append", "data": "".join(_line(o) for o in objs)}


SYNTHETIC = {
    "claude_dedup_ttft": ("claude", [_w(_cu("2026-08-17T12:00:00Z"), _ca("2026-08-17T12:00:02Z", "m1"),
                                        _ca("2026-08-17T12:00:03Z", "m1"))]),
    "claude_tool_and_llm": ("claude", [_w(
        _cu("2026-08-17T12:00:00Z"),
        _ca("2026-08-17T12:00:02Z", "m1", blocks=[{"type": "tool_use", "id": "t1", "name": "Bash"}]),
        _cr("2026-08-17T12:00:07Z", "t1"), _ca("2026-08-17T12:00:10Z", "m2"))]),
    "claude_sidechain": ("claude", [_w(_cu("2026-08-17T12:00:00Z"),
                                       _ca("2026-08-17T12:00:01Z", "sub1", isSidechain=True))]),
    "claude_incremental_and_truncate": ("claude", [
        _w(_cu("2026-08-17T12:00:00Z"), _ca("2026-08-17T12:00:02Z", "m1")),
        _w(_ca("2026-08-17T12:00:05Z", "m2")),
        {"op": "write", "data": _line(_cu("2026-08-17T13:00:00Z")) + _line(_ca("2026-08-17T13:00:01Z", "x1"))}]),
    "claude_partial_line": ("claude", [
        {"op": "append", "data": _line(_ca("2026-08-17T12:00:02Z", "m1")) + _line(_ca("2026-08-17T12:00:05Z", "m2"))[:40]},
        {"op": "append", "data": _line(_ca("2026-08-17T12:00:05Z", "m2"))[40:]}]),
    "claude_synthetic_lines_move_cursor": ("claude", [_w(
        _cu("2026-08-18T08:00:00Z", "pedido"), _ca("2026-08-18T08:00:05Z", "m1"),
        _cu("2026-08-18T08:30:00Z", "<task-notification>terminou</task-notification>"),
        _ca("2026-08-18T08:30:04Z", "m2"))]),
    "claude_synthetic_lines_not_turns": ("claude", [_w(
        _cu("2026-08-17T12:00:00Z", "pedido de verdade"),
        _cu("2026-08-17T12:00:01Z", "<task-notification>agente x terminou</task-notification>"),
        _cu("2026-08-17T12:00:01Z", "<command-name>/model</command-name>"),
        _cu("2026-08-17T12:00:01Z", "<system-reminder>só lembrete</system-reminder>"),
        _ca("2026-08-17T12:00:03Z", "m1"))]),
    "claude_no_steps": ("claude", [_w(_cu("2026-08-17T12:00:00Z"))]),
    "claude_missing_then_created": ("claude", [
        {"op": "none"}, _w(_cu("2026-08-17T12:00:00Z"), _ca("2026-08-17T12:00:02Z", "m1"))]),
    "claude_meta_peer_and_reserved_call": ("claude", [_w(
        _cu("2026-10-06T12:00:00Z", "pedido"),
        _ca("2026-10-06T12:00:04Z", "m1", out=200, blocks=[{"type": "tool_use", "id": "t1", "name": "Bash"}]),
        _cr("2026-10-06T12:00:06Z", "t1"),
        _ca("2026-10-06T12:00:09Z", "m1", out=0),
        _cu("2026-10-06T12:10:00Z", "[de: outra] oi", isMeta=True),
        _ca("2026-10-06T12:10:03Z", "m2", out=90),
        {"type": "assistant", "timestamp": "2026-10-06T12:10:04Z", "isMeta": True,
         "message": {"id": "meta", "usage": {"output_tokens": 5}}},
        {"type": "system", "timestamp": "2026-10-06T12:10:05Z"},
        "não é objeto",
        _ca("2026-10-06T12:10:06Z", "m3", out=0),
        _ca(None, "m4", out=30),
        _ca("2026-10-06T12:10:07Z", None, out=30))]),
    "claude_replaced_same_path": ("claude", [
        _w(_ca("2026-08-17T12:00:02Z", "m1")),
        {"op": "replace", "data": "".join(_line(_ca("2026-08-17T12:00:05Z", f"replacement-{i}")) for i in range(3))}]),
    "claude_huge_gap_and_bad_types": ("claude", [_w(
        _cu("2026-08-17T10:00:00Z"),
        _ca("2026-08-17T12:00:00Z", "m1"),
        {"type": "assistant", "timestamp": "2026-08-17T12:00:03+00:00",
         "message": {"id": "m2", "usage": {"input_tokens": True, "output_tokens": 4.0, "cache_read_input_tokens": "9"}}},
        {"type": "assistant", "timestamp": "2026-08-17T12:00:04", "message": {"id": "m3", "usage": {"output_tokens": 7}}},
        {"type": "assistant", "timestamp": "não é data", "message": {"id": "m4", "usage": {"output_tokens": 7}}})]),
    "codex_usage_llm_tool": ("codex", [_w(
        _xu("2026-08-30T12:00:00.000Z", "oi"),
        _xi("2026-08-30T12:00:02.000Z", {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "vou"}]}),
        _xi("2026-08-30T12:00:03.000Z", {"type": "custom_tool_call", "call_id": "c1", "name": "exec"}),
        _xi("2026-08-30T12:00:08.000Z", {"type": "custom_tool_call_output", "call_id": "c1", "output": "ok"}),
        _xt("2026-08-30T12:00:08.010Z", 400, 300, 50))]),
    "codex_context_not_turn": ("codex", [_w(
        _xu("2026-08-30T12:00:00.000Z", "<environment_context>cwd</environment_context>"),
        _xi("2026-08-30T12:00:00.100Z", {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "x"}]}),
        _xu("2026-08-30T12:00:01.000Z", "pedido de verdade"),
        _xi("2026-08-30T12:00:04.000Z", {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}),
        _xt("2026-08-30T12:00:04.010Z", 10, 0, 5))]),
    "codex_reasoning": ("codex", [_w(
        _xu("2026-08-30T12:00:00.000Z", "oi"), _xi("2026-08-30T12:00:04.000Z", {"type": "reasoning"}),
        _xt("2026-08-30T12:00:04.000Z", 10, 0, 5), _xt("2026-08-30T12:00:09.000Z", 30, 0, 9))]),
    "codex_repeated_and_compacted": ("codex", [_w(
        _xu("2026-08-30T12:00:00.000Z", "oi"), _xt("2026-08-30T12:00:03.000Z", 1000, 800, 20),
        _xt("2026-08-30T12:00:04.000Z", 1000, 800, 20), _xt("2026-08-30T12:00:05.000Z", 1000, 800, 20),
        _xt("2026-08-30T12:00:06.000Z", 900, 700, 10), _xt("2026-08-30T12:00:07.000Z", 950, 700, 30))]),
    "codex_recent_rates": ("codex", [_w(
        _xi("2026-10-06T12:00:00Z", {"type": "message", "role": "user", "content": [{"text": "check"}]}),
        _xi("2026-10-06T12:00:03Z", {"type": "function_call", "call_id": "c"}),
        _xi("2026-10-06T12:00:08Z", {"type": "function_call_output", "call_id": "c"}),
        _xt("2026-10-06T12:00:08Z", 100, 0, 60), _xt("2026-10-06T12:00:09Z", 100, 0, 60),
        _xi("2026-10-06T12:00:10Z", {"type": "message", "role": "assistant"}),
        _xt("2026-10-06T12:00:10Z", 200, 0, 140))]),
}

REAL = {
    "claude": ["fixtures/contract/transcripts/claude.jsonl", "fixtures/contract/queue/claude-fixture.jsonl",
               "fixtures/bastao_hangar2_recorte.jsonl", "fixtures/jsonl_samples.jsonl",
               "fixtures/contract/costs/claude/projects/-repo-a/s1.jsonl",
               "fixtures/contract/costs/claude/projects/-repo-a/s1/subagents/agent-ab12.jsonl",
               *sorted(f"fixtures/claude_to_codex/{p.name}" for p in (TESTS / "fixtures/claude_to_codex").glob("*.jsonl"))],
    "codex": ["fixtures/contract/transcripts/codex.jsonl", "fixtures/contract/queue/codex-fixture.jsonl",
              "fixtures/codex/rollout_sample.jsonl", "fixtures/codex/rollout_exec.jsonl",
              "fixtures/contract/costs/codex/sessions/2026/09/30/rollout-c1.jsonl"],
}


def _real_cases() -> dict:
    cases = {}
    for provider, files in REAL.items():
        for rel in files:
            try:
                data = (TESTS / rel).read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue   # a fixture guarda o passo como texto JSON; bytes inválidos não cabem nela
            stem = Path(rel).stem.replace("-", "_")
            cases[f"real_{provider}_{stem}"] = (provider, [{"op": "append", "data": data}])
            # Em três pedaços, com corte no meio de uma linha: a leitura incremental tem de dar o mesmo.
            a, b = len(data) // 3, 2 * len(data) // 3
            cases[f"real_{provider}_{stem}_chunks"] = (provider, [
                {"op": "append", "data": data[:a]}, {"op": "append", "data": data[a:b]}, {"op": "append", "data": data[b:]}])
    return cases


def _apply(path: Path, step: dict) -> None:
    op = step["op"]
    if op == "append":
        with path.open("a", encoding="utf-8") as f:
            f.write(step["data"])
    elif op == "write":
        path.write_text(step["data"], encoding="utf-8")
    elif op == "replace":
        other = path.with_name("substituto.jsonl")
        other.write_text(step["data"], encoding="utf-8")
        other.replace(path)


def main() -> None:
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    cases = []
    for name, (provider, steps) in {**SYNTHETIC, **_real_cases()}.items():
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "s.jsonl"
            acc = Accumulator(provider, str(path))
            out = []
            for step in steps:
                _apply(path, step)
                out.append({**step, "snapshot": acc.collect(), "last_call_ts": acc.last_call_ts()})
            cases.append({"name": name, "provider": provider, "steps": out})
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"commit": commit, "command": "uv run python tests/fixtures/contract/gen_stats.py",
                               "cases": cases}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(cases)} casos -> {OUT}")


if __name__ == "__main__":
    main()
