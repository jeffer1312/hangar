# Codex `app-server` contract (spike findings)

Investigação de fatos reais contra a instalação local de `codex-cli 0.141.0`, feita
100% em `/tmp` (sandbox read-only, sem tocar no repo). Todas as respostas abaixo são
output real capturado, não suposição. Objetivo: decidir se dá pra integrar o
Codex ao `claude-pocket` via `codex app-server` (JSON-RPC/stdio) ou se o fallback
`codex exec --json` é o caminho mais seguro.

**Veredito: `app-server` funcionou de forma estável na sequência testada** (init →
thread/start → turn/start, resposta em ~5s, sem pendurar). Mas é `[experimental]`,
tem protocolo grande (v1 legado + v2 "real", ~150 tipos), e authstack/threading tem
bastante superfície não testada aqui (só um turno simples, sem tool calls, sem
aprovações, sem erro). Recomendo: **implementar contra app-server v2**, mas manter
`codex exec --json` documentado como fallback simples caso o app-server se mostre
instável em uso real (tool calls, streams longos, cancelamento).

## Versão

```
$ codex --version
codex-cli 0.141.0
```

`codex app-server` é anunciado como `[experimental] Run the app server or related
tooling` no `--help`. Subcomandos: `daemon`, `proxy`, `generate-ts`,
`generate-json-schema`.

## Protocolo: dois níveis (legado "v1" + "v2")

`codex app-server generate-ts --out <dir>` gera:
- arquivos na raiz (`ClientRequest.ts`, `ServerNotification.ts`, `InitializeParams.ts`,
  etc.) — o núcleo do protocolo mais um conjunto pequeno de tipos "v1" antigos
  (`FuzzyFileSearch*`, `GetAuthStatus`, `GetConversationSummary`, `GitDiffToRemote`).
- `v2/*.ts` — o protocolo real e atual (threads, turns, items, contas, mcp, fs, etc.),
  ~150 arquivos de tipos.

`ClientRequest` e `ServerNotification` (na raiz) já misturam os dois: o `method` de
cada variant já usa a nomenclatura v2 (`thread/start`, `turn/start`, ...) e importa
os `Params`/tipos de `./v2/*`. Não existe um "v1 vs v2" a escolher — é só onde os
arquivos de tipo moram. **Use os nomes de método abaixo, todos confirmados
funcionando.**

Comando que gerou os arquivos (precisa `--out <DIR>`, não aceita stdout):
```
codex app-server generate-json-schema --out <dir>   # 40 arquivos .json + 2 schemas agregados
codex app-server generate-ts --out <dir>             # ~150 arquivos .ts
```

## Handshake que funcionou (sequência real)

Transporte: `codex app-server --stdio`, JSON-RPC 2.0, uma mensagem por linha
(newline-delimited, **sem** framing tipo `Content-Length` do LSP). Importante: o
processo só responde se o stdin **permanecer aberto** — fechar o pipe imediatamente
após escrever a primeira linha faz o processo sair sem responder nada (visto na
primeira tentativa, ver "Achados por passo" abaixo). Um driver real precisa manter o
stdin aberto (pipe persistente), não `cat arquivo | codex app-server --stdio`.

### 1. `initialize`

Request:
```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"claude-pocket-spike","title":"spike","version":"0.0.1"},"capabilities":null}}
```

Response real:
```json
{"id":1,"result":{"userAgent":"claude-pocket-spike/0.141.0 (CachyOS Linux Rolling Release; x86_64) tmux/3.7b (claude-pocket-spike; 0.0.1)","codexHome":"/home/jefferson/.codex","platformFamily":"unix","platformOs":"linux"}}
```
Seguido, sem pedir, de uma notification `remoteControl/status/changed` (irrelevante
pro nosso caso — só ignorar).

`InitializeParams = { clientInfo: ClientInfo, capabilities: InitializeCapabilities | null }`
`ClientInfo = { name: string, title: string | null, version: string }`

### 2. `thread/start` (cria/abre a sessão — devolve o `threadId`)

Request:
```json
{"jsonrpc":"2.0","id":2,"method":"thread/start","params":{"cwd":"/tmp","sandbox":"read-only","approvalPolicy":"never"}}
```

Response real (truncada nos campos irrelevantes):
```json
{"id":2,"result":{"thread":{
  "id":"019f5c00-5d7d-7dd2-b2cb-085ca6d76251",
  "sessionId":"019f5c00-5d7d-7dd2-b2cb-085ca6d76251",
  "status":{"type":"idle"},
  "path":"/home/jefferson/.codex/sessions/2026/07/13/rollout-2026-07-13T12-02-35-019f5c00-5d7d-7dd2-b2cb-085ca6d76251.jsonl",
  "cwd":"/tmp","cliVersion":"0.141.0","source":"vscode", "turns":[]
},"model":"gpt-5.4-mini","sandbox":{"type":"readOnly","networkAccess":false},"approvalPolicy":"never", ...}}
```
**`threadId` = `result.thread.id`** (string UUID v7, não `urn:uuid:` prefixado — mandar
um placeholder tipo `__THREAD_ID__` dá erro `-32600 invalid thread id`).

