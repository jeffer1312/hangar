# Avisos do Claude: uma nota por hook, linha discreta e Ignorar (PR 1 de 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** os avisos do Claude deixam de floodar e de pesar como mensagem: cada hook que falha vira uma nota só com o total, todo aviso aparece como linha centralizada que expande, e "Ignorar" silencia aquele aviso na máquina.

**Architecture:** o servidor marca cada aviso com um mapa `notice_meta` no `ChatEvent` (assunto, total, chave de ignorar), sem trocar `kind` nem `text`, para o cliente antigo continuar vendo o texto de hoje. Os clientes desenham como linha qualquer evento que traga esse mapa e escondem os de chave ignorada. As chaves ignoradas moram num arquivo da máquina (por servidor), com rota do dono e evento na lista de sessões.

**Fora deste PR, registrado:** os avisos de hook do Codex (`runtime/codex.rs:1348-1377`) seguem no cartão antigo e sem a nota única; sem terminal, reiniciar o `hangar-server` zera a conta e o hook que falha ganha uma nota nova por reinício; com terminal a nota é uma por arquivo de conversa (o transcript não marca troca de processo) e o total só aparece ao recarregar.

**Tech Stack:** Rust (`hangar-api`, `hangar-transcript`, `hangar-server`, crate novo `hangar-notice-ignores`), GPUI (`desktop-native/`), Expo/React Native (`mobile/`), `packages/core`, `messages/*.json`.

**Spec:** conversa de 14/11/2026 com o Jefferson. Mock aprovado: `~/.hangar/uploads/feat-claude-notices-905ef8/mock-avisos.html` sobre `mock-base.jpg` (fora do git: é print da tela dele). Letras: **1B** linha centralizada entre fios, detalhe ao expandir; **2B** ação principal na própria linha (as ações são dos PRs 2 e 3; aqui só "Ignorar", dentro do detalhe); **3A** "Consertar…" abre sessão filha pequena (PR 2); **4A** MCP em pílula e no painel Contexto (PR 3); **6A** MCP só em sessão sem terminal; **7A** provar o login ao vivo antes do botão; **8A** reinstalar pela rota existente; **9A** três PRs.

## Global Constraints

- Nada nasce no Python: nenhuma linha nova em `backend/app/`. Se o Python recusar o campo novo do `ChatEvent`, parar e perguntar.
- Build e teste automatizado só com ordem do Jefferson, um por vez, por `scripts/cargo-pinned`. Sem push e sem PR sem ordem.
- Texto de tela vem de `messages/pt.json` e `messages/en.json`, no mesmo commit. No nativo, chave comum por `tr_web`.
- Recurso visível entra no nativo e no Expo. A web (`frontend/`) não muda.
- Identificador novo em inglês; comentário curto, sobre o porquê.
- Tela do Expo só é dada por pronta depois de vista no iPhone; o mock do celular ainda não existe (falta o print dele).
- Cliente antigo com servidor novo continua vendo o que vê hoje: `kind` e `text` dos avisos não mudam.

## Review Focus

1. Cliente antigo (PWA, Expo sem a OTA) com servidor novo: o aviso sai com o mesmo `kind` e `text` de antes; o mapa `notice_meta` é só acréscimo. Teste na Task 2.
2. `notice-ignores.json` ilegível: a rota responde erro; nunca lista vazia, que o próximo `PUT` gravaria por cima. Teste na Task 3.
3. Chave de ignorar fora do formato ou enorme no `PUT`: 422, nada gravado. Teste na Task 3.
4. A fila perdeu a linha da primeira nota (teto de linhas): a regravação com o total cria UMA linha nova, sem erro. Teste na Task 1.
5. A marca `hook_non_blocking_error` dentro do texto de um prompt ou de mensagem da fila: a contagem prévia não a conta como falha. Teste na Task 1.

---

### Task 1: Uma nota por hook com o total; "turno parado por hook" vira rótulo

