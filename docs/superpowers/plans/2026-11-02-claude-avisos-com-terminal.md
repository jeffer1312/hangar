# Claude com terminal: avisos da CLI pelo transcript

Status: ready-for-agent — D1 decidida (A) em 02/11/2026; plano corrigido pela revisão crítica e pela
conferência de símbolos; execução inline.
Base: `feat/claude-headless-notices`, Claude Code 2.1.316. Mesma branch do sem terminal
(`2026-11-02-claude-avisos-sem-terminal.md`).

## Objetivo

Levar ao chat do Claude **com terminal** os avisos da fase 1 que o transcript grava e o leitor
`crates/hangar-server/src/transcript/claude.rs` hoje joga fora. Só por transcript ou plugin: **nunca lendo
a tela do terminal**; o que não sair por esses dois fica de fora.

## Fonte dos requisitos

Decisões do usuário em 02/11/2026 ("ok" = 1A 2A 3A 4A; D1 = A) e a regra "nada de ler da tela; se não der
por plugin, deixa pra lá":

| # | Aviso | Decisão |
|---|---|---|
| T1 | nova tentativa da API | nota na primeira falha e fecho quando a resposta volta (D1-A) |
| T2 | hook que falha | nota igual à do sem terminal, com a soma por turno |
| T3 | motivo do fim do turno | medir o `reason` do `turn.complete` do plugin; se servir, plano à parte |
| T4 | recusa do modelo | nota "o modelo recusou responder", sem o modelo de troca |
| fora | MCP/plugin que não subiu, hook lento, notificação da CLI, modelo de troca | sem transcript nem evento de plugin no 2.1.316 |

## Contratos lidos (transcripts reais das contas, 02/11/2026)

- **Modo da linha:** `entrypoint` = `"cli"` (terminal) ou `"sdk-cli"` (sem terminal, `-p`). Um mesmo
  transcript pode ter os dois (conversa retomada no outro modo). O sem terminal já mostra esses avisos pelo
  ator (`runtime/claude.rs`): o leitor só monta nota em linha `"cli"`.
- **Nova tentativa:** `{"type":"system","subtype":"api_error","retryAttempt":N,"maxRetries":M,"retryInMs":X,
  "error":{"status":529|null,"formatted":"529 Overloaded"|"Connection refused — …"}}`, uma linha por tentativa,
  cada uma com `parentUuid` = a anterior (corrente).
- **Erro final da API:** `assistant` com `isApiErrorMessage: true` e texto "API Error: …": o leitor já mostra.
- **Hook que falha:** anexo `hook_non_blocking_error` (`hookName` `PostToolUse:Bash`, `hookEvent`, `stderr` com o
  prefixo "Failed with non-blocking status code: ", `exitCode`). `hook_blocking_error`, `hook_cancelled` e
  `stop_hook_summary.hookErrors` (o bloqueio do Stop já aparece por `hook_additional_context`, `claude.rs:473`)
  ficam de fora.
- **Fim de turno:** `system/turn_duration`, mas **não** aparece em turno interrompido (medido: duas
  interrupções sem ele). O turno também fecha no próximo prompt real da pessoa.
- **Recusa:** `assistant` com `message.stop_reason: "refusal"`.
- **`turn.complete` do plugin:** `plugins/hangar/hooks/state.ts:120` manda `motivo: e.reason` a
  `POST /api/plugin/state` (`plugin/routes.rs:72`) → `plugin/store.rs:549`, só em memória: medir pelo próprio
  plugin, nunca pela tela.

## Arquitetura

- Peça nova `NoticeFold` em `transcript/claude.rs`, com o estado por arquivo: falhas de hook do turno
  (`BTreeMap` por `hookName`, ordem fixa), grupo de tentativas aberto e o fim do turno.
- **Só onde é pedida.** `LineParser::with_notices()` liga o fold; sem ela o `feed` fica como hoje. Ligam:
  o fluxo ao vivo do chat (`tail.rs`, `FileTail`/`backfill`) e o histórico (`history.rs`, laço de `parse_from`
  :267-323). Ficam de fora, de propósito, os que procuram a "última resposta" ou o texto da pessoa e seriam
  enganados por uma nota: `push/watch.rs:223`, `loops/mod.rs:68`, `tail.rs` `last_committed` (:229),
  `session_write/resume.rs:49`, `subagents.rs:959`, `history.rs:192` (`each_event`), a busca
  (`search/ingest.rs:318`, que não indexa notas, igual ao sem terminal), a transferência e o arquivo.
- O histórico pula anexos sem abrir o JSON (`silent_attachment_timestamp`, `claude.rs:124-134`):
  `hook_non_blocking_error` entra na lista que passa (:129).
- Leitura que começa no meio do arquivo: o `seed` (`mod.rs:96`) também alimenta o fold, e o fold só emite
  nota de hook depois de ver o começo de um turno. O grupo de tentativas abre só com `retryAttempt == 1`.
