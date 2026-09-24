# Task 5: barra direita e controles da sessão

Executor `native-parity-task5-exec2` (claude-opus-5-5, `--effort medium`, `CLAUDE_CONFIG_DIR=/home/jefferson/.claude-claude-200-3`), substituto de `native-parity-task5-exec` (conta Jefferson esgotada; nada tinha sido congelado). Worktree `hangar-native-desktop`, HEAD 55e0d909 destacado, sem stage/stash/commit. Base de comparação: `baseline-task5/`. Matriz por capacidade em [`chat-parity.md`](chat-parity.md).

## O que mudou

- `src/status.rs` (novo): porta de `parseStatusLine` do core. Modelo/esforço, contexto usado/janela (dois pares ou rótulo `ctx`), último turno, custo, janelas de 5 h/7 d/30 d com horário de volta, tempo de sessão, repo/branch/sujo. Codex e Claude sem terminal pegam Git da lista. Campo ausente fica `None`. Cinco testes compilados.
- `src/api`: campos novos em `SessionInfo`/`SessionState` (linha de status, modo do Codex/Claude, modo anterior ao plano, motivo de recarregar, limite, loop, branch/git) e `Stats` do evento `stats`; `read()` genérico (GET sem efeito, queda = rede) e `config()`.
- `src/app/side.rs` (novo): painel direito recolhível e redimensionável (240–480 px, nunca deixa a conversa abaixo de 540 px). Estado, detalhe e loop; contexto, custo, último turno, parada, tempo, estatísticas; aviso de contexto com Compactar; limites; aviso de recarregar com confirmação; projeto com arquivos alterados e diff; atalhos da config (`send_text`, `shell`, anexos). Custo do Codex por `GET /cost` só com o painel visível, a cada 30 s, cancelado ao trocar/fechar. Três testes compilados.
- `src/app/controls.rs` (novo): fileira de modelo/esforço/modo/permissão por provider, catálogo lido no gesto, leitura pelo terminal só com botão explícito, resposta amarrada à sessão e ao valor vivo capturado no gesto. Implementar plano do Claude sem terminal (troca de modo → envio → volta ao plano se o envio falhar). `/plan-preview` do Claude com terminal. Codex antes da conversa: pergunta da lista respondida por `/select` com conferência de que ela não mudou. Um teste compilado.
- `src/app.rs`: ligação dos módulos, evento `stats`, respostas `Reply`/`HeadlessPlan`/`Config`, confirmações novas (preencher, atalho, recarregar), Esc na raiz fecha painéis com o foco fora do campo, texto de interromper por provider (NOTED5 da Task 4), estado do cabeçalho para sessão sem conversa.
- `src/theme.rs`: cor `danger` para medidor acima de 90%. `src/main.rs`: `mod status`.
- `messages/{pt,en}.json`: 127 chaves `native_*` novas em cada idioma, nenhuma alterada ou removida fora delas.
- `tools/parity_session_fixture.py` (novo): backend sintético com seis sessões (Claude com e sem terminal, Codex, Codex antes da conversa, Pi, Kimi), log de mutações e falhas programáveis em mutações e leituras.

Corrigido nesta rodada, depois das provas do executor anterior:
1. Esforço/modelo aplicado com a sessão fora da tela voltava com a pílula no valor antigo (aviso dizia "low", pílula "high"). Agora o valor vivo é capturado no gesto e viaja com o pedido (`Reply::Applied(ctl, label, before)`).
2. "Tentar novamente" do painel de controle fechava o painel (reusava o gesto de alternar). Agora relê.
3. Texto `native_side_changes` em português: "no working tree" → "na working tree".

## Itens herdados

| Item | Onde |
|---|---|
| NOTED1 Task3: aprovação dos hooks do Codex antes da conversa | `controls.rs` `render_prethread`/`preselect`: pergunta e opções da lista, `POST /select {"option": n}`, sem `request_id`; conferida a pergunta antes de enviar |
| NOTED2 Task3: implementar plano do Claude sem terminal | `controls.rs` `implement_headless`: `permission-mode` → `input` → volta ao plano se o envio falhar; incerteza não volta |
| NOTED3 Task3: `/plan-preview` do Claude com terminal | `controls.rs` `discover_plan`/`render_plan_preview`: metadados ao abrir e ao fim do turno, conteúdo só no gesto |
| NOTED5 Task4: texto de interromper | `native_stop_confirm_direct` para Codex e sem terminal; "A sessão recebe Esc." só no Claude com terminal |

