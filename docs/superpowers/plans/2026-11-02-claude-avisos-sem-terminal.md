# Claude sem terminal: avisos que hoje caem calados

Status: ready-for-agent — entrevista fechada em 02/11/2026, nenhuma Task iniciada.
Base: `main` em `bfabb2585`, Claude Code 2.1.316 (tipos do `@anthropic-ai/claude-agent-sdk@0.3.316`, `sdk.d.ts`).
Trabalho em worktree própria (nome confirmado na execução). Só a fase 1 do levantamento; elicitation,
saída de `/comando` com terminal, ações (`stop_task`, contexto, MCP) e o Expo ficam fora.

## Objetivo

O ator do Claude sem terminal (`crates/hangar-server/src/runtime/claude.rs`) joga fora, no `_ => return`
de `on_system` (:1192), eventos que a pessoa precisa ver: nova tentativa da API, MCP ou plugin que não
subiu, hook lento ou com falha, aviso da própria CLI, troca de modelo por recusa e o motivo do fim do
turno. Cada um passa a sair por um canal que já existe, como o Claude faz no terminal: mostra uma vez e
não fica repetindo.

## Fonte dos requisitos

Decisões da entrevista (02/11/2026, rodada em texto; resposta "todos recomendado menos a 2 que é B"):

| # | Evento | Decisão |
|---|---|---|
| R0 | hook | Hook só aparece se demorar mais de ~3 s (1A da conversa anterior); hook rápido e bem-sucedido vai só ao diário |
| R1 | `system/api_retry` | Rótulo desde a 1ª tentativa: `Nova tentativa N de M em X s · erro S`; some quando a resposta volta |
| R2 | MCP/plugin que não subiu | Nota única **por vida do processo**: volta quando o processo do Claude sobe de novo (relançamento por ociosidade incluído) |
| R3 | hook com falha (não bloqueio) | Nota `Hook falhou — …`, falhas repetidas do mesmo hook viram uma nota com o total, como o Codex (aqui `(N× neste turno)`, ver Task 3) |
| R4 | `system/notification` | Só prioridade `high`/`immediate` vira nota única por chave; o resto vai ao diário |
| R5 | `system/model_refusal_fallback` | Nota a cada ocorrência: `O <original> recusou; esta resposta veio do <fallback>` |
| R6 | `terminal_reason` do `result` | Motivo anormal vira nota curta legível; o mesmo texto entra no detalhe da faixa de erro existente |
| R7 | escopo | Só fase 1 (2A) |

Regras do repositório que valem aqui: "Aviso na tela só para ação que falhou ou bloqueio real" e
"Nada nasce no Python" (`CLAUDE.md`); "Regras vigentes" de `docs/decisoes/harnesses.md`.

## Contratos lidos (CLI 2.1.316, `sdk.d.ts`)

- `system/api_retry`: `attempt`, `max_retries`, `retry_delay_ms`, `error_status` (`null` = sem resposta
  HTTP), `error`, `no_response?{waited_ms,retry_wait_ms}`.
- `system/hook_started`: `hook_id`, `hook_name`, `hook_event`. `system/hook_progress`: idem +
  `stdout`, `stderr`, `output`. `system/hook_response`: idem + `exit_code?`, `outcome`
  `success|error|cancelled`. Só chegam com `--include-hook-events`, menos SessionStart e Setup, que
  chegam sempre (hoje caem calados).
- `system/notification`: `key`, `text`, `priority` `low|medium|high|immediate`, `color?`, `timeout_ms?`.
- `system/model_refusal_fallback`: `original_model`, `fallback_model`, `scope?` `session|local`,
  `direction`, `api_refusal_category?`, `retracted_message_uuids?`, `content`.
  `system/model_refusal_no_fallback`: `original_model`, `api_refusal_category?`,
  `api_refusal_explanation?`, `content`.
- `system/init`: `mcp_servers[{name,status}]` com `status` `connected|failed|needs-auth|pending|disabled`
  e `error?`; `plugin_errors?[{plugin,type,message}]` (chave omitida sem erro).
