# Histórico de custos multi-server, por conta

**Data:** 2026-07-01
**Status:** aprovado (brainstorming) — aguardando plano

## Objetivo

Tela dedicada no claude-pocket pra ver custo histórico de uso do Claude Code,
lido de `<config_dir>/metrics/costs.jsonl` (escrito pelo hook `stop:cost-tracker`
do ECC). Cross-server: soma vários servers claude-pocket, mas mantém as contas
Claude separadas.

Caso do usuário: 3 servers, 2 contas Claude → ver as 2 contas separadas, cada
uma somando os 3 servers.

## Modelo de dados (2 níveis)

- **Server** = 1 backend claude-pocket. O frontend já conecta em vários (lista de
  `Server` com id/label/color).
- **Conta** = um config dir do Claude (`list_config_dirs()`), identificada por
  `accountUuid` (de `<config_dir>/.claude.json` → `oauthAccount`). Label de
  exibição = `oauthAccount.emailAddress`. `accountUuid` é estável entre máquinas
  (mesmo login Claude = mesmo UUID) → é a chave de merge cross-server.
- Cada par `(server, conta)` tem seu próprio `<config_dir>/metrics/costs.jsonl`.

Agregação em dois lugares:
- **Backend** agrega por conta, dentro do seu próprio server.
- **Frontend** soma os servers, mantendo contas separadas por `account_id`.

## Preço

Recalculado dos tokens, **não** lido do `estimated_cost_usd` do ECC (tabela
defasada $15/$75). Tabela `RATES` por modelo no backend (fonte: skill claude-api):

| modelo | input $/1M | output $/1M | cache write (1.25x) | cache read (0.1x) |
|--------|-----------|-------------|---------------------|-------------------|
| opus   | 5.00      | 25.00       | 6.25                | 0.50              |
| sonnet | 3.00      | 15.00       | 3.75                | 0.30              |
| haiku  | 1.00      | 5.00        | 1.25                | 0.10              |
| fable  | 10.00     | 50.00       | 12.50               | 1.00              |

`rates_for(model)`: match por substring (`haiku`/`fable`/`opus`/`sonnet`),
fallback `sonnet` conservador. Tabela hardcoded com comentário (preço muda raro;
sem config de UI — YAGNI).

## Backend

### Módulo novo `backend/app/costs.py` (puro, sem FastAPI/tmux)

- `RATES` + `rates_for(model)`.
- `_load(config_dir: Path) -> list[Row]`: lê `config_dir/metrics/costs.jsonl`,
  **dedup: última linha por `session_id`** (as linhas são snapshots cumulativos —
  somar tudo multiplica), timestamp ISO `...Z` → tz local America/Sao_Paulo (-3).
  Arquivo ausente → lista vazia. Linhas inválidas puladas.
- `_cost(row) -> float`: tokens × `rates_for(row.model)`.
- `_account_info(config_dir: Path) -> (account_id, email, label)`: lê
  `config_dir/.claude.json` → `oauthAccount.accountUuid` / `.emailAddress`.
  Ausente → `account_id = label` (fallback), email `None`.
- `report() -> CostReport`: pra cada config dir de `list_config_dirs()`, agrega em
  um `AccountCost`. Buckets: `by_day` (chave `YYYY-MM-DD`), `by_week`
  (ISO `YYYY-Www`), `by_month` (`YYYY-MM`); `today`/`yesterday`; `by_model`;
  `totals`.

### Rota `api.py`

`GET /api/costs` com `dependencies=[Depends(require_auth)]`,
`response_model=CostReport`.

### Models (`models.py`)

```
class Bucket(BaseModel):      # por período ou por modelo
    key: str
    sessions: int
    input: int; output: int; cache_read: int; cache_write: int
    cost: float

class ModelBucket(BaseModel):
    model: str; sessions: int; cost: float

class AccountCost(BaseModel):
    account_id: str
    email: str | None
    label: str
    totals: Bucket           # key = "totals"
    today: float; yesterday: float
    by_day: list[Bucket]; by_week: list[Bucket]; by_month: list[Bucket]
    by_model: list[ModelBucket]

class CostReport(BaseModel):
    accounts: list[AccountCost]
```

## Frontend

### Fetch + merge

- `lib/api.ts`: `fetchCosts(server) -> CostReport` (por server, com auth).
- `lib/costs.ts` (novo, função pura): `mergeAccounts(perServer: CostReport[]) ->
  MergedAccount[]`. Merge por `account_id`:
  - mesma conta em N servers → soma `totals`/`today`/`yesterday`/`by_model` e os
    buckets de período **por `key`** (mesma data soma).
  - contas diferentes ficam separadas.
  - guarda quais servers contribuíram (pra aviso de parcial).
- Tela busca de **todos os servers em paralelo**; server que falha é pulado e a
  tela marca "parcial".

### Tela `screens/Costs.svelte` + rota `#/costs`

- `App.svelte`: nova `route.name === 'costs'` (hash `#/costs`), NavBar com back.
- Entrada: item "Custos" no menu da NavBar da `SessionList` → navega `#/costs`.
- Layout (topo → base):
  1. Seletor de conta: as contas mergeadas (2) + opção "Todas" (soma geral).
  2. Aviso de parcial se algum server não respondeu.
  3. Chips **hoje vs ontem**.
  4. Cards de total: custo, sessões, input/output tokens (abrev K/M/B).
  5. Segment **Dia / Semana / Mês** → tabela do período (período · sessões ·
     tokens · custo · barra relativa ao maior custo).
  6. Seção **por modelo** (custo + sessões por modelo).
- Tudo reflete a conta selecionada, já somando os servers.
- `lib/format.ts`: helper `abbrevNum(n)` → `3.7M` / `1.5B` / `999`.

### Dados / erros

- Fetch on mount + refresh manual (`costs.jsonl` só muda no Stop; sem SSE).
- Nenhum dado (arquivos ausentes) → empty state: "custo aparece após a 1ª sessão
  parar".
- Server offline → parcial, nunca finge total completo.

## Isolamento (unidades)

- `costs.py`: dado config dir → agrega. Sem I/O de rede. Testável sozinho.
- `mergeAccounts` (`lib/costs.ts`): função pura. Testável sozinho.
- `Costs.svelte`: só renderiza o resultado do merge.
- `api.py`: só expõe a rota.

## Testes

- Backend `tests/test_costs.py`:
  - dedup: 2 snapshots do mesmo `session_id` → conta 1 vez.
  - soma dos `by_day` == `totals.cost`.
  - custo de 1 sessão com tokens conhecidos × RATES.
  - 2 config dirs → 2 `AccountCost`.
  - `.claude.json` ausente → fallback label.
- Frontend `lib/costs.test.ts`:
  - mesma conta em 2 servers soma buckets por data.
  - contas diferentes ficam separadas.
  - server faltando → resultado parcial marcado.
- `lib/format` test do `abbrevNum`.

## Fora de escopo (YAGNI)

Filtro de intervalo de datas custom, export CSV, config de preços via UI,
gráfico/chart (tabela + barra basta pra v1).
