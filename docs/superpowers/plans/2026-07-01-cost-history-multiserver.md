# Histórico de Custos Multi-Server — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tela dedicada no claude-pocket que mostra custo histórico de uso do Claude Code, somando vários servers mas separando por conta Claude.

**Architecture:** Backend novo módulo puro `costs.py` agrega `costs.jsonl` por config dir (conta) e expõe `GET /api/costs`. Frontend busca de todos os servers em paralelo e mescla por `account_id` numa função pura (`lib/costs.ts`), renderizando na tela `Costs.svelte` (rota `#/costs`). Preço recalculado dos tokens por modelo, não lido do ECC.

**Tech Stack:** Backend Python 3.14 + FastAPI + Pydantic + pytest. Frontend Svelte 5 + TypeScript + Vitest.

## Global Constraints

- Preço **recalculado dos tokens** com tabela `RATES` por modelo — NUNCA usar `estimated_cost_usd` do ECC (defasado $15/$75).
- Tabela `RATES` verbatim: opus `i=5.0 o=25.0 cw=6.25 cr=0.50`, sonnet `i=3.0 o=15.0 cw=3.75 cr=0.30`, haiku `i=1.0 o=5.0 cw=1.25 cr=0.10`, fable `i=10.0 o=50.0 cw=12.50 cr=1.00`.
- `costs.jsonl`: linhas são snapshots **cumulativos** por sessão → dedup pela **última linha por `session_id`** antes de somar.
- Timezone local fixa: `America/Sao_Paulo` = UTC-3 sem horário de verão (`timezone(timedelta(hours=-3))`).
- Conta identificada por `oauthAccount.accountUuid`; label = `emailAddress`. Fallback = label do config dir.
- Backend segue padrão de rota existente: `@app.get(..., dependencies=[Depends(require_auth)], response_model=...)`.
- Frontend: fetch por server usa `s.baseUrl` + `Authorization: Bearer ${s.token}` + `AbortSignal.timeout(4000)` (modelo `fetchSessionsForServer`).
- Backend testa com `cd backend && uv run pytest`. Frontend testa com `npm --prefix frontend run test` (vitest) e o gate de tipo é `npm --prefix frontend run check`.

---

### Task 1: Backend — primitivas de agregação (`costs.py`)

Módulo puro: preço, load do jsonl (dedup), custo por sessão, bucketing. Sem FastAPI, sem models Pydantic ainda (retorna dicts internos; os models entram na Task 2).

**Files:**
- Create: `backend/app/costs.py`
- Test: `backend/tests/test_costs.py`

**Interfaces:**
- Consumes: nada.
- Produces:
  - `RATES: dict[str, dict[str, float]]`
  - `rates_for(model: str | None) -> dict[str, float]`
  - `LOCAL: timezone`
  - `_load(config_dir: Path) -> list[dict]` — cada dict: `{"dt": datetime, "model": str|None, "in": int, "out": int, "cw": int, "cr": int}`
  - `_cost(row: dict) -> float`
  - `_bucket(rows: list[dict], keyfn) -> list[dict]` — cada dict: `{"key": str, "sessions": int, "in": int, "out": int, "cr": int, "cw": int, "cost": float}`, ordenado `reverse` por key

- [ ] **Step 1: Write the failing test**

Create `backend/tests/test_costs.py`:

```python
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd backend && uv run pytest tests/test_costs.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'app.costs'`

- [ ] **Step 3: Write minimal implementation**

Create `backend/app/costs.py`:

```python
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd backend && uv run pytest tests/test_costs.py -v`
Expected: PASS (6 passed)

- [ ] **Step 5: Commit**

```bash
git add backend/app/costs.py backend/tests/test_costs.py
git commit -m "feat(costs): primitivas de agregacao (rates, load dedup, bucket)"
```

---

### Task 2: Backend — models, `aggregate`, `report` e rota `/api/costs`

Adiciona os models Pydantic, a agregação por conta (com `now` injetável p/ teste), a info de conta (`oauthAccount`), o `report()` que varre config dirs, e a rota.

