# Parte 2C — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. O kickoff exige revisão independente por Task além da revisão final e autoriza executar sem aprovação intermediária.

**Goal:** Observar terminais Claude/Codex sem processo tmux por leitura, preservando estado/prévia e reserva.
**Architecture:** Controle persistente compartilhado, grade alacritty por sessão, parsers/reducer Rust. Python coleta fatos existentes, continua os efeitos do SSE e retoma imediatamente quando Rust não responde.
**Tech Stack:** Python/FastAPI, Rust/Tokio/Axum, tmux, alacritty_terminal =0.26.0.
**Spec:** `docs/superpowers/specs/2026-10-03-hangar-server-parte2c-design.md`.

## Global Constraints

- Base `a80a9e45`, branch `hangar-server-parte2c`; não trocar árvore nem publicar.
- Nunca operar tmux do usuário; provas em socket privado. Nunca subir/reiniciar/parar backend nem instalar.
- Testes focados autorizados. Não executar suíte inteira.
- Windows/psmux no Python. Código e identificadores novos em inglês; comentários curtos em português.
- Sem conteúdo de tela/conversa em log. Specs/plano ignorados, fora dos commits.
- Protocolo Python/Rust sobe junto no commit de integração.
- 2B conserva cano/app-server/fila; reconciliar versão e arquivos de entrada na integração futura.

## Review Focus

- Conteúdo de captura que começa por `%begin`/`%end` não pode virar metadado do protocolo.
- Sessão recriada ou `/clear` enquanto captura aguarda não pode publicar quadro antigo.
- Controle preso/truncado/morto não pode entregar vazio como sucesso nem deixar processo órfão.
- Observador não pode mudar ambiente/tamanho/presença humana nem responder ao terminal.
- Sidecar vazio e Codex nativo saudável devem conservar sua precedência.

### Task 1: Parsers e reducer de terminal com paridade

**Files:** Create `crates/hangar-server/src/terminal_state.rs`, `crates/hangar-server/tests/contract_terminal.rs`, `backend/tests/fixtures/contract/gen_terminal.py`, `backend/tests/fixtures/contract/golden/terminal.json`. Modify `crates/hangar-server/src/lib.rs`.
**Interfaces:** `analyze(pane: &str) -> PaneAnalysis`; `reduce(pane: &str, memory: ReducerMemory, facts: ReducerFacts) -> ReducedState`. Tipos serde, com defaults. `PaneAnalysis` inclui state/label/question/options/spinner/status_line/overlay/login/limit_reset/preview/codex_menu. `ReducerMemory` conserva prev_spinner/frozen/no_spinner/held_state/held_label. Fatos: open_question, plugin_question, plugin_state, hook_state, hook_grace=8, status_line. `ReducedState` inclui análise ajustada e memória.

- [x] **Step 1: Escrever prova de paridade antes da implementação**

O gerador lê fixtures existentes (inclui todos os `pane_*.txt`, mesmo os providers fora de escopo como entrada de regressão do parser genérico), chama funções Python e grava objeto JSON. Acrescentar prosa/ferramenta/plugin/subagente/overlay/rascunho/UTF-8 sintéticos. Teste Rust completo:

```rust
#[test]
fn pane_results_match_python() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract");
    let rows: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("golden/terminal.json")).unwrap()).unwrap();
    for row in rows.as_array().unwrap() {
        assert_eq!(serde_json::to_value(hangar_server::terminal_state::analyze(row["pane"].as_str().unwrap())).unwrap(), row["expected"], "{}", row["name"]);
    }
}
```

Sequências do reducer: mesmo spinner em 4 quadros → idle; 4 ausências após working → idle; hook working até grace 8; menu vence spinner; plugin idle não vence animação; hook idle rebaixa congelado; statusline inteira vence truncada. Testes com memória fornecida são determinísticos, sem tempo real.

- [x] **Step 2: Rodar teste vermelho**
Run: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal`.
Expected: falha porque módulo/funções ainda não existem.

- [x] **Step 3: Portar regras Python completas, sem providers novos no runtime**
Copiar semântica das regex e algoritmos de `classify`, `_live_spinner`, `status_line`, `is_overlay`, `is_login`, `rate_limit_reset`, `menu_codex` e `extract_assistant_text(pane, "claude")`. Não truncar campos UTF-8 por byte; usar caracteres. Reducer aplica exatamente a ordem documentada na análise e os limites 3/4/8. Separar fatos obtidos no Python dos contadores reduzidos no Rust. Python permanece referência dos golden.

- [x] **Step 4: Conferir paridade**
Run: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal`.
Expected: todos os testes do arquivo passam.