Status: ready-for-agent

Código já escrito na worktree (sem commit). Falta provar, cobrir os dois casos do Review Focus e commitar.

**Files:**
- Modify: `crates/hangar-transcript/src/claude.rs` (`NoticeFold`: `hooks`, `hooks_known`, `seed_hooks`, `stamp`, `hook_failure`)
- Modify: `crates/hangar-transcript/src/history.rs` (`seed_notices`, `stamp` no fim de `parse`), `crates/hangar-transcript/src/lib.rs` (`LineParser::at_line`, campo `path`)
- Modify: `crates/hangar-server/src/runtime/claude.rs` (`hook_fails`, `stop_label`, `flush_hook_fails`, `hook_notice`), `runtime/actor.rs` (`entry_id` do `local_output`), `runtime/queue.rs` (`AppendLocal` com id existente troca o texto)
- Modify: `crates/hangar-server/src/queue_feed.rs` (a assinatura que republica a linha inclui `hook_error`; nota já velha na primeira rodada não sai de novo)
- Test: `crates/hangar-server/tests/it/runtime_claude.rs`, `crates/hangar-server/tests/it/transcript_notices.rs`, testes de `queue.rs` e de `queue_feed.rs`
- Docs: `docs/decisoes/harnesses.md` (já editado)

**Interfaces:**
- Produces: nota de hook com id fixo por hook; sem terminal `payload["entry_id"]` no `local_output`; com terminal `hookfail:{uuid da primeira}`. Detalhe `"{hook}: {motivo}"` e, com repetição, sufixo `" (falhou N vezes)"`. Rótulo de estado `"Um hook parou o turno"` por 8 s (`STOP_LABEL`, `STOP_LABEL_S`).

- [x] **Step 1: Rodar os testes das notas**

Run: `scripts/cargo-pinned test -p hangar-server --test it notice`
Expected: PASS em `runtime_claude::notice_*` e `transcript_notices::*`. Falha de compilação ou de asserção: corrigir e repetir só o que falhou.

- [x] **Step 2: Teste da linha perdida da fila**

Em `crates/hangar-server/src/runtime/queue.rs` (módulo `headless_delivery_tests`), `hook_note_with_known_id_rewrites_and_with_lost_id_appends`: `AppendLocal` com `entry_id: Some("hookfail:1:0")` duas vezes deixa UMA linha com o segundo texto e o `ts` da primeira; id que pertence a uma mensagem da pessoa é recusado. O id que a fila não tem é o próprio primeiro `AppendLocal` (mesmo caminho da linha perdida).

Em `crates/hangar-server/src/queue_feed.rs`, `rewritten_hook_note_is_published_again_unless_it_was_already_old`: a linha regravada sai de novo como `message` com o mesmo id; para um seguidor que nasceu 900 s depois da nota, nem a nota nem a regravação saem.

- [x] **Step 3: Teste da marca dentro de texto**

Em `transcript_notices.rs`, `terminal_marker_inside_a_prompt_is_not_a_failure`: arquivo com um prompt cujo texto contém `hook_non_blocking_error`, um `queued_command` com a mesma palavra no `prompt`, e uma falha real `h1` depois; histórico com janela que começa depois do prompt devolve `vec![h1(1)]`.

- [x] **Step 4: Rodar de novo o filtro `notice` e o teste da fila**

Run: `scripts/cargo-pinned test -p hangar-server --test it notice` e `scripts/cargo-pinned test -p hangar-server --lib hook_note` (o filtro pega o teste da fila e o do seguidor)
Expected: PASS.

- [x] **Step 5: Commit**

`git add` dos arquivos acima por caminho. Mensagem: `fix(claude): one hook-failure note per process with a running total`.

### Task 2: Mapa `notice_meta` no evento, em todo aviso do Claude

Status: ready-for-agent