**Files:**
- Modify: `backend/app/models.py` (append)
- Modify: `backend/app/costs.py` (append)
- Modify: `backend/app/api.py` (import + rota)
- Test: `backend/tests/test_costs.py` (append)

**Interfaces:**
- Consumes (Task 1): `_load`, `_cost`, `_bucket`, `rates_for`, `LOCAL`.
- Consumes (existente): `list_config_dirs()` de `app.config` → `list[ConfigDirInfo]` com `.path`/`.label`.
- Produces:
  - Models: `Bucket`, `ModelBucket`, `AccountCost`, `CostReport`.
  - `aggregate(rows: list[dict], account_id: str, email: str | None, label: str, now: datetime) -> AccountCost`
  - `_account_info(config_dir: Path, fallback_label: str) -> tuple[str, str | None, str]`
  - `report(now: datetime | None = None) -> CostReport`
  - Rota `GET /api/costs` → `CostReport`

- [ ] **Step 1: Write the failing test (append a `backend/tests/test_costs.py`)**

```python
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


def test_account_info_fallback_when_missing(tmp_path):
    # dir sem .claude.json E sem oauthAccount em ~/.claude.json usavel -> cai no fallback.
    # (usa um dir isolado; se ~/.claude.json existir e tiver conta, o teste ainda passa pois
    #  _account_info tenta config_dir primeiro — aqui config_dir esta vazio, entao vai pro home;
    #  para isolar de verdade, o teste roda com HOME apontando pro tmp_path.)
    import os
    old = os.environ.get("HOME")
    os.environ["HOME"] = str(tmp_path)
    try:
        aid, email, label = costs._account_info(tmp_path / "cfg", "fallback")
    finally:
        if old is not None:
            os.environ["HOME"] = old
    assert aid == "fallback"
    assert email is None
    assert label == "fallback"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd backend && uv run pytest tests/test_costs.py -v`
Expected: FAIL — `AttributeError: module 'app.costs' has no attribute 'aggregate'`

- [ ] **Step 3a: Append models to `backend/app/models.py`**

```python
class Bucket(BaseModel):
    key: str
    sessions: int
    input: int
    output: int
    cache_read: int
    cache_write: int
    cost: float


class ModelBucket(BaseModel):
    model: str
    sessions: int
    cost: float


class AccountCost(BaseModel):
    account_id: str
    email: Optional[str] = None
    label: str
    totals: Bucket
    today: float
    yesterday: float
    by_day: list[Bucket]
    by_week: list[Bucket]
    by_month: list[Bucket]
    by_model: list[ModelBucket]


class CostReport(BaseModel):
    accounts: list[AccountCost]
```

- [ ] **Step 3b: Append aggregation to `backend/app/costs.py`**