- `result.terminal_reason`: `blocking_limit`, `rapid_refill_breaker`, `prompt_too_long`, `image_error`,
  `model_error`, `api_error`, `malformed_tool_use_exhausted`, `aborted_streaming`, `aborted_tools`,
  `stop_hook_prevented`, `hook_stopped`, `tool_deferred`, `max_turns`, `background_requested`,
  `completed`, `budget_exhausted`, `structured_output_retry_exhausted`, `tool_deferred_unavailable`,
  `turn_setup_failed`.

O que o `sdk.d.ts` não diz e a Task 0 fixa com linhas reais: o formato de `hook_name`; o `outcome` e o
`exit_code` de hook com `exit 1` e com `exit 2` (bloqueio); se um MCP com comando inexistente chega no
`init` como `failed` ou como `pending`.

## Arquitetura

Nada muda nos clientes, no Python nem no i18n: os três canais já são desenhados pelo nativo e pelo Expo.

| Canal | Como sai hoje | Usado por |
|---|---|---|
| Rótulo passageiro | `StateEvent.label` montado em `public_state` (`claude.rs:254`) | R1, R0 (hook lento) |
| Nota na conversa | `policy("local_output",{text,hook_error?,notice?})` → `AppendLocal` → `ChatEvent local-*` | R2–R6 |
| Faixa de erro | `state.problema` + `problema_detalhe` (`headless_turno_erro`, `claude.rs:981`) | R6 |
| Diário | `policy("unknown_private",{kind,event})` (`local_policy.rs:49`, teto de 30 por tipo) | todo evento novo |

Peças novas no `ClaudeEngine`, todas por vida do processo (o motor nasce com o processo em
`actor.rs:137`, então "por vida do processo" sai de graça):

- `notice_once(key,text)`: emite `local_output` só se `self.unknown.insert(format!("notice:{key}"))` for
  verdadeiro. Reaproveita o conjunto `unknown`, que já é o "visto nesta vida" do motor.
- **Retrato não reemite nota.** O motor também nasce quando o Rust reinicia e adota o processo vivo, e
  `hydrate` repassa o `init` gravado por `apply` (`claude.rs:522`). A nota já está na conversa desde a
  primeira vez (`AppendLocal`). Por isso, um campo `replaying: bool` fica ligado durante o laço do `init`
  e do `rate_limit` em `hydrate`, e com ele `notice_once` só marca a chave, sem emitir. Limite aceito: um
  processo que subiu com o servidor fora não mostra a nota daquela vida.
- `retry_label: Option<String>`: rótulo da nova tentativa. Sai em qualquer `stream_event`, `assistant` e
  `user`, **antes** do filtro de `parent_tool_use_id` (`claude.rs:783`), porque o `api_retry` de
  subagente ou de chamada de fundo não traz esse campo e o fluxo dele é descartado ali. Sai também em
  `result`, `cano_saiu`, `reattach` (`claude.rs:409`) e `reset_conversation`.
- `hooks_running: BTreeMap<String,Hook>` (`hook_id` → nome, evento, início) e `hook_streaks` no formato
  do Codex (`codex.rs:318`, `:1310-1342`). `hook_response` de `hook_id` desconhecido não mexe no rótulo; se for falha, a nota sai igual.
- O fim de `on_system` passa a mandar ao diário o que não tem ramo (`unknown_private`,
  `kind: "system/{subtype}"`), no lugar do `_ => return` calado. `hook_progress` é ignorado de forma
  explícita, para não gastar o teto do diário.
- Os ramos novos só chamam `changed` quando o rótulo muda; evento de hook sem efeito na tela volta sem
  desenhar.
- Precedência do rótulo em `public_state`: `retry_label` > hook rodando há ≥ 3 s > `Iniciando sessão…`
  > `self.label`. O contador de tempo e tokens do turno continua sendo acrescentado depois.
- `next_deadline` (`claude.rs:1329`) ganha `início + 3.0` só dos hooks com esse prazo **ainda no futuro**
  (`> self.clock.monotonic_s`). Prazo vencido devolvido de novo deixaria o laço do ator girando sem
  parar. `on_tick` (`claude.rs:1274`) chama `changed` quando o prazo vence.

Todo texto mostrado é montado no servidor em português, como o Codex já faz (`codex.rs:1339`), com o
prefixo `⚙️ ` das notas do Claude (`claude.rs:1061`). Nenhum código novo de `problema`.

