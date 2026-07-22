from __future__ import annotations

import json
import logging
import time
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app.config import list_config_dirs
from app.models import AccountCost, Bucket, CostReport, ModelBucket

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
            latest[key] = d  # last line wins (append-only log), no timestamp comparison
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


def _iso_week(dt: datetime) -> str:
    y, w, _ = dt.isocalendar()
    return f"{y}-W{w:02d}"


def _to_bucket(d: dict) -> Bucket:
    return Bucket(key=d["key"], sessions=d["sessions"], input=d["in"],
                  output=d["out"], cache_read=d["cr"], cache_write=d["cw"],
                  cost=d["cost"])


def _totals(rows: list[dict]) -> Bucket:
    b = _bucket(rows, lambda _dt: "totals")
    return _to_bucket(b[0]) if b else Bucket(
        key="totals", sessions=0, input=0, output=0,
        cache_read=0, cache_write=0, cost=0.0)


def _by_model(rows: list[dict]) -> list[ModelBucket]:
    agg: dict[str, dict] = {}
    for row in rows:
        m = row["model"] or "?"
        a = agg.setdefault(m, {"sessions": 0, "in": 0, "out": 0, "cr": 0, "cw": 0, "cost": 0.0})
        a["sessions"] += 1
        a["in"] += row["in"]
        a["out"] += row["out"]
        a["cr"] += row["cr"]
        a["cw"] += row["cw"]
        a["cost"] += _cost(row)
    return [ModelBucket(model=m, sessions=a["sessions"], cost=a["cost"],
                        input=a["in"], output=a["out"],
                        cache_read=a["cr"], cache_write=a["cw"])
            for m, a in sorted(agg.items(), key=lambda kv: -kv[1]["cost"])]


def aggregate(rows: list[dict], account_id: str, email: str | None,
              label: str, now: datetime) -> AccountCost:
    today = now.strftime("%Y-%m-%d")
    yest = (now - timedelta(days=1)).strftime("%Y-%m-%d")
    day = _bucket(rows, lambda d: d.strftime("%Y-%m-%d"))
    week = _bucket(rows, _iso_week)
    month = _bucket(rows, lambda d: d.strftime("%Y-%m"))
    return AccountCost(
        account_id=account_id,
        email=email,
        label=label,
        totals=_totals(rows),
        today=sum(b["cost"] for b in day if b["key"] == today),
        yesterday=sum(b["cost"] for b in day if b["key"] == yest),
        by_day=[_to_bucket(b) for b in day],
        by_week=[_to_bucket(b) for b in week],
        by_month=[_to_bucket(b) for b in month],
        by_model=_by_model(rows),
    )


def _account_info(config_dir: Path, fallback_label: str) -> tuple[str, str | None, str]:
    # .claude.json fica DENTRO do config dir (CLAUDE_CONFIG_DIR custom) ou em ~/.claude.json (default).
    for f in (config_dir / ".claude.json", Path.home() / ".claude.json"):
        try:
            oa = (json.loads(f.read_text()).get("oauthAccount") or {})
        except (OSError, json.JSONDecodeError, AttributeError, TypeError):
            # AttributeError/TypeError: .claude.json existe mas root nao e dict (corrompido) -> fallback
            continue
        uuid = oa.get("accountUuid")
        if uuid:
            email = oa.get("emailAddress")
            return uuid, email, (email or fallback_label)
    return fallback_label, None, fallback_label


# Cotação USD/BRL: cache em memória de 1h. Falha também "conta" como tentativa (atualiza o
# timestamp) — senão cada request offline pagaria os 3s de timeout até a rede voltar.
_RATE_URL = "https://economia.awesomeapi.com.br/json/last/USD-BRL"
_rate: float | None = None
_rate_at: float = 0.0


def usd_brl() -> float | None:
    global _rate, _rate_at
    now = time.monotonic()
    if _rate_at and now - _rate_at < 3600:
        return _rate
    _rate_at = now
    try:
        with urllib.request.urlopen(_RATE_URL, timeout=3) as r:
            _rate = float(json.load(r)["USDBRL"]["bid"])
    except Exception as e:
        # Mantém a última cotação conhecida (ou None) — front cai pra USD. O log distingue
        # timeout de mudança de schema da API (senão os dois falham idênticos pra sempre).
        logging.getLogger(__name__).warning("cotação USD/BRL falhou: %r", e)
    return _rate


def report(now: datetime | None = None) -> CostReport:
    now = now or datetime.now(LOCAL)
    accounts: list[AccountCost] = []
    for cfg in list_config_dirs():
        cdir = Path(cfg.path)
        rows = _load(cdir)
        acc_id, email, label = _account_info(cdir, cfg.label)
        accounts.append(aggregate(rows, acc_id, email, label, now))
    return CostReport(accounts=accounts, usd_brl=usd_brl())