Add these imports at the top of `costs.py` (below the existing imports). NOTE: import `app.models`/`app.config` locally inside functions is NOT needed — top-level is fine (no circular import: `models.py` and `config.py` don't import `costs.py`):

```python
from app.config import list_config_dirs
from app.models import AccountCost, Bucket, CostReport, ModelBucket
```

Append:

```python
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
        a = agg.setdefault(m, {"sessions": 0, "cost": 0.0})
        a["sessions"] += 1
        a["cost"] += _cost(row)
    return [ModelBucket(model=m, sessions=a["sessions"], cost=a["cost"])
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
        except (OSError, json.JSONDecodeError):
            continue
        uuid = oa.get("accountUuid")
        if uuid:
            email = oa.get("emailAddress")
            return uuid, email, (email or fallback_label)
    return fallback_label, None, fallback_label


def report(now: datetime | None = None) -> CostReport:
    now = now or datetime.now(LOCAL)
    accounts: list[AccountCost] = []
    for cfg in list_config_dirs():
        cdir = Path(cfg.path)
        rows = _load(cdir)
        acc_id, email, label = _account_info(cdir, cfg.label)
        accounts.append(aggregate(rows, acc_id, email, label, now))
    return CostReport(accounts=accounts)
```

- [ ] **Step 3c: Add the route to `backend/app/api.py`**

Add `CostReport` to the existing `from app.models import ...` line (line 20) so it reads:

```python
from app.models import SessionInfo, ChatEvent, CostReport
```

Add a new import near the other `app.` imports:

```python
from app.costs import report as costs_report
```

Add the route (place it right after the `claude_configs` route, ~line 207):

```python
@app.get("/api/costs", dependencies=[Depends(require_auth)], response_model=CostReport)
def costs_endpoint():
    return costs_report()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd backend && uv run pytest tests/test_costs.py -v`
Expected: PASS (9 passed)

- [ ] **Step 5: Smoke-test the route**

Run:
```bash
cd backend && CP_AUTH_TOKEN=test uv run python -c "
from fastapi.testclient import TestClient
from app.api import app
c = TestClient(app)
r = c.get('/api/costs', headers={'Authorization': 'Bearer test'})
print(r.status_code)
assert r.status_code == 200, r.text
assert 'accounts' in r.json()
print('accounts:', len(r.json()['accounts']))
"
```
Expected: `200` and `accounts: N` (N ≥ 0).

- [ ] **Step 6: Commit**

```bash
git add backend/app/costs.py backend/app/models.py backend/app/api.py backend/tests/test_costs.py
git commit -m "feat(costs): report por conta + rota GET /api/costs"
```

---

### Task 3: Frontend — `abbrevNum` em `format.ts`

Helper de abreviação K/M/B pros tokens.

**Files:**
- Modify: `frontend/src/lib/format.ts` (append)
- Test: `frontend/src/lib/format.test.ts` (create)

**Interfaces:**
- Produces: `abbrevNum(n: number): string`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/lib/format.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';
import { abbrevNum } from './format';

describe('abbrevNum', () => {
  it('abbreviates millions', () => {
    expect(abbrevNum(3_668_662)).toBe('3.7M');
  });
  it('abbreviates billions', () => {
    expect(abbrevNum(1_539_946_914)).toBe('1.5B');
  });
  it('abbreviates thousands', () => {
    expect(abbrevNum(12_500)).toBe('12.5K');
  });
  it('leaves small numbers as-is', () => {
    expect(abbrevNum(999)).toBe('999');
  });
  it('drops trailing .0', () => {
    expect(abbrevNum(2_000_000)).toBe('2M');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm --prefix frontend run test -- --run format.test`
Expected: FAIL — `abbrevNum` is not exported / not a function.

- [ ] **Step 3: Write minimal implementation (append to `frontend/src/lib/format.ts`)**

```typescript
// Abrevia contagem grande: 3668662 -> "3.7M", 1.5e9 -> "1.5B", 999 -> "999".
export function abbrevNum(n: number): string {
  for (const [div, suf] of [[1e9, 'B'], [1e6, 'M'], [1e3, 'K']] as const) {
    if (n >= div) return (n / div).toFixed(1).replace(/\.0$/, '') + suf;
  }
  return String(Math.round(n));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm --prefix frontend run test -- --run format.test`
Expected: PASS (5 passed)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/format.ts frontend/src/lib/format.test.ts
git commit -m "feat(format): abbrevNum K/M/B"
```

---

### Task 4: Frontend — tipos + `fetchCostsForServer`

Espelha os models do backend em `types.ts` e adiciona o fetch por server (modelo `fetchSessionsForServer`).

**Files:**
- Modify: `frontend/src/lib/types.ts` (append)
- Modify: `frontend/src/lib/api.ts` (append)

**Interfaces:**
- Consumes: `Server` de `./auth`.
- Produces:
  - Types: `CostBucket`, `CostModelBucket`, `AccountCost`, `CostReport`.
  - `fetchCostsForServer(s: Server): Promise<CostReport>`

- [ ] **Step 1: Append types to `frontend/src/lib/types.ts`**

```typescript
export interface CostBucket {
  key: string;
  sessions: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  cost: number;
}

export interface CostModelBucket {
  model: string;
  sessions: number;
  cost: number;
}

export interface AccountCost {
  account_id: string;
  email: string | null;
  label: string;
  totals: CostBucket;
  today: number;
  yesterday: number;
  by_day: CostBucket[];
  by_week: CostBucket[];
  by_month: CostBucket[];
  by_model: CostModelBucket[];
}

export interface CostReport {
  accounts: AccountCost[];
}
```

- [ ] **Step 2: Append fetch to `frontend/src/lib/api.ts`**

Add `CostReport` to the existing `import type { ... } from './types';` block, then append:

```typescript
// Custo de UM servidor (baseUrl+token explicitos), sem mexer no ativo. Igual fetchSessionsForServer:
// a visao agregada chama todos em paralelo; um servidor lento/offline falha rapido (timeout 4s) e e
// pulado, sem segurar os demais.
export async function fetchCostsForServer(s: Server): Promise<CostReport> {
  const res = await fetch(`${s.baseUrl}/api/costs`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(4000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<CostReport>;
}
```

- [ ] **Step 3: Typecheck**

Run: `npm --prefix frontend run check`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/types.ts frontend/src/lib/api.ts
git commit -m "feat(costs): tipos + fetchCostsForServer"
```

---

### Task 5: Frontend — merge cross-server (`lib/costs.ts`)

Função pura que soma os servers mantendo contas separadas por `account_id`.

**Files:**
- Create: `frontend/src/lib/costs.ts`
- Test: `frontend/src/lib/costs.test.ts`

**Interfaces:**
- Consumes: `CostReport`, `AccountCost`, `CostBucket`, `CostModelBucket` de `./types`.
- Produces:
  - `interface ServerResult { report: CostReport | null }` — `null` = server falhou.
  - `interface MergedReport { accounts: AccountCost[]; partial: boolean }`
  - `mergeAccounts(results: ServerResult[]): MergedReport`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/lib/costs.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';
import { mergeAccounts, type ServerResult } from './costs';
import type { AccountCost, CostBucket } from './types';

function bucket(key: string, cost: number): CostBucket {
  return { key, sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost };
}

function acc(id: string, cost: number, days: CostBucket[]): AccountCost {
  return {
    account_id: id, email: `${id}@x.com`, label: id,
    totals: { key: 'totals', sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost },
    today: 0, yesterday: 0,
    by_day: days, by_week: [], by_month: [],
    by_model: [{ model: 'opus', sessions: 1, cost }],
  };
}

describe('mergeAccounts', () => {
  it('sums the same account across servers by day key', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [bucket('2026-07-01', 5)])] } };
    const b: ServerResult = { report: { accounts: [acc('u1', 3, [bucket('2026-07-01', 3)])] } };
    const merged = mergeAccounts([a, b]);
    expect(merged.accounts).toHaveLength(1);
    expect(merged.accounts[0].totals.cost).toBe(8);
    expect(merged.accounts[0].by_day).toHaveLength(1);
    expect(merged.accounts[0].by_day[0].cost).toBe(8);
    expect(merged.partial).toBe(false);
  });

  it('keeps different accounts separate', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [])] } };
    const b: ServerResult = { report: { accounts: [acc('u2', 3, [])] } };
    const merged = mergeAccounts([a, b]);
    expect(merged.accounts).toHaveLength(2);
  });

  it('marks partial when a server failed', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [])] } };
    const failed: ServerResult = { report: null };
    const merged = mergeAccounts([a, failed]);
    expect(merged.partial).toBe(true);
    expect(merged.accounts).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm --prefix frontend run test -- --run costs.test`
Expected: FAIL — cannot find module `./costs` / `mergeAccounts`.

- [ ] **Step 3: Write minimal implementation**

Create `frontend/src/lib/costs.ts`:

```typescript
import type { AccountCost, CostBucket, CostModelBucket, CostReport } from './types';

export interface ServerResult {
  report: CostReport | null; // null = servidor falhou/offline
}

export interface MergedReport {
  accounts: AccountCost[];
  partial: boolean; // algum servidor nao respondeu
}

function addBuckets(into: Map<string, CostBucket>, list: CostBucket[]): void {
  for (const b of list) {
    const cur = into.get(b.key);
    if (!cur) {
      into.set(b.key, { ...b });
    } else {
      cur.sessions += b.sessions;
      cur.input += b.input;
      cur.output += b.output;
      cur.cache_read += b.cache_read;
      cur.cache_write += b.cache_write;
      cur.cost += b.cost;
    }
  }
}

function addModels(into: Map<string, CostModelBucket>, list: CostModelBucket[]): void {
  for (const m of list) {
    const cur = into.get(m.model);
    if (!cur) into.set(m.model, { ...m });
    else { cur.sessions += m.sessions; cur.cost += m.cost; }
  }
}

interface Acc {
  account_id: string;
  email: string | null;
  label: string;
  totals: CostBucket;
  today: number;
  yesterday: number;
  day: Map<string, CostBucket>;
  week: Map<string, CostBucket>;
  month: Map<string, CostBucket>;
  model: Map<string, CostModelBucket>;
}

function emptyTotals(): CostBucket {
  return { key: 'totals', sessions: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0 };
}

// Ordena buckets de periodo por key desc (data mais recente primeiro).
function sortDesc(m: Map<string, CostBucket>): CostBucket[] {
  return [...m.values()].sort((a, b) => (a.key < b.key ? 1 : a.key > b.key ? -1 : 0));
}

export function mergeAccounts(results: ServerResult[]): MergedReport {
  const byId = new Map<string, Acc>();
  let partial = false;

  for (const r of results) {
    if (!r.report) { partial = true; continue; }
    for (const a of r.report.accounts) {
      let acc = byId.get(a.account_id);
      if (!acc) {
        acc = {
          account_id: a.account_id, email: a.email, label: a.label,
          totals: emptyTotals(), today: 0, yesterday: 0,
          day: new Map(), week: new Map(), month: new Map(), model: new Map(),
        };
        byId.set(a.account_id, acc);
      }
      acc.totals.sessions += a.totals.sessions;
      acc.totals.input += a.totals.input;
      acc.totals.output += a.totals.output;
      acc.totals.cache_read += a.totals.cache_read;
      acc.totals.cache_write += a.totals.cache_write;
      acc.totals.cost += a.totals.cost;
      acc.today += a.today;
      acc.yesterday += a.yesterday;
      addBuckets(acc.day, a.by_day);
      addBuckets(acc.week, a.by_week);
      addBuckets(acc.month, a.by_month);
      addModels(acc.model, a.by_model);
    }
  }

  const accounts: AccountCost[] = [...byId.values()].map((acc) => ({
    account_id: acc.account_id,
    email: acc.email,
    label: acc.label,
    totals: acc.totals,
    today: acc.today,
    yesterday: acc.yesterday,
    by_day: sortDesc(acc.day),
    by_week: sortDesc(acc.week),
    by_month: sortDesc(acc.month),
    by_model: [...acc.model.values()].sort((a, b) => b.cost - a.cost),
  }));

  return { accounts, partial };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm --prefix frontend run test -- --run costs.test`
Expected: PASS (3 passed)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/costs.ts frontend/src/lib/costs.test.ts
git commit -m "feat(costs): mergeAccounts cross-server"
```

---

### Task 6: Frontend — tela `Costs.svelte`, rota `#/costs` e entrada no menu

Tela que busca todos os servers, mescla e renderiza; rota no `App.svelte`; item "Custos" no menu da `SessionList`.

**Files:**
- Create: `frontend/src/screens/Costs.svelte`
- Modify: `frontend/src/App.svelte` (Route type, parseHash, import, render branch)
- Modify: `frontend/src/screens/SessionList.svelte` (item de menu)

**Interfaces:**
- Consumes: `listServers` de `../lib/auth`, `fetchCostsForServer` de `../lib/api`, `mergeAccounts`/`ServerResult`/`MergedReport` de `../lib/costs`, `abbrevNum` de `../lib/format`, `AccountCost`/`CostBucket` de `../lib/types`, `NavBar`.

- [ ] **Step 1: Create `frontend/src/screens/Costs.svelte`**

```svelte
<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import { listServers } from '../lib/auth';
  import { fetchCostsForServer } from '../lib/api';
  import { mergeAccounts, type ServerResult, type MergedReport } from '../lib/costs';
  import { abbrevNum } from '../lib/format';
  import type { AccountCost, CostBucket } from '../lib/types';

  interface Props { onBack: () => void; }
  let { onBack }: Props = $props();

  let loading = $state(true);
  let merged = $state<MergedReport>({ accounts: [], partial: false });
  let selected = $state<string>('all'); // account_id ou 'all'
  let period = $state<'day' | 'week' | 'month'>('day');

  async function load() {
    loading = true;
    const servers = listServers();
    const results: ServerResult[] = await Promise.all(
      servers.map(async (s) => {
        try { return { report: await fetchCostsForServer(s) }; }
        catch { return { report: null }; }
      }),
    );
    merged = mergeAccounts(results);
    if (!merged.accounts.some((a) => a.account_id === selected)) selected = 'all';
    loading = false;
  }

  $effect(() => { load(); });

  // Soma uma lista de listas de buckets por key (usado no modo "Todas").
  function sumBuckets(lists: CostBucket[][]): CostBucket[] {
    const m = new Map<string, CostBucket>();
    for (const list of lists) for (const b of list) {
      const cur = m.get(b.key);
      if (!cur) m.set(b.key, { ...b });
      else {
        cur.sessions += b.sessions; cur.input += b.input; cur.output += b.output;
        cur.cache_read += b.cache_read; cur.cache_write += b.cache_write; cur.cost += b.cost;
      }
    }
    return [...m.values()].sort((a, b) => (a.key < b.key ? 1 : -1));
  }

  const view = $derived.by<AccountCost | null>(() => {
    const accs = merged.accounts;
    if (accs.length === 0) return null;
    if (selected !== 'all') return accs.find((a) => a.account_id === selected) ?? null;
    // "Todas" = agrega todas as contas
    const sum = (f: (a: AccountCost) => number) => accs.reduce((t, a) => t + f(a), 0);
    const modelMap = new Map<string, { model: string; sessions: number; cost: number }>();
    for (const a of accs) for (const mb of a.by_model) {
      const cur = modelMap.get(mb.model);
      if (!cur) modelMap.set(mb.model, { ...mb });
      else { cur.sessions += mb.sessions; cur.cost += mb.cost; }
    }
    return {
      account_id: 'all', email: null, label: 'Todas',
      totals: {
        key: 'totals', sessions: sum((a) => a.totals.sessions),
        input: sum((a) => a.totals.input), output: sum((a) => a.totals.output),
        cache_read: sum((a) => a.totals.cache_read), cache_write: sum((a) => a.totals.cache_write),
        cost: sum((a) => a.totals.cost),
      },
      today: sum((a) => a.today), yesterday: sum((a) => a.yesterday),
      by_day: sumBuckets(accs.map((a) => a.by_day)),
      by_week: sumBuckets(accs.map((a) => a.by_week)),
      by_month: sumBuckets(accs.map((a) => a.by_month)),
      by_model: [...modelMap.values()].sort((a, b) => b.cost - a.cost),
    };
  });

  const rows = $derived(
    view ? (period === 'day' ? view.by_day : period === 'week' ? view.by_week : view.by_month) : [],
  );
  const peak = $derived(Math.max(1, ...rows.map((r) => r.cost)));
  const money = (n: number) => `$${n.toFixed(2)}`;
</script>

<NavBar title="Custos" showBack={true} onBack={onBack} />

<div class="costs">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if !view}
    <p class="muted">Sem dados ainda. O custo aparece após a 1ª sessão parar.</p>
  {:else}
    {#if merged.partial}
      <p class="warn">⚠ Alguns servidores não responderam — total parcial.</p>
    {/if}

    <div class="tabs" role="tablist" aria-label="Conta">
      <button class:on={selected === 'all'} onclick={() => (selected = 'all')}>Todas</button>
      {#each merged.accounts as a}
        <button class:on={selected === a.account_id} onclick={() => (selected = a.account_id)}>
          {a.email ?? a.label}
        </button>
      {/each}
    </div>

    <div class="chips">
      <span class="chip">Hoje <b>{money(view.today)}</b></span>
      <span class="chip">Ontem <b>{money(view.yesterday)}</b></span>
    </div>

    <div class="cards">
      <div class="card"><div class="v">{money(view.totals.cost)}</div><div class="l">custo total</div></div>
      <div class="card"><div class="v">{view.totals.sessions}</div><div class="l">sessões</div></div>
      <div class="card"><div class="v">{abbrevNum(view.totals.input)}</div><div class="l">input</div></div>
      <div class="card"><div class="v">{abbrevNum(view.totals.output)}</div><div class="l">output</div></div>
    </div>

    <div class="tabs" role="tablist" aria-label="Período">
      <button class:on={period === 'day'} onclick={() => (period = 'day')}>Dia</button>
      <button class:on={period === 'week'} onclick={() => (period = 'week')}>Semana</button>
      <button class:on={period === 'month'} onclick={() => (period = 'month')}>Mês</button>
    </div>

    <table>
      <thead><tr><th>período</th><th>sess</th><th>in</th><th>out</th><th>cache</th><th>custo</th><th></th></tr></thead>
      <tbody>
        {#each rows as r}
          <tr>
            <td class="k">{r.key}</td>
            <td class="n">{r.sessions}</td>
            <td class="n">{abbrevNum(r.input)}</td>
            <td class="n">{abbrevNum(r.output)}</td>
            <td class="n">{abbrevNum(r.cache_read)}</td>
            <td class="c">{money(r.cost)}</td>
            <td class="bar"><span style="width:{(r.cost / peak) * 100}%"></span></td>
          </tr>
        {/each}
      </tbody>
    </table>

    <h3>Por modelo</h3>
    <table>
      <tbody>
        {#each view.by_model as m}
          <tr><td class="k">{m.model}</td><td class="n">{m.sessions}</td><td class="c">{money(m.cost)}</td></tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .costs { padding: 12px 16px 40px; }
  .muted { color: var(--text-dim, #8b949e); }
  .warn { color: #d29922; font-size: 13px; }
  .tabs { display: flex; gap: 8px; flex-wrap: wrap; margin: 12px 0; }
  .tabs button {
    background: var(--surface, #161b22); border: 1px solid var(--border, #30363d);
    color: inherit; padding: 6px 14px; border-radius: 8px; font-size: 14px;
  }
  .tabs button.on { background: var(--accent, #238636); border-color: var(--accent, #3fb950); }
  .chips { display: flex; gap: 10px; margin: 8px 0; }
  .chip { background: var(--surface, #161b22); border: 1px solid var(--border, #30363d); border-radius: 8px; padding: 6px 12px; font-size: 13px; }
  .cards { display: flex; gap: 10px; flex-wrap: wrap; margin: 12px 0; }
  .card { background: var(--surface, #161b22); border: 1px solid var(--border, #30363d); border-radius: 10px; padding: 12px 16px; min-width: 92px; }
  .card .v { font-size: 20px; font-weight: 700; }
  .card .l { font-size: 11px; color: var(--text-dim, #8b949e); text-transform: uppercase; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; margin-top: 8px; }
  th, td { padding: 6px 8px; text-align: left; border-bottom: 1px solid var(--border, #30363d); }
  th { color: var(--text-dim, #8b949e); font-weight: 600; font-size: 11px; text-transform: uppercase; }
  .n, .c { text-align: right; font-variant-numeric: tabular-nums; }
  .c { font-weight: 700; color: var(--accent, #3fb950); }
  .k { white-space: nowrap; }
  .bar { width: 80px; }
  .bar span { display: block; height: 8px; border-radius: 4px; background: var(--accent, #3fb950); }
  h3 { margin: 20px 0 4px; font-size: 15px; }
</style>
```

- [ ] **Step 2: Wire the route in `frontend/src/App.svelte`**

Add `'costs'` to the `Route` union (near line 10):

```typescript
  type Route =
    | { name: 'loading' }
    | { name: 'login' }
    | { name: 'sessions' }
    | { name: 'costs' }
    | { name: 'chat'; sessionName: string };
```

In `parseHash` (before the final `return { name: 'sessions' }`), add:

```typescript
    if (path === '/costs') return { name: 'costs' };
```

Import the screen with the other screen imports (near line 6):

```typescript
  import Costs from './screens/Costs.svelte';
```

Add a render branch. Find the `{:else if route.name === 'sessions'}` block (~line 188) and add this branch right after that block's closing (before `{:else if route.name === 'chat'}`):

```svelte
  {:else if route.name === 'costs'}
    <Costs onBack={() => navigateTo('#/')} />
```

- [ ] **Step 3: Add the menu entry in `frontend/src/screens/SessionList.svelte`**

In the NavBar dropdown menu (the block with `role="menuitem"` buttons, ~line 318, near "openAddServer"), add a new item that navigates to `#/costs`:

```svelte
        <button class="menu-item" role="menuitem" onclick={() => { showMenu = false; window.location.hash = '#/costs'; }}>
          Custos
        </button>
```

(If the local state var isn't `showMenu`, match the name used by the sibling menu-item buttons in that block — they all set the same "close menu" flag.)

- [ ] **Step 4: Typecheck**

Run: `npm --prefix frontend run check`
Expected: 0 errors.

- [ ] **Step 5: Build to confirm it compiles**

Run: `npm --prefix frontend run build`
Expected: build succeeds.

- [ ] **Step 6: Manual smoke (optional, needs backend running)**

Start backend (`cd backend && CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main`) and frontend (`npm --prefix frontend run dev`), pair, open the menu → "Custos", confirm the screen loads, tabs Dia/Semana/Mês switch, account selector shows accounts + "Todas".

- [ ] **Step 7: Commit**

```bash
git add frontend/src/screens/Costs.svelte frontend/src/App.svelte frontend/src/screens/SessionList.svelte
git commit -m "feat(costs): tela de custos, rota #/costs e entrada no menu"
```

---

## Self-Review

**Spec coverage:**
- Modelo 2 níveis (server/conta) → Task 2 (`report` por config dir) + Task 5 (`mergeAccounts`). ✅
- Preço recalc por modelo → Task 1 (`RATES`/`rates_for`/`_cost`). ✅
- Dedup por session_id → Task 1 (`_load`) + teste. ✅
- account_id via `oauthAccount` + fallback → Task 2 (`_account_info`) + testes. ✅
- Rota `/api/costs` → Task 2. ✅
- Fetch todos servers em paralelo + parcial → Task 6 (`load`) + Task 5 (`partial`). ✅
- Tela: seletor conta + Todas, hoje/ontem, cards, abas dia/semana/mês, por modelo → Task 6. ✅
- `abbrevNum` K/M/B → Task 3. ✅
- Empty state → Task 6 (`!view`). ✅
- Testes backend + frontend → Tasks 1,2,3,5. ✅

**Placeholder scan:** nenhum TBD/TODO; todo passo de código tem código completo. A única nota condicional (nome do flag de fechar menu na Task 6 Step 3) traz a regra explícita de como resolver, não é placeholder.

**Type consistency:** py `Bucket`/`ModelBucket`/`AccountCost`/`CostReport` ↔ ts `CostBucket`/`CostModelBucket`/`AccountCost`/`CostReport`. Nomes py `Bucket` vs ts `CostBucket` divergem de propósito (evita colisão no frontend), mas os campos batem 1:1 (`key/sessions/input/output/cache_read/cache_write/cost`). `fetchCostsForServer`, `mergeAccounts`, `ServerResult`, `MergedReport`, `abbrevNum` consistentes entre Tasks 4/5/6.