## Ordem e paralelismo

Task 0 primeiro (fixa os formatos). Tasks 1 a 5 mexem no mesmo arquivo (`runtime/claude.rs`) e no mesmo
arquivo de teste: rodam em sequência. Task 6 (regra) e Task 7 (prova ao vivo) no fim.

### Task 0: Gravar linhas reais da CLI 2.1.316

Status: ready-for-agent
Arquivos: `crates/hangar-server/tests/fixtures/claude_events/*.jsonl` (novos).

- [x] **Step 1: Montar a pasta descartável** no scratchpad, fora de qualquer repo: `.mcp.json` com um
  servidor `quebrado` de comando inexistente; `.claude/settings.json` com três hooks de comando:
  `PreToolUse` com `sleep 4; exit 0`, `PostToolUse` com `echo falhou >&2; exit 1` e `UserPromptSubmit`
  com `exit 0`. Plugin quebrado: `--plugin-dir` apontando para uma pasta sem manifesto.
- [x] **Step 2: Rodar** `claude -p --output-format stream-json --input-format stream-json --verbose
  --include-partial-messages --include-hook-events --plugin-dir <pasta-ruim>` nessa pasta, com o prompt
  "rode ls" pela entrada padrão como linha JSON `{"type":"user","message":{"role":"user","content":"rode ls"}}`,
  e guardar a saída. Repetir o `PostToolUse` com `exit 2` para ver o bloqueio, e com `"async": true` para
  ver se hook assíncrono emite `hook_started`; se emitir, ele fica sem rótulo de lento (só diário).
- [x] **Step 3: Provocar `api_retry`** com `ANTHROPIC_BASE_URL` apontando para um servidor local de
  scratchpad que responde 529 (script descartável no scratchpad, nunca no repo).
- [x] **Step 4: Recortar** as linhas `system/init`, `hook_*`, `api_retry` e `result` em arquivos
  `.jsonl` pequenos, sem `session_id`, caminho pessoal nem texto da conversa. Os testes os leem por
  `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_events/…")`, como
  `tests/it/archive_history.rs:17`.
- [x] **Step 5: Anotar no próprio plano** o formato de `hook_name`, o `outcome`/`exit_code` de `exit 1`
  e `exit 2`, e o `status` do MCP quebrado no `init`.

  Achados (02/11/2026, CLI 2.1.316):
  - `hook_name` é `Evento:matcher` (`PreToolUse:Bash`) ou só o evento quando não há matcher
    (`UserPromptSubmit`). Rótulo: `Rodando hook {hook_name}…`; detalhe da falha: `{hook_name}: {motivo}`.
  - `exit 1` → `outcome: "error"`, `exit_code: 1`, motivo no `stderr`. `exit 2` → `outcome: "error"`,
    `exit_code: 2`. A regra do plano (exit 2 só diário) vale como está.
  - Hook `async` emite `hook_started` igual aos outros, sem campo que o distinga, e o `hook_response`
    pode chegar depois do `user` do resultado da ferramenta. Não há como separá-lo: se passar de 3 s ele
    também vira rótulo, e o `result` limpa o que ficar aberto.
  - `UserPromptSubmit` emite `hook_started`/`hook_response` **antes** do `init`.
  - MCP com comando inexistente chega no `init` como `failed`, **sem** `error`: o bloqueio condicional
    não se aplica. Conector do claude.ai ainda conectando chega como `pending` (sem nota).
  - `--plugin-dir` para pasta sem manifesto é ignorado calado; manifesto corrompido entra em
    `plugin_errors` com `plugin: "inline[0]"` e `message` longa com o caminho (corte em 300 caracteres).
  - `api_retry`: `error` é texto curto (`overloaded`, `unknown`); sem resposta → `error_status: null`.
    Depois do último retry vem `result` com `subtype: "success"`, `is_error: true`,
    `terminal_reason: "api_error"`: já cai no ramo da faixa `headless_turno_erro`.

Prova: `ls crates/hangar-server/tests/fixtures/claude_events/` lista `init.jsonl`, `hooks.jsonl`,
`api_retry.jsonl` e `result.jsonl`; `rg -n -F '/home/' crates/hangar-server/tests/fixtures/claude_events/` não acha nada. Os `session_id` dos fixtures são sintéticos (`00000000-0000-4000-8000-…`).