Logo depois chega a notification `thread/started` com o mesmo objeto `thread`
embrulhado em `{"thread": ...}`, mais `mcpServer/startupStatus/updated` (starting →
ready) para o server interno `codex_apps`.

`ThreadStartParams` aceita (todos opcionais exceto nenhum): `model`, `modelProvider`,
`serviceTier`, `cwd`, `approvalPolicy`, `approvalsReviewer`, `sandbox`
(`SandboxMode`), `config`, `serviceName`, `baseInstructions`,
`developerInstructions`, `personality`, `ephemeral`, `sessionStartSource`,
`threadSource`.

### 3. `turn/start` (envia mensagem do usuário / roda um turno)

Request:
```json
{"jsonrpc":"2.0","id":3,"method":"turn/start","params":{"threadId":"019f5c00-5d7d-7dd2-b2cb-085ca6d76251","input":[{"type":"text","text":"responda apenas: ok","text_elements":[]}]}}
```
`UserInput` é uma union: `{type:"text", text, text_elements}` |
`{type:"image", url, detail?}` | `{type:"localImage", path, detail?}` |
`{type:"skill", name, path}` | `{type:"mention", name, path}`.

Response imediata (turno ainda rodando):
```json
{"id":3,"result":{"turn":{"id":"019f5c00-654a-7051-95b7-706bc91d7ae2","items":[],"itemsView":"notLoaded","status":"inProgress",...}}}
```

Depois disso chega uma sequência de **notifications** (sem id, `method` + `params`),
nesta ordem exata observada:

1. `thread/status/changed` → `{"status":{"type":"active","activeFlags":[]}}`
2. `turn/started` → `{"threadId","turn":{...status:"inProgress"}}`
3. `item/started` (item `userMessage`, eco da mensagem enviada)
4. `item/completed` (mesmo item `userMessage`)
5. `item/started` (item `reasoning`, vazio nesse caso — modelo `gpt-5.4-mini` com
   `reasoningEffort: "low"`)
6. `item/completed` (mesmo item `reasoning`)
7. `item/started` (item `agentMessage`, `text:""`, `phase:"final_answer"`)
8. `item/agentMessage/delta` → `{"threadId","turnId","itemId","delta":"ok"}` — **este é
   o equivalente ao streaming de texto** (pode vir mais de um delta por resposta longa)
9. `item/completed` (item `agentMessage`, agora `text:"ok"` completo)
10. `thread/tokenUsage/updated` → contagem de tokens (shape abaixo)
11. `account/rateLimits/updated` → rate limits da conta
12. `thread/status/changed` → `{"status":{"type":"idle"}}`
13. `turn/completed` → `{"threadId","turn":{...status:"completed","durationMs":4385}}`

**Nomes de notification confirmados e mapeados pro que a task pediu:**
- turno iniciado: `turn/started`
- turno concluído: `turn/completed`
- texto do assistant (delta/streaming): `item/agentMessage/delta`
- texto do assistant (item completo): `item/completed` com `item.type == "agentMessage"`
- contagem de tokens: `thread/tokenUsage/updated`
- rate limits da conta: `account/rateLimits/updated`
- status do thread (idle/active): `thread/status/changed`
- item genérico (raciocínio, exec de comando, etc.) começou/terminou:
  `item/started` / `item/completed`, com `item.type` variando
  (`userMessage`, `reasoning`, `agentMessage`, e por extensão do schema também
  `commandExecution`, `fileChange`, `mcpToolCall`, etc. — não exercitados aqui porque
  o prompt não gerou tool calls, mas os tipos existem em `ThreadItem`/`ServerNotification`
  gerados: `item/commandExecution/outputDelta`,
  `item/commandExecution/terminalInteraction`, `item/fileChange/outputDelta`,
  `item/fileChange/patchUpdated`, `item/mcpToolCall/progress`, `item/plan/delta`,
  `item/reasoning/summaryTextDelta`, `item/reasoning/summaryPartAdded`,
  `item/reasoning/textDelta`).

### Shape real de `thread/tokenUsage/updated`

