import json
from datetime import datetime, timedelta
from pathlib import Path

from app import costs


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(r) for r in rows) + "\n")


def test_rates_for_matches_by_substring():
    assert costs.rates_for("claude-opus-4-8")["i"] == 5.0
    assert costs.rates_for("claude-sonnet-5")["o"] == 15.0
    assert costs.rates_for("claude-haiku-4-5")["i"] == 1.0
    assert costs.rates_for("claude-fable-5")["o"] == 50.0
    assert costs.rates_for(None)["i"] == 3.0  # fallback sonnet


def test_load_dedups_cumulative_snapshots_by_session_id(tmp_path):
    src = tmp_path / "metrics" / "costs.jsonl"
    _write_jsonl(src, [
        {"timestamp": "2026-07-01T10:00:00.000Z", "session_id": "s1",
         "model": "claude-opus-4-8", "input_tokens": 100, "output_tokens": 10,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
        {"timestamp": "2026-07-01T10:05:00.000Z", "session_id": "s1",
         "model": "claude-opus-4-8", "input_tokens": 200, "output_tokens": 20,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    rows = costs._load(tmp_path)
    assert len(rows) == 1                 # 2 snapshots da mesma sessao = 1
    assert rows[0]["in"] == 200           # ultima linha vence
    assert rows[0]["out"] == 20


def test_load_missing_file_returns_empty(tmp_path):
    assert costs._load(tmp_path) == []


def test_load_skips_invalid_lines(tmp_path):
    src = tmp_path / "metrics" / "costs.jsonl"
    src.parent.mkdir(parents=True)
    src.write_text('nao-e-json\n{"timestamp":"2026-07-01T10:00:00Z","session_id":"s1",'
                   '"model":"claude-opus-4-8","input_tokens":1,"output_tokens":0,'
                   '"cache_write_tokens":0,"cache_read_tokens":0}\n')
    rows = costs._load(tmp_path)
    assert len(rows) == 1


def test_cost_applies_per_model_rates():
    # 1M input opus = $5, 1M output opus = $25
    row = {"dt": None, "model": "claude-opus-4-8",
           "in": 1_000_000, "out": 1_000_000, "cw": 0, "cr": 0}
    assert costs._cost(row) == 30.0


def test_bucket_sums_and_orders():
    rows = [
        {"dt": datetime(2026, 7, 1), "model": "claude-opus-4-8",
         "in": 1_000_000, "out": 0, "cw": 0, "cr": 0},
        {"dt": datetime(2026, 6, 30), "model": "claude-opus-4-8",
         "in": 2_000_000, "out": 0, "cw": 0, "cr": 0},
    ]
    out = costs._bucket(rows, lambda d: d.strftime("%Y-%m-%d"))
    assert [b["key"] for b in out] == ["2026-07-01", "2026-06-30"]  # reverse
    assert out[0]["cost"] == 5.0
    assert out[1]["cost"] == 10.0