**Bloqueio condicional:** se o MCP quebrado chegar no `init` como `pending` e nenhum evento posterior
disser `failed`, R2 não se cumpre só com o `init`. Aí a execução para e pergunta: um `mcp_status` único
alguns segundos depois do `init`, ou só o `init`. Não há consulta periódica (regra "sem polling").

### Task 1: MCP e plugin que não subiram (R2)

Status: ready-for-agent
Arquivos: `crates/hangar-server/src/runtime/claude.rs` (`on_system`, ramo `init`; helper `notice_once`),
`crates/hangar-server/tests/it/runtime_claude.rs`.

- Para cada `mcp_servers[]` com `status == "failed"`:
  `⚙️ O servidor MCP \`<name>\` não subiu: <error>` (sem `: <error>` quando vazio).
- `status == "needs-auth"`: `⚙️ O servidor MCP \`<name>\` precisa de login`.
- Para cada `plugin_errors[]`: `⚙️ O plugin \`<plugin>\` não carregou: <message>`.
- `connected`, `pending` e `disabled` não geram nota. O `init` inteiro, quando tem falha, vai também ao
  diário (`unknown_private`, `kind: "claude_init_problem"`).
- Cenários: `init` com um `failed` → uma nota; o mesmo `init` de novo no mesmo motor → nenhuma; motor novo
  (processo novo) com o mesmo `init` ao vivo → a nota volta; motor novo com o `init` vindo de `hydrate`
  (Rust reiniciado, mesmo processo) → nenhuma; `init` sem falha → nenhuma.
- Junto, o `_ => return` de `on_system` vira diário (`system/{subtype}`), com `hook_progress` ignorado.

- [x] **Step 1: RED** `notice_init_mcp_failed_once_per_process` com a linha de `init.jsonl`.
- [x] **Step 2: GREEN** `notice_once` + leitura de `mcp_servers`/`plugin_errors`.
- [x] **Step 3: RED/GREEN** `notice_init_plugin_error` e `notice_init_clean_is_silent`.
- [x] **Step 4: RED/GREEN** `notice_init_hydrate_does_not_repeat` (retrato do `mods_support::cano_snapshot_json()`
  com o `init` trocado pela linha de `init.jsonl`) e `notice_unhandled_system_goes_to_diary`.

Prova: `scripts/cargo-pinned test -p hangar-server --test it notice_init` → os três passam.

### Task 2: Nova tentativa da API (R1)

Status: ready-for-agent
Arquivos: `claude.rs` (`on_system`, `public_state`, `on_line`/`result`/`cano_saiu`, `reset_conversation`),
teste.

- Texto: `Nova tentativa {attempt} de {max_retries} em {ceil(retry_delay_ms/1000)} s · erro {error_status}`;
  `error_status` nulo → `· sem resposta da API`.
- Cada `api_retry` vai ao diário (`kind: "claude_api_retry"`). Nenhuma nota: tentativa que deu certo some
  sem rastro na tela.
- Cenários: `api_retry` com turno aberto → `public_state.label` começa com o texto; um `stream_event`
  depois → o texto some; `stream_event` de subagente (com `parent_tool_use_id`) → também some; `result`
  → some; dois `api_retry` seguidos → mostra o segundo.

- [x] **Step 1: RED** `notice_api_retry_label_until_stream` com a linha de `api_retry.jsonl`.
- [x] **Step 2: GREEN** campo `retry_label`, precedência em `public_state`, limpeza em todos os pontos da Arquitetura.
- [x] **Step 3: RED/GREEN** `notice_api_retry_without_status`.

Prova: `scripts/cargo-pinned test -p hangar-server --test it notice_api_retry` → passa.

### Task 3: Hooks: lento vira rótulo, falha vira nota agrupada (R0, R3)

Status: ready-for-agent
Revisão: sim — prazo (3 s no relógio do motor, `next_deadline`/`on_tick`) e contrato com a CLI (flag nova
no lançamento).
Arquivos: `crates/hangar-server/src/runtime/claude_launch.rs:109-112` (acrescentar
`--include-hook-events` à lista fixa), `claude.rs` (`on_system`, `public_state`, `on_tick`,
`next_deadline`, `result`, `cano_saiu`, `reset_conversation`), teste.