## Verificação

```
cd desktop-native && cargo build --locked        → exit 0; 2 avisos que já existiam na base (campos de DTO não lidos)
cd desktop-native && cargo test --locked --no-run → exit 0, "Executable unittests src/main.rs"; nenhum teste executado
```

Falharia se: o cliente não compilasse, ou os testes novos (`status::tests`, `app::side::tests`, `app::controls::tests`) deixassem de compilar.

## Prova na janela real

Binário final `bin/hangar-native-task5-r1f` (sha256 `a56a239cd163e6a88e1ed302db10b0d71f71896a89235a47a4518fc403534bba`), unidade `hangar-native-task5-proof`, janela própria no ws13 do DP-3 (saída física existente). Fixture sintética na porta 18796 (unidade `hangar-native-task5-fixture`). PID ativo, workspace 13 e posição do cursor conferidos antes de cada clique (`/tmp/t5x.sh`, aborta se divergir). Capturas em `~/.hangar/orq/2026-09-23-native-parity/visual/task5-r1/`.

Capturas `00`–`39` são do executor anterior (binários r1a/r1b), `r1c-40`…`48` do r1c, `r1d-53`…`55` do r1d e `r1e-53`…`79` do r1e: todas anteriores ao binário final. O r1e difere do r1f só no clique de "Tentar novamente" do painel de controle; os fluxos do r1e abaixo não passam por ele.

### No binário final (r1f)

| Caso | Captura | Resultado |
|---|---|---|
| Painel do Claude com terminal | `r1f-80-claude-final.png` | estado, modelo·esforço, 72% com barra laranja, 720k de 1M, custo, último turno, parada, tempo, estatísticas, aviso de 60% com Compactar, limites 5 h/semana, projeto `hangar · main*`, atalhos |
| Catálogo com erro e "Tentar novamente" | `r1f-81-model-error.png`, `r1f-82-model-retried.png` | "Não deu para ler as opções: leitura recusada"; o botão relê e mostra Opus 5.5 (✓ atual, cinza), Sonnet 5, Haiku 4.5 |
| Aplicar modelo | `r1f-83-model-applied.png`, `r1f-84-model-live.png` | `POST /model-effort {"model":"sonnet","scope":"session"}`; aviso "Modelo: Sonnet 5"; linha de status muda e o painel mostra "Sonnet5 · high" |
| Ler plano (`/plan-preview`) | `r1f-85-plan-preview.png` | conteúdo do plano em Markdown renderizado; botão vira "Fechar" |
| Permissão do Codex com terminal | `r1f-86-codex-perm.png`, `r1f-87-codex-perm-read.png`, `r1f-88-codex-perm-applied.png` | abrir não lê nada ("Ler do terminal"); ler lista Ask for approval / Approve for me / ✓ Full Access; aplicar deixa a pílula "Permissão: Ask for approval" |
| Esforço do Kimi com resposta lenta e troca de sessão | `r1f-89-kimi-applying.png`, `r1f-90-pi-after.png`, `r1f-91-kimi-back.png` | "Esforço: aplicando…"; no Pi nenhum aviso do Kimi; de volta ao Kimi, aviso "Esforço: low" e pílula "Esforço: low" |
| Falha 409 ao aplicar | `r1f-93-pi-409.png` | "a sessão recusou agora"; pílula continua `cline-pass/kimi-k3` |
| Esc com o foco fora do campo | `r1f-94-esc-closed.png` | clique na área neutra do painel e Esc: painel fecha |
| Interromper por provider | `r1f-95-codex-stop.png`, `r1f-96-claude-stop.png` | Codex: "Interromper a resposta atual?"; Claude com terminal: "… A sessão recebe Esc." |

### No binário r1e (anterior ao final, fora do caminho alterado)