- [x] **Step 5: Revisão independente e commit seletivo**
Revisor lê spec, Task e diff; corrigir importantes com prova focada. Stage apenas arquivos desta Task.
Commit: `feat(server): port terminal state and preview reducers`.

### Task 2: Controle tmux persistente e grade por sessão

**Files:** Create `crates/hangar-server/src/terminal_control.rs`, `crates/hangar-server/tests/terminal_control.rs`. Modify `crates/Cargo.toml`, `crates/Cargo.lock`, `crates/hangar-server/Cargo.toml`, `crates/hangar-server/src/lib.rs`, `backend/app/plugin_bridge.py`, `backend/tests/test_plugin_bridge.py`.
**Interfaces:** `TerminalPool` compartilha actor por `(name, provider, binding, target)`; `capture(request) -> Result<CaptureResult, TerminalError>`, `release(consumer)` remove referência. `CaptureRequest` contém consumer/name/provider/binding/target/lines/colors/join. `CaptureResult` contém binding/text/analysis, e instante inicial medido no Python é devolvido sem inventar relógio compartilhado. Em caso de erro não publica nem reutiliza sucesso velho.

- [x] **Step 1: Escrever testes de controle e presença**
Parser recebe bytes fragmentados, `%begin t id flags`/`%end`/`%error`; valida identidade correspondente e tamanho, preserva corpos iniciados por `%`. Fake processo prova timeout/EOF e ausência de comandos de entrada/resize. Socket privado prova mesmo pid para várias capturas, dois consumidores, alvo exato, resize externo, ANSI/join/blank/history e último release. Grade prova UTF-8 dividido, cores, cursor/alternate e ignora PtyWrite. `terminal_preso` recebe fixtures cliente humano/control/desconhecido/erro e devolve false somente quando todos os clientes são controles comprovados.