- `hook_started` → `hooks_running[hook_id] = (hook_name, hook_event, agora)`.
- Rótulo do hook mais antigo rodando há ≥ 3 s: `Rodando hook {hook_event} ({hook_name})…` (forma
  final ajustada ao `hook_name` real da Task 0, sem repetir o evento se ele já vier no nome).
- `hook_response`: tira de `hooks_running`; tudo vai ao diário (`kind: "claude_hook"`, sem `stdout`
  inteiro: corta em 2000 caracteres).
  - `outcome == "error"` e `exit_code != 2` → falha. Motivo = primeira linha não vazia de `stderr`, senão
    de `output`, senão `exit {exit_code}`, até 300 caracteres. Chave da sequência =
    `hook_name\0hook_event\0motivo`. Primeira falha: nota pelo mesmo formato do Codex
    (`policy("local_output",{text:"Hook falhou — {detalhe}","hook_error":detalhe,"notice":"hook_failed"})`,
    `detalhe = "{hook_name} · {hook_event}: {motivo}"`); repetições só somam.
  - `exit_code == 2` (bloqueio, o hook decidindo) e `cancelled` → só diário. Ajustar à Task 0 se o
    bloqueio vier com outro `outcome`.
  - `success` não fecha a soma: vários hooks dividem o mesmo `hook_name` (revisão da Task 3). Vai ao
    diário como `claude_hook_ok`, para não gastar o teto de `claude_hook`.
  - Falha de `hook_id` desconhecido (assíncrono que responde depois do `result`) também vira nota.
    Sem `stderr`, `output` nem `exit_code`: `sem mensagem de erro`.
- `result` e `transcript_turn_end` fecham as somas (`Hook falhou — {detalhe} ({N}× neste turno)` quando
  repetiu) e esvaziam `hooks_running`; `cano_saiu`, `reattach` e `reset_conversation` esvaziam os dois sem nota.
- Cenários: hook de 1 s → nenhum rótulo, nenhuma nota; hook aberto e `EngineInput::Tick` aos 3,1 s → rótulo; resposta
  → rótulo some; quatro falhas iguais com um sucesso de outro hook no meio e o `result` → duas notas (a primeira e a de `4× neste turno`); falha
  com `exit_code 2` → nenhuma nota; `next_deadline` com hook aberto devolve `início + 3.0` e, depois de
  vencido, não o devolve mais; `hook_response` de `hook_id` desconhecido → nenhum efeito na tela; `reattach`
  com hook aberto → rótulo some.

- [x] **Step 1: RED** `notice_hook_slow_label_after_three_seconds` (relógio do motor, sem `sleep`).
- [x] **Step 2: GREEN** `hooks_running`, precedência, `next_deadline`, `on_tick`.
- [x] **Step 3: RED/GREEN** `notice_hook_failure_streak` e `notice_hook_block_is_diary_only` com linhas de `hooks.jsonl`.
- [x] **Step 4: Flag no lançamento.** Os snapshots do argv em `crates/hangar-server/src/session_open/cases.rs:57,66,78`
  ganham `--include-hook-events` logo depois de `--replay-user-messages`; eles são a prova da flag.

Prova: `scripts/cargo-pinned test -p hangar-server --test it notice_hook` → passam;
`scripts/cargo-pinned test -p hangar-server --lib session_open` → os snapshots passam com a flag.

### Task 4: Aviso da CLI e troca de modelo por recusa (R4, R5)

Status: ready-for-agent
Arquivos: `claude.rs` (`on_system`), teste.

- `notification` com `priority` `high` ou `immediate` → `notice_once("notification:{key}:{text}", "⚙️ {text}")` (mesmo `key` com texto novo é aviso novo). `low`/`medium` → diário
  (`kind: "claude_notification"`).
- `model_refusal_fallback` → nota a cada ocorrência:
  `⚙️ O {original_model} recusou; esta resposta veio do {fallback_model}`. Com `scope == "local"`
  (subagente, `/btw`) acrescenta ` (só esta resposta; a sessão segue no {original_model})`. Também vai ao diário.
  A remoção das mensagens em `retracted_message_uuids` fica fora (registrar na nota de PR).