**Files:**
- Modify: `crates/hangar-api/src/chat.rs:83-116` (campo novo em `ChatEvent`, antes de `commit_blocks`; `hangar-api/tests/contract.rs:34-48` compara com as amostras do Python, por isso o campo ausente não pode sair como `null`)
- Modify: `crates/hangar-server/src/runtime/claude.rs` (emissões de `local_output` de aviso: `:1066` pedido da CLI, `:1091` cancelamento, `:1131` Turno parado, `:1213` Evento desconhecido, `:1392` recusa, `:1464` `hook_notice`, `:1470` `notice_once`, chamado em `:1372` notification e `:1500` MCP/plugin; NÃO a `:1017`, saída de comando com `source`)
- Modify: `crates/hangar-server/src/runtime/queue.rs` (`Action::AppendLocal` ganha `meta`), `runtime/actor.rs:1212-1215`, `crates/hangar-transcript/src/history.rs:524-545` (`entry_event`)
- Modify: `crates/hangar-transcript/src/claude.rs` (`note`, `hook_note`, `stamp`)
- Modify: `packages/core/src/types.ts:211` (`ChatEvent`)
- Test: `crates/hangar-server/tests/it/runtime_claude.rs`, `transcript_notices.rs`, testes de `history.rs`

**Interfaces:**
- Produces, Rust: `ChatEvent.notice_meta: Option<serde_json::Map<String, Value>>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`. O mapa presente (mesmo vazio) é o que faz do evento um aviso. Chaves: `key: String` (só no aviso que pode ser ignorado), `subject: String` (hook: o `hook_name`), `count: u64` (só quando > 1).
- Produces, TS: `notice_meta?: { key?: string; subject?: string; count?: number }`.
- Chave de ignorar (`key`). Aviso de bloqueio real ou de defeito do próprio Hangar não tem chave e não pode ser ignorado (regra "Aviso na tela só para ação que falhou ou bloqueio real"):

| Aviso | `key` |
|---|---|
| Hook que falha | `hook:{hook_name}` |
| MCP ou plugin que não subiu | `note:startup:{md5_8 dos nomes ordenados}` (servidor novo com problema muda a chave e o aviso volta) |
| Nova tentativa da API (com terminal) | `note:retry` |
| `system/notification` | `note:notification:{key do evento}` |
| Recusa do modelo, Turno parado (motivo anormal), cancelamento, evento desconhecido, pedido da CLI respondido vazio | sem `key` |

- Fila: `Action::AppendLocal { …, meta: Option<Value> }` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), gravado na linha como `notice_meta`. A regravação por id troca `text`, `hook_error` e `notice_meta` só quando o `count` novo é maior que o gravado: cada nota é gravada por uma tarefa própria e duas regravações seguidas podem chegar fora de ordem. O ator repassa `payload["meta"]`.
- O seguidor da fila (`queue_feed.rs`, `Signature`) republica a linha quando o `notice_meta` muda, como já faz com `hook_error`.
- O `kind` e o `text` de cada aviso ficam como estão hoje.

- [x] **Step 1: Testes que falham**

`runtime_claude.rs`:
- `notice_meta_on_hook_failure`: primeira falha de `PostToolUse:Bash` → `payload["meta"] == {"key":"hook:PostToolUse:Bash","subject":"PostToolUse:Bash"}`; no fim do turno com 4 falhas → `meta["count"] == 4`, mesmo `entry_id`.
- `notice_meta_on_cli_notes`: nota de MCP na abertura → `meta["key"]` começa com `"note:startup:"`, e outro conjunto de servidores dá outra chave; `notification` com `key: "update"` → `"note:notification:update"`; `terminal_reason: "blocking_limit"` e recusa do modelo → `meta == {}` (aviso sem chave).
- `local_command_output_has_no_notice_meta`: saída de `/cost` (`local_output` com `source`) → `payload["meta"]` nulo.

`queue.rs` (`headless_delivery_tests`):
- `rewrite_with_a_smaller_count_is_dropped`: linha com `notice_meta.count == 5`; `AppendLocal` do mesmo id com `count == 3` não muda a linha; com `count == 6` muda.