- [x] **Step 2: Rodar testes vermelhos focados**
Run: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control`; `(cd backend && uv run pytest tests/test_plugin_bridge.py -q)`.
Expected: recurso de controle/presença ainda não implementado.

- [x] **Step 3: Implementar pool, frames e checkpoints**
Executar sem shell `tmux -u -C -N attach-session -E -f read-only,ignore-size -t =name`. Testes injetam socket via construtor, nunca env global. Um actor serializa os comandos permitidos `display-message`/`capture-pane`, trata notificações separadas do corpo e alimenta alacritty somente para o pane correto. Startup/command têm timeout e limites. `kill_on_drop`, encerramento ao último consumidor e expiração conservam a vida do usuário intacta. Capturas plain/ANSI/join/histórico seguem flags atuais; checkpoint da tela nunca substitui histórico ou wrap não provados. Pool Windows devolve indisponível. Presença consulta tty/control-mode, filtra só modo 1 explícito, recusa formato desconhecido conservadoramente.

- [x] **Step 4: Conferir testes e uso real isolado**
Run: mesmos comandos focados acima.
Expected: passam; prova tmux privado não muda dimensões/env da sessão, usa mesmo pid e fecha observador. Fixture é sintética, temporários removidos.

- [x] **Step 5: Revisão independente e commit seletivo**
Commit: `feat(server): observe terminal screens through persistent tmux control`.

### Task 3: Integrar ponte interna e reserva Python

**Files:** Create `backend/app/terminal_observer.py`, `backend/tests/test_terminal_observer.py`, `crates/hangar-server/src/terminal_routes.rs`, `crates/hangar-server/tests/terminal_routes.rs`. Modify `backend/app/state.py`, `backend/app/adapters/claude.py` (provider explícito no monitor), `backend/app/preview.py`, `backend/app/adapters/codex/adapter.py` (somente lease de observação no monitor nativo), `backend/app/rust_server.py`, `backend/tests/fixtures/contract/gen_terminal.py` (desligar ponte na referência Python), `backend/tests/test_codex_adapter.py` (prova do lifecycle terminal/headless), `backend/tests/test_rust_server.py` (fake de saúde acompanha protocolo), `crates/hangar-server/src/routes.rs`, `crates/hangar-server/src/terminal_state.rs` (diagnóstico tipado da decisão plugin, sem alterar golden), `crates/hangar-server/src/lib.rs`, `docs/decisoes/plataforma.md`, `CLAUDE.md` se regra nova necessária.
**Interfaces:** Ponte Python configurada com endereço/segredo do Supervisor em memória. Listener interno do mesmo processo em `127.0.0.1:0`, independente do bind público; saúde publica `terminal_address`, Supervisor valida endereço/porta depois de confirmar protocolo. Lease por produtor StateMonitor/PreviewBroker, não aparelho. `capture` valida consumer/binding/started/text/analysis. `reduce` recebe memória + fatos e usa parser/reducer Task 1; erro chama processamento anterior. Endpoint `/__hangar_server/terminal` POST é loopback + segredo interno, `deny_unknown_fields`, enum de operações; autenticação antes de ler JSON. Nenhuma escrita tmux. Liberação não cria observer.

- [x] **Step 1: Escrever testes de integração e reserva**
Python: sem configuração/Windows → sem HTTP; erro/frame inválido → captura Python; duas fontes compartilham lease; binding trocado/clear durante leitura descarta antigo; release/close desliga; reduce preserva memória na reserva. Preservar sidecar vazio; preview pane usa resultado Rust sem duplicar markdown/full. Rust: bind público específico `127.0.0.2:0` continua com observação acessível pelo listener interno `127.0.0.1:0`; ambos fecham com stop. Endereço de saúde inválido/externo não recebe token. Segredo errado/dono remoto → 404 sem criar actor; corpo inválido → erro limitado; acquire/capture/release/reduce tipados; falha do controle → 503, sem texto vazio. Prova Python StateMonitor/SSE existente não perde drain/permissão/loop/shells.

- [x] **Step 2: Rodar testes vermelhos focados**
Run: `(cd backend && uv run pytest tests/test_terminal_observer.py -q)`; `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_routes`.
Expected: integração ainda ausente.

- [x] **Step 3: Integrar produtores e ciclo do filho**
Configurar ponte no Supervisor apenas após saúde/protocolo e endereço interno loopback confirmado; limpar após saída. `routes::serve` sobe listener interno efêmero e público juntos sob o mesmo encerramento, sem outro processo/lifespan. Router privado só observação, nunca proxy; bind público não muda. `shared_capture` usa Rust somente sob lease explicitamente Claude/Codex; mantém cache e shield atuais, com binding no descarte. StateMonitor Claude coleta fatos e delega reducer inteiro; no erro executa bloco anterior mantendo memória. Metadados/shells/perm/loop/dedupe/SSE continuam no caminho existente. Diagnóstico plugin/pane usa informação tipada do ponto anterior ao plugin no reducer, sem log de texto do pane; a API reduce existente conserva seus resultados golden. PreviewBroker Claude usa análise Rust apenas para o mesmo quadro/binding; sidecar conserva prioridade e `_gen` conserva descarte. Codex terminal mantém lease do observador durante o monitor compartilhado, adquirido após identificar sessão não-headless, sem substituir estado/prévia nativos. Lease renova mesmo com agente ocioso, solta no finally e acompanha mudança de thread; Codex headless não tenta tmux. Não tocar lista nem envio. Subir protocolos juntos de 1 para 2. Atualizar regra/evidência com limitação Windows e referência ao ensaio isolado.

- [x] **Step 4: Conferir caminhos tocados**
Run: `(cd backend && uv run pytest tests/test_terminal_observer.py tests/test_shared_capture.py tests/test_state_classifier.py tests/test_preview_dedup.py tests/test_rust_server.py tests/test_internal_api.py tests/test_plugin_bridge.py tests/test_codex_adapter.py tests/test_sse.py -q)`; `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal --test terminal_control --test terminal_routes`.
Expected: arquivos focados passam. Não rodar suite inteira.

- [x] **Step 5: Revisão independente e commit seletivo**
Commit: `feat(server): integrate terminal observation with Python fallback`.

- [x] **Step 6: Revisão final e comunicação**
Revisão de `a80a9e45..HEAD`, status real e relatório de checks. Uso real em backend vivo não autorizado: informar essa pendência, além de Windows. Manter branch/worktree, sem push. Avisar `hangar-send hangar` com commits/checks/limitações.

## Código completo da entrega

Os arquivos abaixo são a implementação completa vigente de cada unidade, com os testes vinculados nas Tasks.

- Task 1: [processamento de estado/prévia](../../../crates/hangar-server/src/terminal_state.rs), [contrato](../../../crates/hangar-server/tests/contract_terminal.rs), [gerador Python](../../../backend/tests/fixtures/contract/gen_terminal.py).
- Task 2: [controle/grade](../../../crates/hangar-server/src/terminal_control.rs), [testes](../../../crates/hangar-server/tests/terminal_control.rs), [presença](../../../backend/app/plugin_bridge.py).
- Task 3: [ponte Python](../../../backend/app/terminal_observer.py), [rotas privadas](../../../crates/hangar-server/src/terminal_routes.rs), [monitor](../../../backend/app/state.py), [prévia](../../../backend/app/preview.py), [lifecycle Codex](../../../backend/app/adapters/codex/adapter.py), [supervisor](../../../backend/app/rust_server.py), [rotas/listeners](../../../crates/hangar-server/src/routes.rs).
- [Conferência e decisões](../specs/2026-10-03-hangar-server-parte2c-verification.md).