- `model_refusal_no_fallback` → nota a cada ocorrência: `⚙️ O {original_model} recusou responder` +
  `: {api_refusal_explanation}` quando houver. Mesma regra do R5 (é a outra metade do mesmo evento),
  conferida com ele na entrega.
- Cenários: `high` duas vezes com o mesmo texto → uma nota; `medium` → nenhuma; fallback duas vezes → duas
  notas; `scope local` → sufixo.

- [x] **Step 1: RED/GREEN** `notice_notification_high_once`, `notice_notification_low_is_diary_only`.
- [x] **Step 2: RED/GREEN** `notice_refusal_fallback_each_time`, `notice_refusal_without_fallback`.

Prova: `scripts/cargo-pinned test -p hangar-server --test it notice_` → todos os `notice_*` passam (o
`cargo test` aceita um filtro só).

### Task 5: Motivo do fim do turno (R6)

Status: ready-for-agent
Arquivos: `claude.rs` (ramo `result`, função livre `terminal_reason_text`), teste.

- `terminal_reason_text(reason) -> Option<&str>`; `None` para o fim normal ou parado pela pessoa:
  `completed`, `aborted_streaming`, `aborted_tools`, `background_requested`, `tool_deferred`.
  Textos:

  | motivo | texto |
  |---|---|
  | `prompt_too_long` | a conversa passou do tamanho do contexto |
  | `image_error` | a API recusou uma imagem |
  | `model_error` | erro do modelo |
  | `api_error` | erro da API |
  | `malformed_tool_use_exhausted` | o modelo errou o formato da ferramenta várias vezes |
  | `stop_hook_prevented`, `hook_stopped` | um hook parou o turno |
  | `max_turns` | limite de passos do turno |
  | `budget_exhausted` | orçamento do turno esgotado |
  | `structured_output_retry_exhausted` | a resposta estruturada falhou várias vezes |
  | `tool_deferred_unavailable` | a ferramenta adiada não está disponível |
  | `turn_setup_failed` | o turno não conseguiu começar |
  | outro valor | o próprio valor (motivo novo da CLI não some) |

