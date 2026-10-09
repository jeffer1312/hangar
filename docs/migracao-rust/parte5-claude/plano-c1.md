# Parte 5, C1: estado do Claude sem terminal direto no Rust — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** com o Rust de pé, estado, prévia, pensamento, ferramenta, pergunta e sugestão de uma
sessão Claude sem terminal saem do ator Rust direto no SSE, sem a ida e volta ao Python; e os
serviços `last_usage`, `reload_stamp` e `unknown_private` passam a rodar no Rust.

**Architecture:** reusa a peça da 5B do Codex (PR #120): o ator escreve a vista num canal em
processo (`RuntimeRegistry::live`, `runtime/gateway.rs`) e `state/runtime_feed.rs` publica no hub
os eventos coalescidos em 150 ms; o hub descarta as cópias do Python. A C1 liga o mesmo feed para
`Provider::ClaudeHeadless`, traz para o Rust o que hoje só o Python acrescenta ao estado do Claude
(sugestão do plugin, linha de status da sessão parada) e desliga as fontes do Python para essas
sessões.

**Tech Stack:** Rust (`crates/hangar-server`), Python (`backend/app`), golden gerado pelo Python.

**Spec:** `docs/migracao-rust/parte5-claude/spec.md`. A C1 mudou de escopo por decisão do dono
(08/10/2026): em vez dos quatro serviços, tira o Python do caminho do estado da sessão sem
terminal, onde a medição mostrou o custo (Python +33 ms/s contra Rust +14 ms/s com uma sessão
gerando; sai mais `state` que `preview`). `native_message` vai para a C3; `session.patch_meta`
fica no Python, porque a 5B decidiu que o Rust não grava o sidecar.

**Depende do #120 na `main`.** A execução começa trazendo a `main` (com o #120) para a branch
`hangar-server-parte5-c1`. Linhas abaixo conferidas na branch `hangar-server-parte5b-codex`
(`79a1e5e62`); cada Task reconfere as dela na base.

## Global Constraints

- **Testes:** cada Task roda só os próprios testes focados (decisão do dono na 5-0): `cd backend && uv run pytest tests/<x>.py` e `cd crates && nice -n 10 env CARGO_BUILD_JOBS=4 cargo test -p hangar-server --test <x>` (ou `--lib <mod>::`), com `CARGO_TARGET_DIR` próprio da worktree, no máximo 2 `cargo` na máquina. Nada de suíte inteira; antes do push, `scripts/verificar-local --so linux`.
- **Contrato interno:** uma subida na junção, no próximo número livre (`backend/app/rust_server.py`, `crates/hangar-server/src/lib.rs`, testes fixos `crates/hangar-server/tests/proxy.rs` e `crates/hangar-server/tests/terminal_routes.rs`).
- **Paridade:** o `state` que o cliente recebe sai igual ao do Python (golden gerado rodando o código Python); diferença deliberada é espelhada no Python e registrada no comentário do golden.
- **Dono único:** com o feed ligado, nada desses seis eventos sobe do Python para essas sessões; erro do feed vira `state` com `problema=runtime_falhou`, nunca a volta das fontes do Python.
- **Desempenho:** nada de laço novo; o feed acorda pelo canal do ator, pelo hub ou pelo empurrão de fatos; leitura de arquivo em `spawn_blocking`.
- Log e diário só com código e nome da sessão.
- Identificador novo em inglês; comentário curto em português; `git add` por caminho; commits descritivos em inglês.

## Review Focus

- **Sessão parada** (não aberta no Rust, ou que fechou): o card continua com a linha de status de modelo, esforço e contexto, como o Python mostra hoje (`_linha_parada`). Teste na Task 3.
- **Sugestão do plugin** com a sessão sem terminal: aparece e some como hoje. Teste na Task 2.
- **Convidado e Connect** (8766/8768) veem o mesmo estado da sessão sem terminal que o dono. Teste na Task 1.
- **Troca de modo** (`/modo-execucao`, com terminal ↔ sem terminal): o hub religa e troca o dono do estado sem ficar com os dois nem com nenhum. Teste na Task 1.
- **Reabertura no meio de um turno** (o conserto `2558cdb8a`): o feed mostra `working` até o `result`. Teste na Task 1.

---

### Task 1: O feed liga para o Claude sem terminal

**Files:**
- Modify: `crates/hangar-server/src/side.rs` (`Binding::state_events` ~:53, partida do feed em `ensure_monitor` ~:283, canal privado `private_events`), `backend/app/sse.py` (`_estado_do_rust` ~:726, `_fontes_do_estado` ~:1089, `_segue_transcript` ~:1104)
- Test: testes do `side.rs` (`#[cfg(test)]`), `crates/hangar-server/tests/` do canal privado, `backend/tests/test_sse_*.py` que cobrem a escolha da fonte

**Interfaces:**
- Consumes: `RuntimeFeed` e `RuntimeRegistry::live` do #120.
- Produces: `Binding::state_events()` devolve os seis `FEED_EVENTS` para `(Provider::ClaudeHeadless, _)`; `_estado_do_rust("claude-headless", name)` é verdadeiro com o modo `pending`/`rust` e `owner.rust_owns("claude", True)`.

- [ ] **Step 1: Testes** (Rust): o hub de uma ligação `ClaudeHeadless` sobe o feed, publica `state`/`preview`/`ask_question`/`pensamento`/`ferramenta` vindos do canal do ator e descarta a cópia de cada um que chegar do Python (uma linha `state_python_leak` no log); o canal privado serve a ligação `ClaudeHeadless` com os seis eventos; a troca `headless` no `info` religa e troca o dono. Feed com turno reaberto: vista com `in_progress` → `working`.
- [ ] **Step 2: Testes** (Python): para `claude-headless` com o Rust dono, `merged_events` não cria `state`, `preview`, `pensamento` nem `ferramenta`; no modo `python` cria como hoje; o convidado lê pelo `rust_state_pump`.
- [ ] **Step 3: Implementar.** A drenagem que o laço de `state` do Python disparava na borda de "entregável" (`sse.py` ~:1352-1366) deixa de existir para essas sessões; o ator já drena sozinho.
- [ ] **Step 4: Commit** — `feat(state): Claude headless live state from the runtime feed`.

### Task 2: Sugestão do plugin pelo feed

**Files:**
- Modify: `crates/hangar-server/src/state/runtime_feed.rs`, `crates/hangar-server/src/state/facts.rs` (interesse e empurrão), `crates/hangar-server/src/state/live.rs` se o laço de interesse do `Monitor` for compartilhado
- Test: testes do `runtime_feed.rs`

**Interfaces:**
- Consumes: `StateFactsClient` / `FactsStore` (`state/facts.rs`): os fatos que o Python empurra quando mudam (`state.facts`), com interesse de 30 s renovado a cada ≤25 s, como o `Monitor` faz.
- Produces: o feed do Claude emite `suggest` com `facts.suggestion` (texto ou vazio) só quando muda.

- [ ] **Step 1: Testes:** sugestão empurrada aparece como `suggest`; limpa quando o turno começa (o Python já zera em `working`); o feed do Codex não pede fatos (não tem plugin).
- [ ] **Step 2: Implementar** reusando o laço de interesse do `Monitor`, sem segundo pedido por sessão quando os dois coexistirem.
- [ ] **Step 3: Commit** — `feat(state): plugin suggestion for Claude headless from pushed facts`.

### Task 3: Linha de status da sessão parada

**Files:**
- Create: `crates/hangar-server/src/state/parked.rs`
- Modify: `crates/hangar-server/src/state/runtime_feed.rs` (ramo `Some(None)`), `backend/tests/fixtures/contract/gen_local_policy.py` (golden de `_linha_parada`)
- Test: `crates/hangar-server/tests/contract_local_policy.rs` (casos novos)

**Interfaces:**
- Produces: `pub fn parked_status(name:&str) -> Option<String>`: lê o sidecar `~/.hangar/claude-headless/<nome>.json` (só leitura) e a cauda do transcript; mesma regra de `_linha_parada` (`claude_headless/adapter.py`, procurar o nome): `🤖 <rótulo> (<esforço>)` com motor sem o prefixo da conta, e `💬` do uso da última chamada quando há `context_window`. Reusa `local_policy` (rótulo, esforço padrão, `_fmt_tok`).
- O feed põe essa linha no `state` `idle` da sessão parada do Claude; o Codex continua sem linha (como hoje).

- [ ] **Step 1: Golden** de `_linha_parada` (modelo com e sem `[1m]`, motor com conta, esforço da sessão/do settings, com e sem `context_window`, transcript sem uso).
- [ ] **Step 2: Testes Rust** contra o golden e o feed publicando a linha com a sessão parada.
- [ ] **Step 3: Implementar** (leitura em `spawn_blocking`; recalcula só quando o feed acorda com a sessão parada, não por tique).
- [ ] **Step 4: Commit** — `feat(state): parked Claude headless status line in Rust`.

### Task 4: `last_usage`, `reload_stamp` e `unknown_private` no Rust

**Files:**
- Modify: `crates/hangar-server/src/runtime/local_policy.rs`, `crates/hangar-server/src/runtime/actor.rs` (`COSMETIC_POLICIES`), `backend/app/runtime_policy.py` (saem os três ramos), `backend/tests/fixtures/contract/gen_local_policy.py`
- Test: `crates/hangar-server/tests/contract_local_policy.rs`, `backend/tests/test_runtime_policy.py`

Regras (paridade por golden):
- **`last_usage`**: `_uso_da_ultima_chamada` (cauda de 512 KiB do transcript, último `usage` de mensagem do assistente). Transcript ausente → `{"usage": null}`.
- **`reload_stamp`**: `_marca_config` (SHA1 de `.claude.json` `mcpServers` + `settings.json`, com `json.dumps(sort_keys=True)` e junção por `\0`); motivo `config` só com marca gravada no cano e diferente.
- **`unknown_private`**: anexa em `log_paths.base()/privado/{claude,codex}-headless-desconhecidos.jsonl` com os mesmos tetos (30 por chave/geração/tipo, 10 MiB) e o mesmo formato de linha; diretório `0700`. O caminho base vem do mesmo lugar que o Python usa (conferir `log_paths.base()` e passar ao Rust por ambiente, se ainda não passa).

- [ ] **Step 1: Golden** dos três, gerado chamando as funções Python.
- [ ] **Step 2: Testes Rust** contra o golden; teste Python de que os ramos saíram.
- [ ] **Step 3: Implementar** no mesmo desvio local do ator usado na 5-0.
- [ ] **Step 4: Commit** — `feat(runtime): last usage, reload stamp and unknown events in Rust`.

### Task 5: Medição e documentação

**Files:**
- Create: `scripts/medir-claude-sem-terminal.py` (o script usado em 08/10, com a correção do fim de linha `\r\n` e o caminho do `.env` por argumento)
- Modify: `CLAUDE.md` (regra do estado ao vivo: o feed também para o Claude sem terminal), `docs/decisoes/plataforma.md`, `docs/migracao-rust/README.md`, `docs/migracao-rust/parte5-claude/spec.md` (C1 com o escopo novo)

- [ ] **Step 1: Docs** — cada regra que a C1 torna falsa, corrigida.
- [ ] **Step 2: Roteiro de medição** no `docs/migracao-rust/parte5-claude/medicao-c1.md`: mesma medida de 08/10 (uma sessão gerando resposta longa, CPU do Python e do Rust, `state` por segundo contra `preview`), antes e depois, no uso real do dono.
- [ ] **Step 3: Commit** — `docs(migracao-rust): Claude headless state from the runtime feed (C1)`.

## Ordem

Tasks 1 → 2 → 3 → 4 → 5, todas depois de trazer a `main` com o #120. A Task 4 não depende das
outras e pode ir antes, se o #120 demorar.