`queue_feed.rs`:
- em `rewritten_hook_note_is_published_again_unless_it_was_already_old`: linha com o mesmo `hook_error` e `notice_meta.count` maior também sai de novo.

`history.rs` (testes do arquivo):
- `local_note_row_keeps_kind_and_text_and_carries_the_map`: linha `{"id":"a1","papel":"assistant","text":"⚙️ x","notice_meta":{"key":"note:retry"}}` → `kind == AssistantMsg`, `text == "⚙️ x"`, `notice_meta.key == "note:retry"`; a mesma linha sem `notice_meta` serializa sem o campo.

`transcript_notices.rs`:
- em `terminal_hook_failure_is_one_note_per_file_with_the_total_in_history`: o evento do histórico tem `notice_meta == {"key":"hook:PostToolUse:Bash","subject":"PostToolUse:Bash","count":5}`; o ao vivo, sem `count`.
- `terminal_retry_and_refusal_carry_the_map`: `retry:`/`retry-ok:` → `{"key":"note:retry"}`; `refusal:` → `{}`.

- [x] **Step 2: Rodar e ver falhar** (pulado por decisão do Jefferson em 14/11: uma rodada por tarefa, fila de compilação cheia)

Run: `scripts/cargo-pinned test -p hangar-server --test it notice`
Expected: FAIL nas asserções novas (campo ausente).

- [x] **Step 3: Implementar**

Em `claude.rs` do servidor, `fn cli_note(&mut self, ignore_key: Option<&str>, text: String, effects: &mut Vec<Effect>)` emite `local_output` com `meta` (`{}` ou `{"key": …}`); `notice_once(key, ignore_key, text, effects)` mantém o `key` de hoje como chave de deduplicação e ganha a de ignorar. `hook_notice` manda `meta` com `key`, `subject` e `count` (quando > 1). Em `hangar-transcript`, `note(id, ignore_key: Option<&str>, text, at)` e `hook_note` preenchem `notice_meta`; `stamp` grava também `count`. O espelho Python não muda: `backend/app/models.py:222` não tem `extra="forbid"` e nada no Python valida evento vindo do Rust (conferido na revisão do plano).

- [x] **Step 4: Rodar e ver passar; commit**

Run: `scripts/cargo-pinned test -p hangar-server --test it notice` e `scripts/cargo-pinned test -p hangar-transcript local_note_row`
Expected: PASS. Commit: `feat(claude): tag every CLI notice with a notice map`.

