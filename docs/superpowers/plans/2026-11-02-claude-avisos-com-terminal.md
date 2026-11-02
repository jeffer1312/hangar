# Claude com terminal: avisos da CLI pelo transcript

Status: needs-info — falta a decisão D1 da nova tentativa; o resto está decidido.
Base: `feat/claude-headless-notices` (`56b3332fc`), Claude Code 2.1.316. Mesma branch do sem terminal
(`2026-11-02-claude-avisos-sem-terminal.md`).

## Objetivo

Levar ao chat do Claude **com terminal** os avisos da fase 1 que o transcript grava e o leitor
`crates/hangar-server/src/transcript/claude.rs` hoje joga fora. Só por transcript ou plugin: **nunca lendo
a tela do terminal**; o que não sair por esses dois fica de fora.

## Fonte dos requisitos

Decisões do usuário em 02/11/2026 ("ok" = 1A 2A 3A 4A) e a regra "nada de ler da tela; se não der por
plugin, deixa pra lá":

| # | Aviso | Decisão |
|---|---|---|
| T1 | nova tentativa da API | nota no histórico, as tentativas do turno numa só (forma em D1) |
| T2 | hook que falha | nota igual à do sem terminal, com a soma por turno |
| T3 | motivo do fim do turno | medir o `reason` do `turn.complete` do plugin; se trouxer o motivo, nota igual à do sem terminal; senão, fica o que já existe |
| T4 | recusa do modelo | nota "o modelo recusou responder", sem o modelo de troca |
| fora | MCP/plugin que não subiu, hook lento, notificação da CLI, modelo de troca | sem transcript nem evento de plugin no 2.1.316 |

## Contratos lidos (transcripts reais das contas, 02/11/2026)

- **Nova tentativa:** `{"type":"system","subtype":"api_error","level":"error","retryAttempt":N,"maxRetries":M,
  "retryInMs":X,"error":{"status":529|null,"formatted":"529 Overloaded"|"Connection refused — …",…}}`. Uma
  linha por tentativa; cada tentativa tem `parentUuid` = `uuid` da anterior (corrente), então agrupar por
  `parentUuid` não serve: o grupo é a sequência seguida de `api_error` até a próxima linha de outro tipo.
- **Erro final da API:** linha `assistant` com `isApiErrorMessage: true`, `error` (`rate_limit`,
  `server_error`…) e o texto "API Error: …" no conteúdo. O leitor já a mostra como resposta: nada a fazer.
- **Hook que falha:** `{"type":"attachment","attachment":{"type":"hook_non_blocking_error","hookName":"PostToolUse:Bash",
  "hookEvent":"PostToolUse","stderr":"Failed with non-blocking status code: falhou-de-proposito","exitCode":1,
  "toolUseID":"…","command":"…","durationMs":3}}`. O `stderr` vem com o prefixo
  "Failed with non-blocking status code: " (tirar). Bloqueio (`hook_blocking_error`) e cancelado
  (`hook_cancelled`) ficam de fora, como no sem terminal. `stop_hook_summary.hookErrors` traz o texto de
  bloqueio do Stop, que já aparece pelo `hook_additional_context` (`claude.rs:473`): fica de fora.
- **Fim de turno:** `{"type":"system","subtype":"turn_duration","durationMs":…,"messageCount":…}`, uma linha
  por turno (5.454 nas contas). É o marco que fecha as somas.
- **Recusa:** linha `assistant` com `message.stop_reason: "refusal"`; o conteúdo costuma ser só pensamento.
- Nenhum registro traz id de turno nem `terminal_reason`.

## Arquitetura

- O leitor é por linha (`parse_obj`), mas os dois caminhos que o chamam guardam estado por arquivo: o ao
  vivo, `LineParser` (`transcript/mod.rs:71`, já com o `RewriteFilter`), e o histórico, o laço de
  `transcript/history.rs:302` (que já guarda os `held_ids`). Uma peça nova `NoticeFold` em
  `transcript/claude.rs`, com o estado dos grupos em aberto, entra nos dois e recebe cada linha antes do
  `parse_obj`, devolvendo as notas que ela gera.
- As notas saem com o formato que os clientes já desenham:
  - falha de hook: `ChatKind::Notice`, `text: "hook_failed"`, `hook_error: "{hookName}: {motivo}"` (o mesmo
    do sem terminal, `history.rs:417`);
  - nova tentativa e recusa: `ChatKind::AssistantMsg` com texto `⚙️ …`.
- Id de cada nota = derivado do `uuid` da linha que a gerou (`hookfail:{uuid}`, `retry:{uuid}`,
  `refusal:{uuid}`), estável entre recarga, histórico e busca (`search/ingest.rs` indexa por id).
