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


def test_load_last_line_wins_on_timestamp_tie(tmp_path):
    src = tmp_path / "metrics" / "costs.jsonl"
    _write_jsonl(src, [
        {"timestamp": "2026-07-01T10:00:00.000Z", "session_id": "s1",
         "model": "claude-opus-4-8", "input_tokens": 100, "output_tokens": 0,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
        {"timestamp": "2026-07-01T10:00:00.000Z", "session_id": "s1",
         "model": "claude-opus-4-8", "input_tokens": 999, "output_tokens": 0,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    rows = costs._load(tmp_path)
    assert len(rows) == 1
    assert rows[0]["in"] == 999  # ultima linha vence mesmo com timestamp igual


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


def test_aggregate_today_yesterday_and_totals():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=costs.LOCAL)
    y = now - timedelta(days=1)
    rows = [
        {"dt": now, "model": "claude-opus-4-8",
         "in": 1_000_000, "out": 0, "cw": 0, "cr": 0},   # hoje, $5
        {"dt": y, "model": "claude-opus-4-8",
         "in": 2_000_000, "out": 0, "cw": 0, "cr": 0},    # ontem, $10
    ]
    acc = costs.aggregate(rows, "uuid-1", "a@b.com", "a@b.com", now)
    assert acc.account_id == "uuid-1"
    assert acc.email == "a@b.com"
    assert acc.today == 5.0
    assert acc.yesterday == 10.0
    assert acc.totals.cost == 15.0
    assert acc.totals.sessions == 2
    # soma dos by_day bate com o total
    assert sum(b.cost for b in acc.by_day) == acc.totals.cost
    assert len(acc.by_model) == 1
    assert acc.by_model[0].model == "claude-opus-4-8"


def test_account_info_reads_oauth(tmp_path):
    (tmp_path / ".claude.json").write_text(json.dumps(
        {"oauthAccount": {"accountUuid": "u-9", "emailAddress": "x@y.com"}}))
    aid, email, label = costs._account_info(tmp_path, "fallback")
    assert (aid, email, label) == ("u-9", "x@y.com", "x@y.com")


def test_account_info_fallback_when_missing(tmp_path, monkeypatch):
    # config_dir sem .claude.json E HOME isolado (sem ~/.claude.json) -> cai no fallback.
    monkeypatch.setenv("HOME", str(tmp_path))
    aid, email, label = costs._account_info(tmp_path / "cfg", "fallback")
    assert aid == "fallback"
    assert email is None


def test_account_info_fallback_when_json_root_not_dict(tmp_path, monkeypatch):
    # .claude.json corrompido (root e lista, nao dict) -> nao explode, cai no fallback.
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".claude.json").write_text("[1, 2, 3]")
    aid, email, label = costs._account_info(tmp_path, "fallback")
    assert aid == "fallback"
    assert email is None
    assert label == "fallback"


def test_by_model_sums_tokens_per_model():
    """by_model passou a carregar tokens, não só sessions/cost. Agregar por modelo é somar linhas
    de dias diferentes num balde só — errar a chave (ex: reusar o acumulador entre modelos)
    daria número plausível e errado, do tipo que ninguém confere de cabeça."""
    rows = [
        {"dt": datetime(2026, 7, 1), "model": "claude-opus-4-8",
         "in": 1_000_000, "out": 10, "cw": 100, "cr": 1_000},
        {"dt": datetime(2026, 6, 30), "model": "claude-opus-4-8",
         "in": 2_000_000, "out": 20, "cw": 200, "cr": 2_000},
        {"dt": datetime(2026, 6, 30), "model": "claude-fable-5",
         "in": 5_000_000, "out": 30, "cw": 300, "cr": 3_000},
    ]
    by = {m.model: m for m in costs._by_model(rows)}

    assert by["claude-opus-4-8"].sessions == 2
    assert by["claude-opus-4-8"].input == 3_000_000     # 1M + 2M, atravessando o mês
    assert by["claude-opus-4-8"].output == 30
    assert by["claude-opus-4-8"].cache_write == 300
    assert by["claude-opus-4-8"].cache_read == 3_000
    # Modelo vizinho intocado: sem isto, um acumulador compartilhado passaria despercebido.
    assert by["claude-fable-5"].input == 5_000_000
    assert by["claude-fable-5"].sessions == 1


def test_by_model_tokens_batem_com_o_total():
    """Invariante: fatiar por modelo não pode criar nem sumir token."""
    now = datetime(2026, 7, 1, 12, 0, tzinfo=costs.LOCAL)
    rows = [
        {"dt": now, "model": "claude-opus-4-8", "in": 7, "out": 11, "cw": 13, "cr": 17},
        {"dt": now, "model": "claude-fable-5", "in": 19, "out": 23, "cw": 29, "cr": 31},
    ]
    acc = costs.aggregate(rows, "uuid-1", None, "l", now)
    assert sum(m.input for m in acc.by_model) == acc.totals.input
    assert sum(m.output for m in acc.by_model) == acc.totals.output
    assert sum(m.cache_write for m in acc.by_model) == acc.totals.cache_write
    assert sum(m.cache_read for m in acc.by_model) == acc.totals.cache_read


def test_usd_brl_cacheia_e_nao_rebate_na_rede(monkeypatch):
    """1ª chamada busca; 2ª usa cache; falha mantém última cotação conhecida."""
    import io

    calls = {"n": 0}

    class FakeResp(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    def fake_urlopen(url, timeout=None):
        calls["n"] += 1
        return FakeResp(b'{"USDBRL": {"bid": "5.4321"}}')

    monkeypatch.setattr(costs.urllib.request, "urlopen", fake_urlopen)
    monkeypatch.setattr(costs, "_rate", None)
    monkeypatch.setattr(costs, "_rate_at", 0.0)

    assert costs.usd_brl() == 5.4321
    assert costs.usd_brl() == 5.4321  # cache: sem novo hit
    assert calls["n"] == 1

    # Cache expirado + rede fora -> mantém a última cotação, e a falha "conta" como tentativa
    monkeypatch.setattr(costs, "_rate_at", 0.0)

    def boom(url, timeout=None):
        calls["n"] += 1
        raise OSError("offline")

    monkeypatch.setattr(costs.urllib.request, "urlopen", boom)
    assert costs.usd_brl() == 5.4321
    n_after_fail = calls["n"]
    assert costs.usd_brl() == 5.4321  # falha cacheada: não tenta de novo já
    assert calls["n"] == n_after_fail