```json
{"method":"thread/tokenUsage/updated","params":{
  "threadId":"019f5c00-5d7d-7dd2-b2cb-085ca6d76251",
  "turnId":"019f5c00-654a-7051-95b7-706bc91d7ae2",
  "tokenUsage":{
    "total":{"totalTokens":14389,"inputTokens":14371,"cachedInputTokens":7552,"outputTokens":18,"reasoningOutputTokens":11},
    "last":{"totalTokens":14389,"inputTokens":14371,"cachedInputTokens":7552,"outputTokens":18,"reasoningOutputTokens":11},
    "modelContextWindow":258400
  }
}}
```
`ThreadTokenUsage = { total: TokenUsageBreakdown, last: TokenUsageBreakdown, modelContextWindow: number | null }`.

### Shape real de `account/rateLimits/updated`

```json
{"method":"account/rateLimits/updated","params":{"rateLimits":{
  "limitId":"codex","limitName":null,
  "primary":{"usedPercent":0,"windowDurationMins":10080,"resetsAt":1784494806},
  "secondary":null,"credits":null,"individualLimit":null,
  "planType":"plus","rateLimitReachedType":null
}}}
```
**Ressalva:** `secondary`, `credits` e `individualLimit` vieram `null` nesta conta
(`planType: "plus"`). Não dá pra confirmar o shape quando não-null sem uma conta que
os exercite — assumir que podem ter o mesmo shape de `primary` (`usedPercent`,
`windowDurationMins`, `resetsAt`) é razoável mas não verificado.

## Auth (decisivo)

```
$ codex login status
Logged in using ChatGPT
```

O handshake completo (`initialize` → `thread/start` → `turn/start`) rodou **sem
pedir login nenhum**, usando a sessão ChatGPT já autenticada no `~/.codex/`. Ou seja:
**`app-server` funciona com o login atual (conta ChatGPT via OAuth), não foi
necessário fornecer API key.** Não testei o caminho de API key (não configurada aqui)
nem o que acontece se a sessão expirar no meio de um turno.

## Layout real do rollout na 0.141.0

**Path é aninhado por data, não flat:**
```
~/.codex/sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<thread-uuid>.jsonl
```
Exemplo real: `/home/jefferson/.codex/sessions/2026/07/13/rollout-2026-07-13T12-02-35-019f5c00-5d7d-7dd2-b2cb-085ca6d76251.jsonl`
— o próprio `thread/start` já devolve esse path em `result.thread.path`, então o
backend não precisa adivinhar/gluear a data: pode usar o path retornado direto.

### Shape das linhas (12 linhas para 1 turno simples, "responda apenas: ok")

Cada linha: `{"timestamp": "...", "type": "<tipo>", "payload": {...}}`. Tipos vistos,
em ordem: `session_meta` (1×), `event_msg` (5×), `response_item` (5×),
`turn_context` (1×).

**Linha 0 é `session_meta`**, sim — confirmado:
```json
{"timestamp":"2026-07-13T15:02:37.646Z","type":"session_meta","payload":{
  "id":"019f5c00-5d7d-7dd2-b2cb-085ca6d76251","cwd":"/tmp",
  "originator":"claude-pocket-spike","cli_version":"0.141.0","source":"vscode",
  "model_provider":"openai","base_instructions":{"text":"...(system prompt completo)..."}
}}
```

`event_msg` tem sub-`type`s próprios (mesma família do `codex exec --json`, ver
abaixo): `task_started`, `user_message`, `agent_message`, `token_count`,
`task_complete`.

### Shape real de `token_count` dentro do rollout (`event_msg.payload`)

```json
{"type":"token_count","info":{
  "total_token_usage":{"input_tokens":14371,"cached_input_tokens":7552,"output_tokens":18,"reasoning_output_tokens":11,"total_tokens":14389},
  "last_token_usage":{"input_tokens":14371,"cached_input_tokens":7552,"output_tokens":18,"reasoning_output_tokens":11,"total_tokens":14389},
  "model_context_window":258400
},"rate_limits":{
  "limit_id":"codex","limit_name":null,
  "primary":{"used_percent":0.0,"window_minutes":10080,"resets_at":1784494806},
  "secondary":null,"credits":null,"individual_limit":null,
  "plan_type":"plus","rate_limit_reached_type":null
}}
```
Nota: no rollout (`event_msg`) os campos vêm em `snake_case`
(`input_tokens`, `window_minutes`) — diferente do JSON-RPC do app-server que vem em
`camelCase` (`inputTokens`, `windowDurationMins`). **Mesmo conteúdo, convenção de
nome diferente conforme a camada** (rollout on-disk vs protocolo JSON-RPC). Isso é
importante: `transcript.py`/parser de rollout do backend precisa de um mapeamento
próprio, não pode reusar tipos do app-server diretamente.

