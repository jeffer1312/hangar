from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

LOCAL = timezone(timedelta(hours=-3))  # America/Sao_Paulo (sem horario de verao)

# Preco real por 1M tokens (fonte: skill claude-api). cache write = 1.25x input, read = 0.1x.
RATES: dict[str, dict[str, float]] = {
    "opus":   {"i": 5.0,  "o": 25.0, "cw": 6.25,  "cr": 0.50},
    "sonnet": {"i": 3.0,  "o": 15.0, "cw": 3.75,  "cr": 0.30},
    "haiku":  {"i": 1.0,  "o": 5.0,  "cw": 1.25,  "cr": 0.10},
    "fable":  {"i": 10.0, "o": 50.0, "cw": 12.50, "cr": 1.00},
}


def rates_for(model: str | None) -> dict[str, float]:
    m = (model or "").lower()
    for key in ("haiku", "fable", "opus", "sonnet"):
        if key in m:
            return RATES[key]
    return RATES["sonnet"]  # fallback conservador


def _load(config_dir: Path) -> list[dict]:
    """Le config_dir/metrics/costs.jsonl, dedup pela ultima linha por session_id
    (linhas sao snapshots cumulativos), converte timestamp p/ tz local."""
    src = config_dir / "metrics" / "costs.jsonl"
    if not src.is_file():
        return []
    latest: dict[str, dict] = {}
    with src.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                d = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts = d.get("timestamp")
            if not ts:
                continue
            key = d.get("session_id") or d.get("transcript_path") or ts
            prev = latest.get(key)
            if prev is None or ts > prev["timestamp"]:
                latest[key] = d
    rows: list[dict] = []
    for d in latest.values():
        dt = datetime.fromisoformat(d["timestamp"].replace("Z", "+00:00")).astimezone(LOCAL)
        rows.append({
            "dt": dt,
            "model": d.get("model"),
            "in": int(d.get("input_tokens", 0) or 0),
            "out": int(d.get("output_tokens", 0) or 0),
            "cw": int(d.get("cache_write_tokens", 0) or 0),
            "cr": int(d.get("cache_read_tokens", 0) or 0),
        })
    return rows


def _cost(row: dict) -> float:
    r = rates_for(row["model"])
    return (row["in"] / 1e6 * r["i"] + row["out"] / 1e6 * r["o"]
            + row["cw"] / 1e6 * r["cw"] + row["cr"] / 1e6 * r["cr"])


def _bucket(rows: list[dict], keyfn) -> list[dict]:
    agg: dict[str, dict] = {}
    for row in rows:
        k = keyfn(row["dt"])
        a = agg.setdefault(k, {"sessions": 0, "in": 0, "out": 0, "cr": 0, "cw": 0, "cost": 0.0})
        a["sessions"] += 1
        a["in"] += row["in"]
        a["out"] += row["out"]
        a["cr"] += row["cr"]
        a["cw"] += row["cw"]
        a["cost"] += _cost(row)
    return [{"key": k, **a} for k, a in sorted(agg.items(), reverse=True)]