| Caso | Captura | Resultado |
|---|---|---|
| Arquivos alterados: vazio, erro, lista, diff | `r1e-57-files-empty.png`, `r1e-58-files-error.png`, `r1e-59-files.png`, `r1e-60-diff.png` | "Nenhum arquivo alterado.", "leitura recusada" em laranja, três arquivos com +/−, diff de `src/app.rs` |
| Recolher, mostrar e arrastar a borda | `r1e-63-side-hidden.png`, `r1e-64-side-wide.png` | "Mostrar painel" devolve o painel; arrasto até 480 px, conversa continua legível |
| Custo do Codex | `r1e-65-codex.png`, `r1e-66-codex-cost-stale.png` | US$ 1.23 "estimativa pela tabela da API"; com a leitura falhando, "último valor; a leitura falhou: custo indisponível agora"; log: `GET /cost` a cada 30 s e nenhum depois de trocar de sessão |
| Codex antes da conversa | `r1e-67-prethread.png`, `r1e-68-prethread-409.png`, `r1e-69-prethread-sent.png`, `r1e-70-prethread-opened.png` | pergunta e opções da lista; 409 com motivo; reenvio "Escolha enviada"; conversa abre sozinha com "A medida do contexto chega depois do primeiro turno." (desconhecido, não zero) |
| Claude sem terminal: Compactar com rascunho, implementar plano | `r1e-71-headless.png`, `r1e-72-compact-protect.png`, `r1e-73-hplan-implement.png` | aviso de 85% em laranja; Compactar sobre "meu rascunho" pede para trocar; Implementar: `permission-mode {"mode":"acceptEdits"}` → `input "Implemente o plano proposto."`, modo vira "Aceitar edições" |
| Recarregar | `r1e-74-reload-state.png`, `r1e-75-reload-confirm.png`, `r1e-76-reloaded.png` | confirmação explícita; `POST /recarregar`; aviso some e "Sessão recarregada." |
| Atalho shell | `r1e-77-shell-confirm.png`, `r1e-79-shell-launched.png` | "Rodar o atalho "Build"?"; só após "Rodar": `POST /shortcut-shell {"command":"echo sintetico"}` |
| Kimi: loop e contexto | `r1c-47-kimi.png` | "Loop 3/10 · rodando", 48%, custo "—" (desconhecido) |
| Modo do Claude com ciclo desconhecido | `r1c-43-mode-unknown.png` | todos os modos indisponíveis sem "só ao criar a sessão"; botão "Ler do terminal" |

Os caminhos de falha do plano sem terminal (troca recusada, envio recusado com volta ao plano) e o Codex modelo/modo foram provados pelo executor anterior (`15`–`17`, `30`–`31`); o código deles não mudou depois.

## Limites e riscos

- Fixture sintética: os formatos seguem as rotas do backend ativo, mas nenhuma sessão real foi mudada. A linha de status do Kimi na fixture não muda depois do esforço (só a resposta da rota), por isso o rótulo aplicado vale até a fonte mudar.
- `before` capturado no gesto: se a linha de status da sessão mudar por outro motivo antes da resposta, o rótulo aplicado some e a pílula mostra o valor vivo.
- Arrastar e colar anexos, voz, terminal, navegador e Git completo continuam fora (matriz).
- Demo: pendente de propósito. Por decisão da árbitra, o binário final sobe numa unidade nova ao lado do demo `hangar-native-parity-demo-20260923` (PID 4190872, não fechar) só depois do APROVA; a janela da Task 4 (PID 684636) pode ser substituída.
- Contexto usado por esta sessão: cerca de 300 mil tokens (estimativa pelo consumo da conversa), menos de 50% da janela.

## Rodada 2

Parecer `pareceres/task5-r1.md` (REPROVA, BLOCKER 1) e ordem da árbitra (incluir NOTED 1 e 4; NOTED 2 e 3 pendentes na matriz).

- BLOCKER 1, seletor com mais de uma linha "atual": `same()` (substring nos dois sentidos) apagada. `only_match` (Kimi modelo: nome exato sem diferença de maiúsculas; nome repetido entre providers não marca nenhuma, porque a linha de status não diz qual é), `claude_model_current` (porta de `matchCurrent`: linha de status antes do `active` defasado; `[1m]` decidido pela janela citada), `claude_effort_current` (exato, senão prefixo). Kimi esforço acha o modelo por nome exato; permissão do Codex marca `atual`, senão o nome exato. Teste compilado novo com os casos da receita (K3/K3-256k, K2.7 Coding/Highspeed, nome repetido, Fable com `active` no Opus, `opus`/`opus[1m]`, `med`).
- Varredura: `grep -rn 'same(' desktop-native/src/` sem resultados; os ramos que já comparavam exato (Codex modelo/esforço/modo, Pi, modo do Claude) não mudaram.
- Fora da receita, mesmo `choices`: com o ciclo do Claude lido, "só ao criar a sessão" aparecia em todos os modos; agora só nos que estão fora do ciclo (`15-claude-mode-read.png`).
- NOTED 1: `implement_headless` confere entrega pendente e fila antes de travar a sessão em "implementando".
- NOTED 4: o botão que só esconde o aviso do plano do Claude com terminal diz "Ocultar aviso" (`native_plan_preview_hide`, pt e en).
- Fixture: catálogo do Kimi com os nomes do `~/.kimi-code/config.toml` real (K3, K3-256k, K2.7 Coding, K2.7 Coding Highspeed) e linha de status que acompanha a troca; `active` do Claude preso no Opus, como o picker real.