### Task 3: Avisos ignorados da máquina

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-notice-ignores/Cargo.toml`, `crates/hangar-notice-ignores/src/lib.rs`
- Modify: `crates/Cargo.toml` (membro do workspace), `crates/hangar-server/Cargo.toml`
- Create: `crates/hangar-server/src/notice_ignores.rs` (rota e fonte do evento)
- Modify: `crates/hangar-server/src/routes.rs` (ao lado de `/api/tags`, `:904`), `crates/hangar-server/src/list/hub.rs` (evento, como `tags` em `:528-538`)
- Test: testes do crate; `crates/hangar-server/tests/it/` (rota)

**Interfaces:**
- Crate: `pub fn load(home: &Path) -> io::Result<BTreeSet<String>>` (arquivo ausente = vazio; ilegível = `Err`), `pub fn set(home: &Path, key: &str, ignored: bool) -> Result<BTreeSet<String>, Error>`, `pub fn valid_key(key: &str) -> bool` (prefixo `hook:` ou `note:`, 1 a 200 caracteres, sem caractere de controle), `pub const MAX_KEYS: usize = 200`. Arquivo `~/.hangar/notice-ignores.json`, `{"keys":[…]}`, gravado por tmp + rename.
- Rota, só o dono: `GET /api/notice-ignores` → `{"keys":[…]}`; `PUT /api/notice-ignores` com `{"key":"…","ignored":true|false}` → a lista nova. Chave inválida ou corpo fora de forma: 422 `erro_corpo_invalido`. Lista cheia: 409 `notice_ignores_full`. Arquivo ilegível: 500 `notice_ignores_unreadable`.
- Evento `notice_ignores` na lista do dono (`/api/sessions/events`), com `{"keys":[…]}`, emitido na abertura e a cada mudança. O arquivo tem um escritor só, a rota: ela avisa o hub por um canal `tokio::sync::watch` com a lista nova. Sem vigia de arquivo (o `tags.rs` vigia pastas de conta; aqui não há o que vigiar). O quadro sai como o de `tags` (`list/hub.rs:522-540`, `publish_tags`).
- A lista é do servidor: cliente ligado a várias máquinas guarda um conjunto por servidor. Convidado não recebe a lista e vê todos os avisos.

- [x] **Step 1: Testes que falham**

No crate: `missing_file_is_empty`, `unreadable_file_is_an_error_and_set_does_not_overwrite_it`, `set_adds_and_removes`, `invalid_key_is_refused` (`""`, `"x:y"`, 201 caracteres, `"hook:a\nb"`), `full_list_refuses_the_201st`.
No servidor: `notice_ignores_put_then_get_and_event` (PUT de `hook:Stop` → GET traz a chave → o SSE da lista publica `notice_ignores`), `notice_ignores_refuses_guest_token`.

- [x] **Step 2: Rodar e ver falhar** (pulado: uma rodada por tarefa)

Run: `scripts/cargo-pinned test -p hangar-notice-ignores` e `scripts/cargo-pinned test -p hangar-server --test it notice_ignores`
Expected: FAIL (crate e rota não existem: criar o esqueleto até falhar na asserção).

- [x] **Step 3: Implementar o crate, a rota e o evento**

Crate no molde do `crates/hangar-update-news` (pequeno, sobre o `hangar-core`); o servidor só liga a rota e o evento. Da `tags.rs` aproveitar só o porteiro do dono e os códigos 422 e 409.

- [x] **Step 4: Rodar e ver passar; commit**

Commit: `feat(server): per-machine ignored notices`.

### Task 4: Nativo: linha centralizada, Ignorar e rótulo de turno parado

Status: ready-for-agent

**Files:**
- Create: `desktop-native/src/app/notice_line.rs` (desenho da linha e do detalhe)
- Modify: `desktop-native/src/app.rs` (`render_message` `:5425`, `display_body` `:6781-6790`, `render_working` `:4122`, `turn_done_text` `:6846`; evento da lista ao lado do de `tags`, `:2072`), `desktop-native/src/app/servers.rs:180` (o mesmo evento das máquinas remotas)
- Create: `desktop-native/src/app/notice_ignores.rs` (leitura, `PUT` e conjunto por servidor, no molde de `app/tags.rs:136` e `:178`, que usam `api.server_read` e `api.server_send`)
- Modify: `messages/pt.json`, `messages/en.json`
- Test: testes de unidade do módulo (texto da linha, filtro por chave)

**Interfaces:**
- Consumes: `ChatEvent.notice_meta` (Task 2); rota e evento `notice_ignores` (Task 3).
- Produces: `fn notice_title(event: &ChatEvent) -> String` e `fn notice_hidden(event: &ChatEvent, ignored: &BTreeSet<String>) -> bool`, usados também pela Task 6.
- Chaves novas (pt / en): `notice_hook_failed_line` "Hook {name} falhou" / "Hook {name} failed"; `notice_count` "{count} vezes" / "{count} times"; `notice_ignore` "Ignorar" / "Ignore"; `notice_ignored_line` "Aviso ignorado nesta máquina" / "Notice ignored on this machine"; desfazer usa `comum_desfazer`.

Desenho (medidas do mock, em px da tela dele):
- Evento com `notice_meta` vira uma linha de 27 de altura, centralizada entre dois fios de 1 px (cor da borda do tema), em vez do rótulo "Aviso" + parágrafo + bloco de código.
- Conteúdo: ícone de 14 (triângulo de aviso, cor `warning`), título 12,5 `muted`, `· {count} vezes` mais apagado quando houver, chevron do `disclosure` (`app.rs:4034`).
- Título: hook → `notice_hook_failed_line` com `notice_meta.subject`; os outros → primeira linha do `text` sem o `⚙️ ` inicial, cortada com reticências.
- Aberto (estado em `expanded`, chave `notice:{event.id}`): bloco de código com `hook_error` ou o `text` inteiro, e o botão fantasma "Ignorar", só no aviso que tem `notice_meta.key`. Nada de fundo opaco: superfície transparente sobre o vidro.
- "Ignorar": `PUT` com `ignored: true`; a linha vira "Aviso ignorado nesta máquina · Desfazer" até a conversa ser recarregada; "Desfazer" manda `ignored: false`. Erro do `PUT` aparece na própria linha.
- Evento de chave ignorada não é desenhado, em nenhuma sessão daquele servidor, assim que o evento `notice_ignores` chega.
- O aviso de hook do Codex (`hook_failed`/`hook_prompt` sem `notice_meta`) segue no desenho de hoje.
- Linha de fim de turno: com a sessão parada e `state.label` não vazio, `✻ Trabalhou por 48s · {label}`, o trecho do rótulo em `warning_text`. Some quando o servidor tira o rótulo.

- [ ] **Step 1: Testes que falham**

`notice_title_for_hook_uses_subject`, `notice_title_for_cli_note_strips_the_gear_and_keeps_the_first_line`, `notice_hidden_only_for_ignored_key`, `notice_without_key_is_never_hidden_and_has_no_ignore`, `event_without_notice_meta_is_not_a_notice_line`.

- [ ] **Step 2: Rodar e ver falhar**

Run: `scripts/cargo-pinned test --locked --manifest-path ../desktop-native/Cargo.toml notice_`
Expected: FAIL.

- [ ] **Step 3: Implementar módulo, ligação em `render_message`, cliente da rota e rótulo em `render_working`**

- [ ] **Step 4: Rodar e ver passar**

- [ ] **Step 5: Verificação manual no app**

Janela de pré-visualização (sem trocar o app instalado dele) numa sessão `cx-*` sem terminal com um hook que falha de propósito: print da linha fechada, aberta, ignorada e do rótulo de turno parado. Comparar cada print com o mock no mesmo estado e listar o que difere.

- [ ] **Step 6: Commit**

`feat(native): Claude notices as a discreet expandable line with Ignore`.

### Task 5: Expo: linha que expande, Ignorar e rótulo de turno parado

Status: needs-info (falta o print do chat no celular para o mock)

**Files:**
- Create: `mobile/src/chat/NoticeRow.tsx` (o nome `NoticeLine` já existe, exportado em `MessageList.tsx:409` e usado em `CompactIdleAnimation.tsx:6`; ele fica como está)
- Modify: `mobile/src/chat/MessageList.tsx:215-227` e `:409-438`, `mobile/src/chat/LiveWork.tsx`, `packages/core/src/` (`noticeLine.ts`: título e filtro, com os mesmos casos da Task 4), `packages/core/src/api.ts` (rota, ao lado de `/api/tags` em `:1246-1251`), `mobile/src/stores/sessions.ts:582` (evento, ao lado de `tags`; um conjunto por servidor)
- Test: `packages/core/src/noticeLine.test.ts`, `mobile/src/chat/MessageList.test.tsx`

**Interfaces:**
- Consumes: `ChatEvent.notice_meta`, rota e evento `notice_ignores`, chaves da Task 4.
- Produces: `noticeTitle(ev: ChatEvent): string | null`, `noticeHidden(ev: ChatEvent, ignored: ReadonlySet<string>): boolean` em `packages/core`.

Desenho: a linha centralizada de hoje (13 pt, apagada) ganha os fios, o ícone e o chevron; toque abre o detalhe (padrão do `ThinkingBlock`), com alvo de 44 pt e `accessibilityState.expanded`. "Ignorar" no detalhe, vibração por `ui/haptics.ts`. O rótulo de turno parado entra numa linha de rodapé no lugar da `WorkingLine` enquanto `state.label` vier com a sessão parada. O lugar exato sai do mock sobre o print do celular.

- [ ] **Step 1: Mock do celular sobre o print dele, aprovado por letra**
- [ ] **Step 2: Testes que falham** (`noticeTitle`, `noticeHidden`, `MessageList` desenha a linha para evento com `notice_meta`, esconde o ignorado e não mostra "Ignorar" no aviso sem `key`)
- [ ] **Step 3: Rodar e ver falhar** — pelo script `test` de cada `package.json` (o do mobile compila o i18n antes, e chave nova exige isso), filtrando o arquivo, sob a trava da máquina: `flock -w 900 /tmp/hangar-verificacao.lock npm --prefix mobile test -- src/chat/MessageList.test.tsx` e o mesmo com `--prefix packages/core` e `src/noticeLine.test.ts`
- [ ] **Step 4: Implementar**
- [ ] **Step 5: Rodar e ver passar**
- [ ] **Step 6: Verificação manual no iPhone** (linha fechada, aberta, ignorada, rótulo; tema claro e escuro)
- [ ] **Step 7: Commit** — `feat(mobile): Claude notices as an expandable line with Ignore`

### Task 6: Avisos ignorados em Configurações

Status: ready-for-agent

**Files:**
- Modify: `desktop-native/src/app/settings/appearance_page.rs` (grupo `settings_conversation_group`: corpo em `conversation_body`, `:597`)
- Modify: `mobile/app/config/geral.tsx`
- Modify: `messages/pt.json`, `messages/en.json`

**Interfaces:**
- Consumes: lista de chaves (Task 3), `PUT` com `ignored: false`.
- Chaves novas: `notice_ignored_title` "Avisos ignorados" / "Ignored notices"; `notice_ignored_empty` "Nenhum aviso ignorado" / "No ignored notices"; `notice_show_again` "Voltar a mostrar" / "Show again"; rótulo de cada chave por `notice_key_hook` "Hook {name}" / "Hook {name}", `notice_key_startup` "MCP ou plugin que não subiu" / "MCP or plugin that did not start", `notice_key_retry` "Nova tentativa da API" / "API retry", `notice_key_notification` "Aviso da CLI: {name}" / "CLI notice: {name}".

Uma linha de ajuste "Avisos ignorados" com o total; abre a lista (os quatro estados: carregando, vazio, erro, lista), cada item com "Voltar a mostrar".

- [ ] **Step 1: Teste do rótulo da chave** (`notice_key_label("hook:Stop") == "Hook Stop"`, chave desconhecida mostra a própria chave)
- [ ] **Step 2: Implementar nos dois clientes**
- [ ] **Step 3: Verificação manual** (ignorar na conversa, voltar a mostrar em Configurações, aviso reaparece)
- [ ] **Step 4: Commit** — `feat: list of ignored notices in settings`

### Task 7: Novidade, decisões e verificação do PR

Status: ready-for-agent

- [ ] **Step 1: `docs/decisoes/harnesses.md`** — regra do mapa `notice_meta` e das chaves de ignorar em "Regras vigentes"; no `CLAUDE.md`, uma linha em Plataforma para os avisos ignorados (arquivo, rota, evento).
- [ ] **Step 2: Novidade** pela skill `hangar-novidade` (mudança visível: linha de aviso e Ignorar), com a animação e o print.
- [ ] **Step 3: `scripts/verificar-local`** em primeiro plano, com ordem do Jefferson.
- [ ] **Step 4: Perguntar PR × main e abrir o PR** só com a ordem dele.
