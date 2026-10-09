# Parte 5 (Codex): spec

**Estado:** aprovada pelo dono em 07/10/2026. Cada subparte ganha plano próprio antes do código.
**Análise:** [`analise.md`](analise.md). **Base do plano:** `main` + `fix/codex-hardening`
(Stop encerra os comandos do turno, `codexErrorInfo`, pensamento ao vivo, `codex_turno_cortado`,
kill com identidade); a spec conta com isso e não refaz.

## A regra, em uma frase

Com o Rust de pé, **todo o Codex é do Rust**: nasce, roda, recebe mensagens, publica estado,
mexe em contas, configuração e integração, e o lançador do pane não importa Python. O Python do
Codex fica só como a reserva do processo inteiro (modo `python`: Rust ausente, incompatível ou
caído) e sai na parte 7, como manda o dono único.

## Fora desta parte

- Pi, omp, Kimi e orq.
- Módulos compartilhados que saem inteiros na parte 6: `archive.py`/`archive_providers.py`,
  `bastao.py`, `config_sync.py` (menos o trecho do Codex, ver 5F), `credenciais.py` (menos as
  rotas do Codex, ver #115). O ramo do Codex neles muda só para chamar o Rust quando a escrita
  passar a ser dele.
- Desktop web e Electron (parados). Tela nova vai para PWA/mobile (`frontend/` e `mobile/`) e
  para o nativo (`desktop-native/`).
- Fixar a versão do Codex como o T3 faz: o Hangar continua usando o Codex instalado.

## Tipos do protocolo

Medição e alternativas em [`analise.md`](analise.md#tipos-do-protocolo). Decisão proposta:

- **Crate novo `crates/hangar-codex`**, sem axum, usado pelo `hangar-server` e pelo lançador.
  Módulo `proto` com structs escritas à mão só do que o Hangar usa:
  - `ClientRequest`, `ServerRequest` e `ServerNotification` como `#[serde(tag = "method")]`, cada
    uma com variante `Unknown { method, params: Box<RawValue> }` para método novo (registra uma
    vez por método e por servidor, sem texto de conversa).
  - Parâmetros e resultados dos ~25 métodos usados (lista no plano), todo campo opcional com
    `#[serde(default)]`, nunca `deny_unknown_fields`, `ReasoningEffort` como texto livre.
  - Os três nomes do esforço (`effort` nos pedidos e em `ThreadSettings`, `reasoningEffort` nas
    respostas, `reasoning_effort` no `collaborationMode`) ficam em campos separados com o nome
    exato do schema. Nenhum `alias` cruzado: o rename tem de quebrar o teste, não passar calado.
- **Recorte do schema da versão conferida** em `crates/hangar-codex/schema/<versão>/` (só as
  definições usadas, gerado por script a partir de `generate-json-schema --experimental`; o
  arquivo inteiro tem 736 KB). Teste no CI: cada campo que as structs leem ou escrevem existe no
  recorte, com o mesmo nome.
- **`scripts/conferir-codex-schema`**: gera o schema do Codex instalado, refaz o recorte e mostra o
  diff dos campos usados. É o passo obrigatório ao subir a versão conferida.
- **Aviso de versão:** a resposta do `initialize` traz `userAgent` (`…/0.159.3 (…)`). Major.minor
  diferente da conferida → no máximo uma linha no diário por minuto por versão e um aviso discreto na sessão
  (`codex_versao_nao_conferida`, com as duas versões). A decodificação continua tolerante.
- **Versão conferida:** a mais nova instalada nas duas máquinas quando o plano for escrito
  (hoje 0.159.3 aqui e 0.160.1 no notebook).
- `runtime/codex.rs` passa a usar os tipos em 5A. O `hangar-cano` continua opaco: repassa linhas e
  só lê o mínimo para o snapshot (`id`, `method`, `threadId`, delta), sem depender do crate.

## Subpartes, ordem e dependências

```
5A tipos + cliente ──┬── 5B sem terminal ──┬── 5C com terminal ──┐
                     │                     ├── 5D fork/revert/goal/terminais
                     │                     └── 5H voz            ├── 5I lançadores
                     └── 5E catálogo de modelos ──┬── 5F integração nativa ┘
                                                   └── 5G transferência (precisa de 5B)
#115 contas/login/cotas ───────────────────────────► 5F/5G e consumidores de sessões
```

A 5B depende também da **5-0** (caminho de escrita comum aos dois provedores: tabela de despacho
rota × provedor × modo, rotas genéricas reivindicadas no Rust, `runtime_coordinator` perguntando
à tabela, `format_status` e `skill_catalog` no Rust), que a metade Claude escreve e revisa no próprio fluxo; esta só usa o resultado.
Divisão e regras dos arquivos comuns em [`../parte5-claude/contrato-par.md`](../parte5-claude/contrato-par.md).
5B e 5E correm em paralelo depois de 5A. A gestão de contas Claude/Codex foi separada na
[issue #115](https://github.com/jeffer1312/hangar/issues/115); a 5E mantém o catálogo de modelos.
Cada subparte tem plano próprio, execução com revisor
por Task, revisão final, PR em rascunho e uso real pelo canal de testes antes da seguinte que
depende dela. Mudou rota `/internal`, evento do `side-events` ou variável do filho → o próximo
número livre do contrato interno (`RUST_SERVER_PROTOCOL`/`INTERNAL_PROTOCOL`) na junção.

### 5A. Tipos e cliente (sem mudança de dono)

- `hangar-codex::proto` e recorte do schema, como acima.
- `hangar-codex::client`: JSON-RPC sobre qualquer `AsyncRead + AsyncWrite` (ids, futuros com
  prazo, pedidos do servidor, canal limitado de notificações). Três transportes: cano (socket com
  token e quadros `cano_*`), WebSocket (`tokio-tungstenite`, já no lockfile pelo axum) e stdio
  efêmero (`codex app-server -c features.plugins=false`, para cota/catálogo/admin).
- `runtime/codex.rs` reescrito sobre os tipos, com os mesmos 28 testes passando e o golden
  `codex-golden.json` igual.
- Junta `fix/codex-hardening` antes (ou espera o PR dela na `main`).

Pronta quando: testes do motor e golden iguais; teste do recorte verde; aviso de versão aparece
com um `userAgent` falso de outra versão.

### 5B. Codex sem terminal no Rust (o dono)

- **Nascimento e vida no Rust.** Sai a trava de `_born_in_rust` (`runtime_coordinator.py:515`) e
  os "o Codex é sempre do Python" (:635, :679, :1026, :1249). O Rust sobe o cano do Codex
  (argv/env de `sem_terminal.py`, escopo do systemd, `CODEX_HOME` da conta, sem `TMUX`), conecta,
  religa pelo snapshot, mata o grupo com conferência de identidade (`taskkill /T` no Windows),
  limpa órfãos e grava o sidecar `~/.hangar/codex-sessions/<nome>.json` com a mesma trava. A rota
  de criação continua no Python nesta subparte e chama o Rust para abrir, como no Claude.
- **Religar sem varrer.** O `watch_sessions` de 2 s vira evento: saída do cano (`cano_saiu`) e
  mudança no diretório de sidecars (`notify`), com teto de uma tentativa por sessão a cada 2 s e o
  teto de 3 subidas seguidas (`TETO_SUBIDAS`).
- **Controles no Rust:** `restart`, troca de modo de permissão (mesmo sandbox só grava o sidecar;
  outro sandbox reabre o servidor ocioso), modelo, esforço, Fast, modo de colaboração, `/compact`,
  `steer`, `interrupt` + `thread/backgroundTerminals/terminate`. Saem os `lifecycle_required` do
  motor (`codex.rs:559-561`).
- **Linha de status e catálogo de skills no Rust.** Saem as chamadas `format_status` e
  `skill_catalog` da política (`runtime_policy.py:171-208`); a linha é calculada só quando as
  entradas mudam.
- **Estado, prévia e `ask_question` publicados pelo Rust** (fim do "o Python segue dono" de
  `side.rs:1048`), com o coalescimento de 150 ms da regra vigente. A lista lê o estado do Rust.
- **Pedidos do servidor:**

  | Pedido | Resposta |
  |---|---|
  | `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` | Cartão Permitir / Negar / Sempre permitir (como hoje) |
  | `item/permissions/requestApproval` | Cartão com os caminhos (leitura/escrita) e rede pedidos: Permitir neste turno (`scope: turn`) / Permitir na sessão (`scope: session`) / Negar (perfil vazio; o plano confere contra o Codex real que isso nega) |
  | `item/tool/requestUserInput` | Pergunta nativa (como hoje) |
  | `mcpServer/elicitation/request`, modo formulário | Pergunta nativa: `enum`/booleano viram opções, texto/número viram campo livre; resposta `accept` com `content`. Esquema que não cabe (objeto aninhado, multisseleção) → `decline` + nota no chat |
  | `mcpServer/elicitation/request`, modo URL | Cartão com o link e Concluí / Cancelar → `accept` / `cancel` |
  | `currentTime/read` | Hora do sistema |
  | `item/tool/call`, `account/chatgptAuthTokens/refresh`, `attestation/generate`, legados v1, desconhecido | `-32601` + nota no chat (regra vigente) |

  Pedido de outra thread (subagente) entra no mesmo ramo: o filtro de thread só vale para
  notificação, nunca descarta pedido.
- **Rotas do Codex sem terminal no Rust:** as genéricas reivindicadas pela 5-0 (`/input`,
  `/steer`, `/interrupt`, `/select`, `/answer`, `DELETE …/queue/{id}`) viram `Rust` na tabela de
  despacho para Codex sem terminal; `/rename`, `DELETE` e `/recarregar` com Codex, as só do Codex
  (`/question/skip`, `/models`, `/model`, `/service-tier`, `/codex/mode`, `/limits`) e
  `/commands` entram aqui. Com terminal tudo continua repassado ao Python até 5C.

Pronta quando: no uso real, criar, conversar, aprovar cada tipo de pedido, interromper com
comando longo rodando, reiniciar o backend no meio de um turno, trocar modo de permissão, Fast e
modelo, e matar o cano à mão; tudo com o Rust de dono e `CP_RUST_SERVER=0` ainda abrindo pelo
Python.

### 5C. Codex com terminal no Rust

- O Rust conecta no app-server do pane pelo WebSocket do sidecar (`endpoint`), confere o
  `app_pid`, manda `initialize` e assina com `thread/resume` em espera crescente (1 s ×1,5 até
  10 s, como hoje) até o rollout existir. Um cliente por sessão.
- Envio, fila, confirmação pelo rollout (`runtime/receipt.rs` já lê o `session_meta`), `steer`, `interrupt`,
  perguntas e estado pelo mesmo motor de 5B, com o transporte trocado. Troca de thread feita na
  TUI (`thread/started` com outro id, `primary_thread`) atualiza o sidecar.
- **Teclado só onde o protocolo não alcança:** seletor `/permissions` (port de
  `codex_permissions.py`), "implementar plano" e menu antes da thread, pelo escritor de terminal
  do Rust (parte 2D/4) e pela captura do `Monitor` (`terminal_state.rs:297`). No Windows, sem
  tmux `-C`, o escritor usa o comando avulso do psmux, como a parte 4 faz.
- **Pane que some:** evento do `Monitor`, não laço de 1 s; mata o app-server pelo pid com
  conferência de identidade, apaga o sidecar, limpa a fila.
- **Troca terminal ⇄ sem terminal** (`/modo-execucao`) no Rust, com as mesmas condições (ocioso,
  sem pedido pendente, fila vazia) e o mesmo desfazer.
- Rotas de 5B passam a atender também a sessão com terminal; mais `/codex-permissions` e
  `/codex/plan/implement`.

Pronta quando: no uso real, conversar pelo celular com a TUI aberta no PC, mandar enquanto a TUI
trabalha, trocar permissão pelo app, fechar o pane, trocar de modo nos dois sentidos.

### 5D. Fork, revert, objetivo e terminais em segundo plano

Depende de 5B (e de 5C para sessão com terminal). Tela no PWA/mobile e no nativo.

- **Terminais em segundo plano:** lista por `thread/backgroundTerminals/list` na sessão (comando,
  há quanto tempo), botão encerrar um (`terminate`) e limpar (`clean`). Lido ao abrir a lista e
  em `turn/completed`, nunca em laço.
- **Revert:** "voltar até aqui" numa mensagem do usuário → `thread/revert`; o chat recarrega pela
  notificação `thread/reverted`.
- **Fork:** "continuar em outra sessão a partir daqui" → `thread/fork`, sessão nova com sidecar
  próprio, mesma conta e modo.
- **Objetivo:** `thread/goal/set|get|clear` e notificações `goal/updated|cleared` numa faixa da
  sessão.

Pronta quando: cada ação usada uma vez no celular e no nativo com uma sessão real.

### 5E. Catálogo de modelos; contas na #115

- A 5E mantém `hangar-codex::models`: catálogo `/codex/models?client_version=`, reserva
  `model/list`, validação da escolha e `/api/model-options?provider=codex`.
- A #115 assume catálogo de contas Claude/Codex, ambiente e identidade pública, cadastro,
  exclusão, login, logout, renovação Claude, cotas, cache e redefinição Codex. As rotas
  `/api/codex-contas/*`, `/api/credenciais/codex*`, `/api/claude-configs*`,
  `/api/conta-estado*`, `/api/cotas` e `/api/cotas/sugestao` entram nessa frente.
- O serviço é `hangar-server::accounts`; o cliente efêmero é o `hangar-codex` já disponível.
  O app-server administrativo não usa nem ocupa o cliente de uma sessão.
- `POST /api/sessions` continua na parte 6. Seus consumidores Python e os adaptadores Rust
  usam o mesmo lock de existência da conta; criar sessão, transferência e lançadores não
  migram na #115. Leitores locais puros continuam até as partes 6/7.
- O Rust grava marcadores, credencial do device flow e cache quando ativo. Os CLIs gravam
  sua autenticação nativa. Preparação de configuração, hooks, skills e plugins fica nos
  reconciliadores Python até a 5F, por gancho interno restrito. A propagação do device flow
  grava apenas Pi/omp no Python; não pode voltar a gravar `auth.json` do Codex.
- Escopos de conta para custos (`costs/codex.rs`, `CodexScope`) usam os retratos públicos de
  contas, sem entregar credenciais ao consumidor.

Contratos, consumidores e autoria de arquivos:
[inventário e referência Python](../../../backend/tests/fixtures/accounts_contract/README.md).
Falha de conta migrada com Rust ativo é erro identificado; a reserva Python é do processo
inteiro. Modelos ficam prontos quando catálogo e escolha forem usados pelo app. A #115 tem
aceite próprio de gestão de contas nas três interfaces, sem consumir redefinição real na prova.

### 5F. Integração nativa e sincronização entre contas

- `hangar-codex::integration`: o reconciliador (`codex_integracao`), o cliente admin
  (`codex_importador`: `externalAgentConfig/*`, `codex plugin …`), instruções
  (`AGENTS.override.md`), ganchos (`hooks.json`, arquivos de gancho, instalador dos ganchos do
  Hangar), skills, fragmentos, adaptação de ganchos do Claude (`codex_compat`), utilitários de
  arquivo (`codex_arquivos`), opções de contexto (`codex_opcoes`), catálogo de mensagens
  (`codex_msgs`), sincronização entre contas (`codex_contas_sync`, `codex_contas_plugins`).
- **TOML:** leitura com o crate `toml` (novo no workspace do servidor; o nativo já usa); escrita
  continua pelo próprio Codex (`config/batchWrite` numa cópia, troca com conferência), como hoje.
  O bloco `[model_providers.X]` de `agentes_sync.gravar_codex` continua emenda de texto, em Rust.
- Gatilhos iguais aos de hoje: abertura de sessão Codex e botão Reconciliar; nunca gravar
  confiança para autoaprovar ganchos (regras vigentes). Intervalo de 6 h e nova tentativa em 5 min
  continuam.
- Rotas `/api/harness/codex/*` e a parte Codex de `config_sync` (`_export_codex`/`_apply_codex`)
  e de `harness_saude` passam a chamar o Rust.
- Windows: travas (`LockFileEx` no lugar de `msvcrt`), caminhos UNC e `PureWindowsPath` viram
  testes Rust com casos Windows; leitura de "Regras vigentes" de `windows.md` antes de cada Task.

Pronta quando: reconciliar numa conta limpa e numa já integrada dá o mesmo resultado que o
Python (comparação de árvore de arquivos), e a sincronização leva uma skill e um plugin para uma
conta adicional.

### 5G. Transferência Claude → Codex

`transfer.py`, `claude_to_codex.py` e os trechos Codex de `conversation_transfer.py`,
`conversation_history.py` e da rota `/conta` vão para `hangar-codex::transfer` +
`hangar-server`. Depende de 5B (sessão), #115 (conta) e 5E (catálogo). A regra vigente de prova vale:
conferir os itens enviados após retomada, incluindo resultados completos.

### 5H. Voz (beta)

`codex_voice.py` e `codex_voice_broker.py` vão para o `hangar-server` (WebSocket
`/codex/voice`, `/codex/voices`). O broker deixa de importar `app.api` e manda pela fila do Rust.
Continua opt-in por `codex_voice_beta`.

### 5I. Lançadores e ganchos sem Python

- `hangar-codex-tui` (lançador do pane), `hangar-codex` (wrapper do shell) e os adaptadores de
  gancho `codex-hook-allow.py`/`codex-hook-json.py` viram subcomandos de um binário Rust que usa
  `hangar-codex` (sidecar, contas, instruções, transferência). Detalhes que valem: reinício do
  app-server na mesma porta com `_QUEDAS_MAX`, `finally` que só apaga o sidecar se o `app_pid`
  ainda é o dele, `codex.CMD`/PATHEXT no Windows, sem SIGHUP.
- `shell/codex.{fish,posix.sh,ps1}` e `install-claude-wrapper.sh` apontam para o binário;
  binário ausente cai no script Python (reserva até a parte 7, como `cano.py`).
- Antes das Tasks: "Regras vigentes" de `instalacao.md` (o binário entra no pacote da máquina).

Pronta quando: abrir Codex com terminal pelo app e pelo shell, numa conta adicional, com a TUI
reiniciando o app-server depois de um kill, sem `python` no `ps` do pane.

## Destino de cada arquivo

| Arquivo (linhas) | Vai para | Subparte |
|---|---|---|
| `adapters/codex/appserver.py` (431) | `hangar-codex::client` | 5A |
| `adapters/codex/adapter.py` (2626) | motor `runtime/codex.rs` + `runtime/codex_terminal.rs` (WebSocket) + rotas `routes/codex.rs` | 5B/5C |
| `adapters/codex/sem_terminal.py` (203) | `runtime/codex.rs` (subida, modos, kill) | 5B |
| `adapters/codex/sessions.py` (395) | `hangar-codex::sidecar` | 5B (lançador usa em 5I) |
| `adapters/codex/lancador.py` (100) | `hangar-codex::launcher` | 5C/5I |
| `adapters/codex/questions.py` (45), `async_questions.py` (135) | `runtime/codex.rs` (já portado; conferir paridade) | 5B |
| `adapters/codex/chat_controls.py` (19) | `runtime/codex.rs` (catálogo de skills) | 5B |
| `adapters/codex/rollout.py` (373) | já em `transcript/codex.rs`; `status_line_do_rollout` → `list/` | 5B |
| `adapters/codex/transfer.py` (360), `claude_to_codex.py` (409) | `hangar-codex::transfer` | 5G |
| `codex_permissions.py` (133) | `terminal_state.rs` + escritor terminal | 5C |
| `codex_contas.py` (246) | `hangar-server::accounts`; leitor Python puro fica até 6/7 | #115 |
| `codex_contas_api.py` (282), `codex_contas_login.py` (656) | `hangar-server::accounts` (rotas, login e preparação) | #115 |
| `codex_appserver.py` (227) | `hangar-codex::client` (stdio efêmero) + `accounts::quotas` (HTTP) | 5A/#115 |
| `codex_models.py` (296) | `hangar-codex::models` | 5E |
| `oauth_codex.py` (376) | device flow no `hangar-server::accounts`; propagação Pi/omp por gancho Python | #115 |
| `uso_codex.py` (218) | conferir se `/api/uso` já é do Rust (parte 3); se sim, é referência e sai na 7 | 5E |
| `codex_contas_sync.py` (1318), `codex_contas_plugins.py` (645) | `hangar-codex::integration::sync` | 5F |
| `codex_integracao.py` (1105), `codex_importador.py` (409), `codex_instrucoes.py` (220), `codex_hooks_arquivos.py` (158), `codex_hook_installer.py` (136), `codex_fragmentos.py` (134), `codex_skills.py` (393), `codex_compat.py` (291), `codex_arquivos.py` (268), `codex_opcoes.py` (66), `codex_msgs.py` (104) | `hangar-codex::integration::*` | 5F |
| `codex_voice.py` (149), `codex_voice_broker.py` (365) | `hangar-server` (`voice/`) | 5H |
| `scripts/hangar-codex-tui` (662), `scripts/hangar-codex` (290), `codex-hook-allow.py` (49), `codex-hook-json.py` (52) | subcomandos do binário Rust; o script fica de reserva até a 7 | 5I |
| `shell/codex.{fish,posix.sh,ps1}` (63) | ficam (são funções de shell), apontando para o binário | 5I |
| Ramos Codex em `api.py`, `registry.py`, `sse.py`, `state.py`, `terminal_input.py`, `runtime_*.py` | somem do caminho com o Rust de pé; ficam só para o modo `python` | 5B/5C |
| Ramos Codex em `cotas.py`, `credenciais.py`, `costs_sources.py` | contas/cotas e retratos públicos no Rust; outros provedores continuam no Python | #115 |
| Ramos Codex em `harness_saude.py`, `harness_api.py`, `config_sync.py`, `agentes_sync.py` | integração geral no Rust | 5F |
| Ramos Codex em `archive*.py`, `bastao.py`, `orq_politica.py`, `cliproxy_accounts.py` | ficam: módulo compartilhado, sai inteiro na parte 6 | — |

Todo arquivo Python acima fica no disco como reserva do modo `python` e sai na parte 7.

## Desempenho (conferido contra "erros que já custaram")

- Nenhum laço: `watch_sessions` (2 s) e o vigia de pane (1 s) viram eventos com teto; terminais em
  segundo plano e cota lidos sob demanda ou por evento.
- Um consumidor por sessão; canais limitados; deltas coalescidos em 150 ms; estado republicado só
  quando muda; linha de status recalculada só quando as entradas mudam.
- Cota e catálogo com cache em disco e teto; 429 espera 10 min; nada de chamar o Python.
- `codex --version` e app-server efêmero custam processo (pior no Windows): cache por binário
  (caminho + mtime).
- Leitura de rollout só pela cauda; `serde` emprestado onde o dado não sobrevive.
- Medir antes/depois em release (CPU e pico de RSS) com 10 sessões Codex trabalhando: Python atual
  × Rust.

## Como provar

Cada subparte: testes Rust do que mudou + golden onde o cliente lê formato + uso real pelo canal
de testes com o dono (roteiro na própria subparte) + `CP_RUST_SERVER=0` continua abrindo o Codex
pelo Python. Codex real só com a conta do dono e na máquina dele, nunca credencial copiada.
O uso real da parte 5 (Codex e Claude) é no backend do notebook, pelo canal de testes, feito pelo
dono com a sessão `hangar` de lá: esta sessão nunca sobe nada nesse backend, só avisa quando a
branch está pronta para testar.

## Regras que mudam

- `harnesses.md`, "Codex sem terminal: o app-server é do CANO": "Pedido do servidor sem tela
  recebe `-32601`" passa a valer só para os pedidos da última linha da tabela de 5B.
- `dono-unico/desenho.md`, decisão 2 (Codex fica no Python): substituída por esta spec depois de
  5B provada; entra em `superado.md`.
- README da migração, "Ainda no Python": sai o Codex por subparte.
- Regra nova em `harnesses.md`: "Protocolo do Codex é tipado e tolerante; campo usado existe no
  recorte do schema conferido; versão diferente avisa".

## Decisões para o dono

1. **Tipos:** (a) enxuta, como acima. ➡️ recomendada.
2. **Onde moram os lançadores em Rust:** subcomandos do `hangar-cano` (já é por sessão, já vai em
   todo pacote, já é procurado pelo `rust_bins.find_bin`; nenhum artefato novo no instalador) ou
   binário novo `hangar-codex`. ➡️ `hangar-cano`.
3. **Leitor Python de contas fica até as partes 6/7** para os 22 módulos que só leem. ➡️ sim.
4. **Ordem:** 5A → (5B ‖ 5E) → 5C → 5D/5F/5G/5H → 5I. ➡️ esta.
5. **5D (fork, revert, objetivo, terminais) dentro da parte 5** e não num trabalho à parte. ➡️ sim,
   depois de 5B/5C, porque usa o mesmo motor e os mesmos tipos.