Binário final `bin/hangar-native-task5-r2b` (sha256 `b6bb1bb14dc15e1247b017a91601bcb7e1ce67ed93df2677f778055d6cddfe48`). Capturas em `visual/task5-r2/` (as do r2a, anterior à correção dos modos, em `visual/task5-r2/r2a/`, fora das hashes):

| Caso | Captura | Resultado |
|---|---|---|
| Kimi em K3 | `01-kimi-model-k3.png` | só K3 com ✓; K3-256k, K2.7 Coding e Highspeed clicáveis |
| Trocar para K3-256k e reabrir | `02-kimi-applied.png`, `03-kimi-reopen-256k.png` | `POST /kimi/model {"model":"k3-256k"}`; pílula "Modelo: K3-256k"; só K3-256k com ✓ e K3 clicável |
| Trocar para K2.7 Coding e reabrir | `04-kimi-reopen-coding.png` | só K2.7 Coding com ✓; Highspeed clicável |
| Esforço do Kimi | `05-kimi-effort-list.png` | níveis do K2.7 Coding (só high, marcado) |
| Claude com `active` defasado | `06-claude-model-opus.png`, `07-claude-model-sonnet.png` | antes: ✓ Opus 5.5; depois de aplicar Sonnet (`POST /model-effort {"model":"sonnet","scope":"session"}`), o picker ainda diz Opus, e o ✓ fica no Sonnet 5 com Opus 5.5 clicável; barra do plano com "Ocultar aviso" |
| Esforço do Claude | `08-claude-effort-list.png` | seis níveis, só high marcado |
| Modo do Claude lido do terminal | `15-claude-mode-read.png` | Plano, Automático, Aceitar edições disponíveis sem nota; ✓ Manual; Sem permissões e Não perguntar cinza com "só ao criar a sessão" |
| Codex modelo, esforço, modo, permissão | `09-codex-model.png`…`12-codex-permission.png` | uma linha marcada em cada: GPT-6 Astra, high, Padrão, Full Access |
| Pi modelo e esforço | `13-pi-model.png`, `14-pi-effort.png` | ✓ Kimi K3; ✓ high |

```
cd desktop-native && cargo build --locked        → exit 0; 2 avisos que já existiam na base
cd desktop-native && cargo test --locked --no-run → exit 0, "Executable unittests src/main.rs"; nenhum teste executado
```

## Step 15

Aprovada na rodada 2 (`pareceres/task5-r2-arbiter.md`). Nenhum código mudou depois do r2b.

- Demo final: unidade `hangar-native-parity-task5-final-20260924.service` (systemd-run, cgroup `app.slice`, independente das sessões de agente), PID 1006328, binário `bin/hangar-native-task5-r2b` (sha256 `b6bb1bb14dc15e1247b017a91601bcb7e1ce67ed93df2677f778055d6cddfe48`), ws12 do DP-3. Conectada ao backend real `http://127.0.0.1:8765`; token digitado pelo stdin do `wtype`, nunca em argv, arquivo ou log. Nenhuma sessão aberta nela (`visual/task5-r2/final-01-connected.png`).
- Demo anterior preservada: `hangar-native-parity-demo-20260923.service`, PID 4190872, ws9, não tocada (pode ter rascunho do usuário em memória).
- Encerrados por PID conferido: janela da Task 4 (`hangar-native-parity-task4-final-20260924`, 684636), janela de prova (`hangar-native-task5-proof`, 989753), fixture (`hangar-native-task5-fixture`, 979975; porta 18796 fechada).
- Tela: DP-3 no ws9 (`hyprctl monitors`), eDP-1 no ws2, HDMI-A-1 no ws5. Janelas nativas vivas: 4190872 (ws9) e 1006328 (ws12).
- Índice do git vazio; nenhum stash, commit ou push.