- Histórico lido a partir do meio do arquivo (cauda) começa com o estado vazio: a soma conta só o que leu.
  Aceito.

## Decisão aberta

**D1. Forma da nota de nova tentativa** (terminal não tem rótulo ao vivo; o transcript grava cada tentativa
quando ela acontece):
- **A)** Nota na primeira falha ("⚙️ A API falhou (529 Overloaded); nova tentativa 1 de 10"). As seguintes só
  somam, e quando a resposta volta sai o fecho ("⚙️ A API voltou na 4ª tentativa"). Duas notas só quando
  houve mais de uma tentativa; se acabou em erro, o erro final da API já aparece. ➡️
- **B)** Só a nota da primeira falha.
- **C)** Só um resumo quando o grupo fecha.

## Tasks

### Task 0: Fixtures e medida do `turn.complete`

Status: ready-for-agent

- [ ] **Step 1: Recortar** de transcripts reais (sem `sessionId`, caminho pessoal nem texto de conversa)
  para `crates/hangar-server/tests/fixtures/claude_events/terminal_*.jsonl`: uma sequência de `api_error`
  que voltou e uma que terminou em `isApiErrorMessage`, `hook_non_blocking_error` repetido com
  `turn_duration` no fim, e uma recusa.
- [ ] **Step 2: Medir o `reason`** que o plugin manda no `turn.complete` (`plugins/hangar/hooks/state.ts:120`,
  gravado como `motivo` pela rota de estado do plugin): sessão `cx-avisos-term` com terminal na pasta da
  prova, um turno normal e um interrompido (Esc), lendo o `motivo` gravado pelo servidor, nunca a tela.

Prova: os arquivos existem e `rg -n '/home/|"sessionId"' …/terminal_*.jsonl` não acha nada; os valores de
`reason` anotados aqui. **Bloqueio de T3:** se `reason` não distinguir os motivos anormais, T3 fica com o
que já existe (o erro final da API) e a Task 4 sai do plano.

### Task 1: `NoticeFold` e falha de hook (T2)

Status: ready-for-agent · Revisão: sim — contrato entre o leitor ao vivo e o histórico (os dois têm de dar
as mesmas notas).

- Primeira `hook_non_blocking_error` de um `hookName` no turno → nota; as seguintes somam; `turn_duration`
  fecha: `hook_failed` com `"{detalhe} ({N}× neste turno)"` quando N > 1.
- Cenários: três falhas e `turn_duration` → duas notas; ao vivo (`LineParser`) e histórico dão as mesmas
  notas com os mesmos ids; `hook_blocking_error` → nada.

- [ ] **Step 1: RED/GREEN** no próprio `transcript/claude.rs` (testes `feed` existentes, ~:786) e um caso em
  `tests/it/contract_history.rs` com o mesmo arquivo pelos dois caminhos.

Prova: `scripts/cargo-pinned test -p hangar-server --lib transcript` e
`scripts/cargo-pinned test -p hangar-server --test it contract_history` passam.

### Task 2: Nova tentativa (T1, forma de D1)

Status: needs-info (D1)

- Grupo = `api_error` seguidos; fecha na primeira linha de outro tipo (ou `turn_duration`).
- Cenários conforme D1, com as duas fixtures da Task 0.

Prova: os mesmos dois comandos da Task 1.

### Task 3: Recusa (T4)

Status: ready-for-agent

- `assistant` com `stop_reason: "refusal"` → nota "⚙️ O modelo recusou responder", id `refusal:{uuid}`.
- Prova: o comando de `--lib transcript`.

### Task 4: Motivo do fim do turno (T3) — só se a Task 0 confirmar

Status: needs-info (Task 0)

O caminho do plugin até uma nota na conversa com terminal não existe (o `AppendLocal` é do ator sem
terminal). Se a Task 0 confirmar que `reason` serve, esta Task é planejada à parte, com a rota.

### Task 5: Regra e prova ao vivo

- [ ] **Step 1:** entrada em `docs/decisoes/harnesses.md` (a regra vigente dos avisos ganha "com terminal:
  só transcript ou plugin, nunca a tela") e a tabela do que ficou de fora.
- [ ] **Step 2 (verificação manual):** build, troca do par binário + `auth.json` + `libhangar_auth.so` na
  pasta da versão, reinício, sessão `cx-avisos-term` com o hook que falha; conferir pelo `/history`.

## Aceitação

- T1, T2, T4 com teste passando nos dois caminhos (ao vivo e histórico); T3 resolvido pela Task 0.
- Nenhuma leitura de tela; nenhum arquivo em `backend/`, `frontend/`, `mobile/`, `desktop-native/`, `messages/`.