- Notas, com o formato que os clientes já desenham e ids estáveis:
  - falha de hook: `Notice`, `text: "hook_failed"`, `hook_error: "{hookName}: {motivo}"`, id `hookfail:{uuid}`
    da primeira falha; fecho `hookfail-sum:{uuid da linha que fechou}:{md5_8(hookName)}` com
    `"{detalhe} ({N}× neste turno)"`;
  - nova tentativa: `AssistantMsg` "⚙️ A API falhou ({formatted}); nova tentativa 1 de {M}", id `retry:{uuid}`;
    fecho, na primeira `assistant` sem `isApiErrorMessage` depois de N > 1 tentativas: "⚙️ A API voltou na
    {N+1}ª tentativa", id `retry-ok:{uuid da assistant}`. O grupo fecha só em `assistant`/`user` (anexo
    intercalado não fecha); terminou em `isApiErrorMessage`: sem fecho (o erro já aparece);
  - recusa: `AssistantMsg` "⚙️ O modelo recusou responder", id `refusal:{uuid}`.

## Tasks

### Task 0: Fixtures e medida do `turn.complete`

Status: ready-for-agent

- [ ] **Step 1: Recortar** de transcripts reais (sem `sessionId`, caminho pessoal nem texto de conversa)
  para `crates/hangar-server/tests/fixtures/claude_events/terminal_*.jsonl`: `api_error` que voltou e que
  terminou em `isApiErrorMessage`; `hook_non_blocking_error` repetido com `turn_duration`; turno interrompido
  (sem `turn_duration`); transcript misto `cli`/`sdk-cli`; falha de hook gravada com `sdk-cli`; recusa.
  Onde a conta não tiver o caso com `cli`, montar a partir do real trocando só o `entrypoint`.
- [ ] **Step 2: Medir o `reason`** numa sessão `cx-avisos-term` com terminal: log temporário no próprio plugin
  (arquivo no scratchpad, removido depois) com o `e.reason` de um turno normal e de um interrompido.

Prova: arquivos existem e `rg -n '/home/|"sessionId"' …/terminal_*.jsonl` não acha nada; valores de `reason`
anotados aqui. Se `reason` não distinguir motivo anormal, T3 fica com o erro final da API, que já aparece.

### Task 1: `NoticeFold`, filtro `cli` e falha de hook (T2)

Status: ready-for-agent · Revisão: sim — contrato entre o leitor ao vivo e o histórico.

- Ordem: (1) `with_notices()` e o filtro `entrypoint == "cli"`; (2) `hook_non_blocking_error` passando por
  `silent_attachment_timestamp`; (3) soma e fechos (`turn_duration` ou prompt real); (4) `seed` alimenta o fold.
- Cenários: três falhas e `turn_duration` → duas notas; turno interrompido e prompt novo → o fecho sai antes
  do prompt; falha `sdk-cli` → nada; transcript misto → só as linhas `cli`; histórico inteiro, cauda e ao vivo
  (`FileTail` a partir do meio) dão as mesmas notas com os mesmos ids; `LineParser` sem `with_notices()` →
  nada novo (o push e a "última resposta" não mudam).

- [ ] **Step 1: RED/GREEN** nos testes de `transcript/claude.rs` (`mod tests`, :757) e um caso novo em
  `tests/it/contract_history.rs` que passa o mesmo arquivo pelo histórico e pelo `LineParser`.

Prova: `scripts/cargo-pinned test -p hangar-server --lib transcript` e
`scripts/cargo-pinned test -p hangar-server --test it contract_` passam.

### Task 2: Nova tentativa (T1, D1-A)

Status: ready-for-agent

- Cenários: 3 tentativas e resposta → nota da primeira + "voltou na 4ª"; 1 tentativa e resposta → só a nota;
  terminou em `isApiErrorMessage` → só a nota; `queued_command` intercalado não fecha; leitura começando na
  tentativa 2 → nada (abre só na 1).

Prova: os mesmos comandos da Task 1.

### Task 3: Recusa (T4)

Status: ready-for-agent

- `assistant` `cli` com `stop_reason: "refusal"` → nota; `sdk-cli` → nada.

Prova: `scripts/cargo-pinned test -p hangar-server --lib transcript`.

### Task 4: Motivo do fim do turno (T3) — só se a Task 0 confirmar

Status: needs-info (Task 0). Não há caminho do plugin até uma nota na conversa com terminal (o `AppendLocal` é
do ator sem terminal); se `reason` servir, vira plano à parte.

### Task 5: Regra e prova ao vivo

- [ ] **Step 1:** em `docs/decisoes/harnesses.md`, a regra dos avisos ganha "com terminal: só transcript ou
  plugin, nunca a tela; nota só em linha `cli`", e a entrada lista o que ficou de fora.
- [ ] **Step 2 (verificação manual):** build, troca do par (binário, `hangar-server.auth.json`,
  `auth/<sha>/libhangar_auth.so`) na pasta da versão, reinício, sessão `cx-avisos-term` com o hook que falha;
  conferir pelo `/history` e pelo fluxo ao vivo, nunca pela tela.

## Aceitação

- T1, T2, T4 com teste passando no histórico (inteiro e cauda) e ao vivo, com os mesmos ids.
- Nenhuma nota nova em linha `sdk-cli` nem nos leitores sem `with_notices()`.
- Nenhuma leitura de tela; nenhum arquivo em `backend/`, `frontend/`, `mobile/`, `desktop-native/`, `messages/`.