- Corrigido pela revisão final: `blocking_limit` é o limite de contexto ("a conversa chegou ao limite do
  contexto") e `rapid_refill_breaker` a compactação automática em laço ("a compactação automática entrou
  em laço"). O limite de uso chega como `api_error`: a nota e o prefixo da faixa saem quando o turno
  gerou `Effect::UsageLimit` ou é `headless_sem_login`. `api_error` leva o status: "erro da API (529)".
- Motivo anormal → nota `⚙️ Turno parado: {texto}` a cada ocorrência. No `result` de erro que já monta a
  faixa (`claude.rs:981-983`), o `problema_detalhe` passa a começar pelo texto:
  `{texto} — {subtype}: {result}`.
- Cenários: `success` + `completed` → nada novo; `success` + `stop_hook_prevented` → nota, sem faixa;
  erro + `prompt_too_long` → nota e detalhe da faixa começando por "a conversa passou"; `aborted_tools` →
  nada; `blocking_limit` → nenhuma nota; motivo desconhecido `xyz` → `Turno parado: xyz`.

- [x] **Step 1: RED/GREEN** `notice_terminal_reason_hook_stop` e `notice_terminal_reason_normal_is_silent`.
- [x] **Step 2: RED/GREEN** `notice_terminal_reason_in_error_strip` e `notice_terminal_reason_unknown_value`.

Prova: `scripts/cargo-pinned test -p hangar-server --test it notice_terminal_reason` → passam.

### Task 6: Regra em `docs/decisoes/harnesses.md`

Status: ready-for-agent
Arquivos: `docs/decisoes/harnesses.md` ("Regras vigentes" + entrada datada).

- [x] **Step 1: Regra vigente**, uma só: "Evento da CLI do Claude sem terminal escolhe um canal que já
  existe: rótulo passageiro (o que some sozinho), nota na conversa (falha ou mudança que a pessoa
  precisa saber), faixa (só o erro do turno) ou diário. Nota de configuração sai uma vez por vida do
  processo; nota de ocorrência (recusa, motivo do fim, falha de hook agrupada) sai quando acontece.
  Nada cai no `_ => return` sem ir ao diário."
- [x] **Step 2: Entrada** "Avisos do Claude sem terminal (02/11/2026)" com a tabela R0–R6, os formatos
  da Task 0 e o motivo do 2B (relançamento por ociosidade é invisível, mas a pessoa aceitou rever a nota).

Prova: `rg -n "Avisos do Claude sem terminal" docs/decisoes/harnesses.md` acha a regra e a entrada.

### Task 7: Prova ao vivo (verificação manual)

Status: ready-for-human — precisa do ok dele para trocar o binário e reiniciar o backend.

Feito em 02/11/2026. O backend roda o binário do pacote (`~/.hangar/versions/0.1.0.9074/bin/hangar-server`),
não o de `~/.hangar/bin`; a troca leva o par inteiro (binário, `hangar-server.auth.json` e `auth/<sha>/libhangar_auth.so`),
senão o Python recusa ("sem par válido da biblioteca de autenticação") e assume a porta sozinho.
Resultado: nota de hook e soma `(3× neste turno)` certas; rótulo `Rodando hook PreToolUse:Bash…` aos 6 s com hook de 5 s;
relançamento repete a nota de MCP uma vez. A conta com os plugins do marketplace trouxe ~20 conectores sem login ou
falhos: uma nota por servidor despejava 20 notas por processo, virou uma nota só (`eb95b9016`) e o servidor do
projeto passou a vir antes dos conectores (`b8d30a059`).

- [x] **Step 1: Build** `scripts/cargo-pinned build --release -p hangar-server` na worktree.
- [x] **Step 2: Troca** (com ok): `~/.hangar/bin/hangar-server` → `.bak-2026-11-02-avisos-claude`;
  copiar o novo; `systemctl --user restart hangar-backend.service`.
- [x] **Step 3: Sessão `cx-avisos`** sem terminal numa pasta descartável com o `.mcp.json` quebrado, o
  hook lento e o hook que falha da Task 0. Conferir: nota do MCP uma vez; ao mandar duas mensagens, a
  nota não repete; rótulo do hook lento aparece e some; `Hook falhou` e, no fim do turno, `(N× neste turno)`.
- [x] **Step 4: Relançar** a sessão (`/recarregar`): a nota do MCP volta uma vez (2B).
- [ ] **Step 5: Prints** do nativo e do Expo da mesma conversa, um por um, caminho absoluto no relato.
- [ ] **Step 6: Encerrar** `cx-avisos`; manter ou devolver o binário conforme ele disser.

`api_retry`, `notification`, recusa e `terminal_reason` só têm prova por fixture: não há como provocá-los
numa sessão real sem custo ou conta específica. O relato diz isso.

## Aceitação

- `system/informational` não é tratado hoje (nenhuma ocorrência em `src/`, cai calado no `_ => return`):
  na fase 1 só passa a ir ao diário. O `content` dele com `prevent_continuation` traz o motivo real de um
  hook que parou o turno; trocar o texto genérico do R6 por ele fica para depois da fase 1.
- Cada R0–R6 tem teste que passa (Tasks 1–5) e a regra está em `harnesses.md` (Task 6).
- Nenhum evento novo some sem ir ao diário.
- Nenhum arquivo em `backend/`, `frontend/`, `mobile/`, `desktop-native/` ou `messages/` mudou.
- Prova ao vivo da Task 7 com prints do nativo e do Expo, ou o motivo de não ter rodado.
- Bateria do repositório (`scripts/verificar-local`) só quando ele pedir, antes do push.

## Riscos

- `--include-hook-events` multiplica as linhas por chamada de ferramenta. O diário (log privado) tem teto
  de 30 por sessão, geração e tipo (`local_policy.rs:681`, `:726-727`): não enche, mas do 31º hook em
  diante daquela vida o diário não tem mais o registro. "Vai ao diário" significa isso. O processamento é
  O(1) por linha e não redesenha sem mudança de rótulo.
- A nota de recusa não retira a resposta recusada da conversa (fica para a fase de ações).
- Se o `init` não trouxer o MCP como `failed`, ver o bloqueio condicional da Task 0.