Fixture completa (12 linhas, anonimizada só pelo fato de rodar em `/tmp` com prompt
inócuo) salva em
`backend/tests/fixtures/codex/rollout_sample.jsonl`.

## Fallback: `codex exec --json`

Comando:
```
codex exec --json --skip-git-repo-check -s read-only "responda apenas: ok"
```

Output real (4 linhas, streaming NDJSON no stdout):
```json
{"type":"thread.started","thread_id":"019f5c01-2206-75c3-be11-7e94314d5860"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"ok"}}
{"type":"turn.completed","usage":{"input_tokens":14371,"cached_input_tokens":7552,"output_tokens":16,"reasoning_output_tokens":9}}
```
Sem streaming de delta (só chega o item já completo), sem rate limits, sem token
detalhado por thread (`modelContextWindow` ausente). Muito mais simples de
consumir (processo roda até o fim e sai — sem precisar manter handshake nem stdin
aberto), mas perde granularidade: não dá pra mostrar "digitando..." char-a-char, só
"turn started" → "turn completed" com o texto final pronto.

`stderr` mostrou avisos inofensivos de skills mal-formadas do usuário (não
relacionados ao Codex/protocolo):
```
ERROR codex_core::session::session: failed to load skill .../huashu-design/SKILL.md: invalid description: exceeds maximum length of 1024 characters
```

## Flags de sandbox confirmados

`codex exec --help` mostra `-s, --sandbox <SANDBOX_MODE>` com valores
`read-only | workspace-write | danger-full-access` — **não** é
`-c sandbox_mode=read-only` (isso existe como escape hatch genérico `-c key=value`,
mas o flag dedicado é `-s`/`--sandbox`). `--skip-git-repo-check` existe e foi usado
em todos os testes (rodando fora de um repo git, em `/tmp`).

Para `thread/start` (app-server), o campo equivalente é `sandbox: "read-only"`
dentro de `params` (não um flag CLI).

## Achados por passo (comandos que funcionaram vs falharam)

1. **Schema**: `codex app-server generate-json-schema > arquivo` **falhou**
   (`error: the following required arguments were not provided: --out <DIR>`).
   Corrigido com `--out <dir>` → gerou 40 arquivos + 2 agregados
   (`codex_app_server_protocol.schemas.json` 526K,
   `codex_app_server_protocol.v2.schemas.json` 440.6K).
2. **TS bindings**: mesmo problema, mesma correção (`--out <dir>`, não stdout) →
   ~150 arquivos `.ts`.
3. **Handshake stdio, 1ª tentativa**: `cat handshake.jsonl | timeout 25 codex
   app-server --stdio` **não devolveu nada** (stdout e stderr vazios, exit 0) — o
   processo saiu assim que o stdin fechou, sem dar tempo de responder. Corrigido
   mantendo o stdin aberto (`(cat arquivo; sleep 5) | ...` ou, no driver final, um
   `subprocess.Popen` com pipe persistente) → respondeu em <1s.
4. **Handshake completo (init → thread/start → turn/start)**: precisou de um driver
   real (script Python com `subprocess.Popen`, thread de leitura, extração do
   `threadId` da resposta antes de montar a próxima mensagem) porque o `threadId` é
   dinâmico (UUID v7 gerado pelo server) — um pipe estático com JSON pré-formatado
   não serve para esse passo. Funcionou de primeira depois disso, ~5s de ponta a
   ponta pro turno completo.
5. **`codex login status`**: funcionou direto, sem flag extra —
   `Logged in using ChatGPT`.
6. **Rollout**: gerado tanto pelo handshake do app-server quanto pelo `codex exec`
   direto; ambos gravam em `~/.codex/sessions/YYYY/MM/DD/`. Cópia feita a partir do
   rollout gerado pelo app-server (mais completo, 12 linhas com `response_item` e
   `turn_context`, vs. as poucas linhas que `codex exec` sozinho geraria).

## Ressalvas (por que não é DONE liso)

- App-server só foi exercitado num turno trivial, sem tool calls, sem aprovação,
  sem erro, sem cancelamento (`turn/interrupt`), sem reconexão. O protocolo tem
  *muito* mais notification types (~70) do que os ~13 vistos num turno simples —
  documentei os nomes existentes no schema mas não confirmei o payload de todos.
- `rate_limits.secondary` / `.credits` / `.individual_limit` vieram `null` nesta
  conta — shape não confirmado quando não-null.
- Não testei o que acontece se o app-server travar no meio de um turno real (o
  brief pedia pra registrar isso *se* acontecesse; não aconteceu nos testes feitos,
  mas a amostra é pequena — 2 handshakes completos).
- Não testei autenticação via API key (só a sessão ChatGPT já logada foi
  exercitada).
