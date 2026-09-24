# Task 6 — paridade visual do layout e prévia duplicada (rodada 1)

Régua: app web/Electron atual (`frontend/src/components/Composer.svelte`, `Sidebar.svelte`,
`DesktopSessionContext.svelte`, tokens de `frontend/src/app.css`). Binário da rodada:
`~/.hangar/orq/2026-09-23-native-parity/bin/hangar-native-task6-r1`
(sha256 `b94e8eaeccbae4450f20dc99d12b0830de6197cfd8eb8cacb82c423713a9984c`). Todas as capturas
`native/r1-*` e `side/*` são desse binário; as `native/a*`–`d*` são de binários intermediários
(`t6a`–`t6e`) e ficam só como histórico.

Capturas: `~/.hangar/orq/2026-09-23-native-parity/visual/task6-r1/` (`web/`, `native/`, `side/`).

## Step 16 — prévia duplicada

**Causa.** O web tem três donos da prévia; o nativo só tinha dois. Quando o pane (ou o retrato
de reconexão) reemite a última prosa depois que ela já foi gravada e uma ferramenta começou, o
nativo aceitava o quadro como prévia nova e desenhava a resposta de novo com "Trabalhando". O web
descarta esse quadro pela regra (c) de `Chat.svelte` (~linha 2598): prévia com 16+ caracteres
contida em qualquer das 6 últimas respostas gravadas não é prévia.

**Correção.** `desktop-native/src/chat.rs`: `update_preview` aplica a mesma regra
(`recently_committed`), e a normalização saiu para `flatten`, usada também por
`preview_matches`. A regra "resposta não relacionada não limpa a prévia" (teste
`unrelated_assistant_commit_and_unchanged_replay_keep_preview`, decisão anterior) ficou intacta.
Teste novo: `pane_replay_of_a_recorded_answer_is_not_a_preview` (compilado, não executado).

**Prova no binário (fixture determinística).** Modo novo `replay` em
`tools/parity_stream_fixture.py`: grava a resposta com acento depois do ponto ("…janela. É só
um clique na sessão."), abre um `tool_use`, e reemite a mesma prévia com a sessão trabalhando.

| Binário | Captura | Resultado |
|---|---|---|
| antes (`hangar-native-task5-r2d`, a demo atual) | `native/e01-replay-before.png` | resposta aparece duas vezes, a segunda com "Trabalhando" (o defeito do print do usuário) |
| depois (`task6-r1`) | `native/r1-14-replay-after.png` | uma vez só, seguida do `Bash … em execução` |

O que faria a prova falhar: a segunda cópia com "Trabalhando" abaixo do `Bash` na captura do
depois.

**Sessão real: não reproduzi.** Três tentativas com o binário antigo, só assistindo:
minha própria sessão (sem terminal) reaberta no meio de uma ferramenta (`d01`, `d08`), e uma
sessão descartável `cx-t6-dup` com terminal na 200-3 (Opus 5.5) fazendo texto + ferramentas
longas (`d03`–`d06`). Nenhuma mostrou a duplicação; o defeito depende do momento em que o pane
ou a reconexão reemite o texto. A prova de "antes" vem da fixture acima. `cx-t6-dup` foi
fechada no fim.

## Step 17 — layout

### Compositor (`app.rs` `render_composer`, `app/controls.rs` `render_ctl_pills`, `app/chrome.rs`)

- Cartão único como o `.composer-card`: raio 18, borda de vidro, fundo `--chrome-bg`, sombra
  `0 12px 40px`, borda accent a 45% com o campo em foco; largura até 1400 centrada.
- Aba de estado acima do cartão: fila ("⏳ N na fila · mandar agora", que substitui o antigo
  botão de texto), repositório com ícone de pasta, branch em mono e `*` em âmbar, e o anel de
  contexto de 22 px (arco a partir do topo, âmbar a 70, vermelho a 90).
- Campo sem moldura, placeholder "Mensagem para {Claude|Codex|Kimi|Pi|OMP}…"; a sugestão do
  terminal virou placeholder "{texto} · Tab para usar", como no web.
- Linha de controles: Comandos (ícone), pílulas de modelo e esforço (✦ no Claude), permissão no
  Codex, Anexar (clipe), Recentes (relógio), modo com seta; à direita "Orientar agora" quando
  cabe, e Enviar (44×44 accent, seta) que vira Parar (44×44, quadrado vermelho) com a sessão
  trabalhando e o campo vazio.
- A linha solta "Modelo/Esforço/Modo" e a dica de atalhos espremida sumiram: a dica foi para o
  tooltip de Enviar e de Parar.
- Anexos: miniaturas 56×56 raio 12 com × no canto; falha e envio em curso numa linha abaixo.
- Seletores, comandos, recentes e confirmação flutuam sobre a conversa presos ao topo do
  compositor, com a borda e a sombra de popover (`--elev-2`).
- Faixa de estatísticas abaixo do cartão (11 px, centrada), que saiu do painel direito.

### Barra lateral (`app.rs` `render_session_row`)

Marca do Hangar + "Hangar" e selo "Experimental"; cabeçalho "SESSÕES" com contagem e contagem
de aguardando; linhas com marca na cor do estado (selo do provider no canto só com providers
misturados), nome, linha de estado (pergunta em âmbar, rótulo do trabalho em itálico), pasta,
branch (sem main/master) e +A −R; selo de estado à direita (ponto verde no ocioso, pílulas "em
execução", "aguardando", "encerrado", "limite de uso"); aguardando com borda e fundo âmbar;
selecionada com accent a 10%. Rodapé com "Conexão" (ícone de tomada).

### Painel direito (`app/side.rs`)

Cartão com raio 24 e margem 12; cabeçalho com nome em mono, detalhe do estado e selo; botão de
recolher com ícone no canto (o "Ocultar painel" de texto saiu; fechado, um ícone no cabeçalho
da conversa reabre). Atalhos viraram a faixa de ações com ícone em cima e rótulo embaixo.
Contexto: número grande (44 px) com cor por limiar, "do contexto · usado de total", custo à
direita, barra de 4 px, linha mono com parada/último turno/sessão, limites em barras com
"reseta". Avisos (processo desatualizado, contexto cheio) no molde `.aviso`. Seção
"REPOSITÓRIO" em caixa alta com +A −R coloridos. Rodapé "Provider · servidor" e fila.

### Tokens (`theme.rs`)

Cores de `app.css`: warning `#ff9f0a`, erro `#ff453a`, sucesso `#34c759`, `--chrome-bg`,
bordas, pílulas de estado, `--font-mono` (JetBrainsMono Nerd Font). Toda cor da tela sai daqui.

## Step 18 — lado a lado (`side/`)

| Área / estado | Folha | Web | Nativo |
|---|---|---|---|
| Barra lateral com estados e seleção | `side/sidebar.png` | `web/01` | `native/r1-15` (backend real) |
| Painel direito com dados | `side/rightbar.png` | `web/01` | `native/r1-15` |
| Compositor vazio | `side/composer-idle.png` | `web/01` | `native/r1-15` |
| Compositor trabalhando (Parar) | `side/composer-working.png` | `web/06` | `native/r1-08` |
| Seletor de modelo aberto | `side/composer-model-picker.png` | `web/02` | `native/r1-02` |
| Seletor de esforço aberto | `side/composer-effort-picker.png` | `web/03` | `native/r1-03` |
| Seletor de modo aberto | `side/composer-mode-picker.png` | `web/04` | `native/r1-04` |
| Comandos | `side/commands.png` | `web/05` | `native/r1-05` |

Estados só no nativo (fixture `parity_session_fixture.py`, sem par web aberto na mesma
condição): com texto `r1-06`, trabalhando com texto `r1-07`, anexos `r1-09`, Codex com
permissão, limite e custo estimado `r1-10`, pergunta do terminal `r1-11`, Kimi trabalhando com
loop `r1-12`, sem terminal com plano, processo desatualizado e contexto a 89% `r1-13`.

## Diferenças declaradas

| Diferença | Motivo |
|---|---|
| Sem abas Contexto/Arquivos/Git, "Mais alterados" e "Equipe" no painel | funções que o nativo não tem; o painel mostra o que existe |
| Sem Quadro/Canvas, busca, seleção/broadcast, grupos pareados e "+ Nova" na barra lateral | idem; "Conexão" ocupa o rodapé |
| Sem última resposta ("◆ …") e hora na linha ociosa | o DTO da lista do nativo não lê `last_reply`; ler é mudança de dados fora do layout |
| Sem microfone, ditado e orquestração no compositor | funções ausentes no nativo |
| Recentes como botão próprio no compositor | o web não tem esse botão no compositor; no nativo o fluxo já existia aqui e ganhou ícone |
| Popover alinhado à esquerda do compositor, não ao gatilho | GPUI sem âncora simples no botão; mesma superfície (borda, sombra, largura 380) |
| Comandos como painel sobre a conversa, não modal central | segue o padrão de popover do compositor; a lista e a busca são as mesmas |
| Marca sem animação quando trabalhando | cor accent marca o estado; animação de arcos não portada |
| Nome longo quebra no meio da palavra | GPUI não quebra em hífen como o navegador |
| Limites em uma coluna com o painel estreito | mesma regra do web (colunas de 118 px); cabem duas a partir de ~300 px |
| `px()` com os valores do CSS | o guia do gpui-kit prefere `rem`; aqui a medida é a do web por ordem do usuário |
| Seleção sem `cursor_pointer` | guia do gpui-kit: cursor padrão em botão nativo |

## Verificação

- `cd desktop-native && cargo build --locked` → exit 0, só os 2 avisos antigos (`problema`,
  `steered`/`native` nunca lidos).
- `cd desktop-native && cargo test --locked --no-run` → exit 0,
  `Executable unittests src/main.rs (target/debug/deps/hangar_native-0193f3a534eae887)`.
  Nenhum teste executado. `panel_never_squeezes_the_chat` teve a expectativa de 1060 px
  ajustada a 250 (barra lateral 270, como o web); o binário não muda com isso (conferido por
  `cmp`).
- Uso real: janela própria no DP-3 (ws12, vazio), cliques com PID, workspace, geometria e
  cursor (±3 px) conferidos antes de cada um; foco devolvido ao ws5 do usuário após cada
  rodada; o DP-3 ficou no ws12 o tempo todo. Diálogo de arquivos (KDE) conferido por classe e
  workspace antes de digitar. Fixtures com `XDG_CONFIG_HOME` próprio: a conexão lembrada do
  usuário não foi regravada.

## Referência web

Capturada no navegador embutido desta sessão, que já veio autenticado pelo perfil do app. Nada
digitado na página nem enviado. Abrir a barra lateral gravou `cp_sidebar_collapsed=0` no perfil;
restaurei `1` no fim (o app não escuta `storage`, a janela do usuário não mudou). Numa leitura
do `localStorage` para achar essa chave, o primeiro caractere do token apareceu no meu
transcript (corte de 80 caracteres); nenhum outro trecho dele.

## Não conferido

- Duplicação em sessão real (acima).
- Tema claro: o nativo só tem o escuro.
- Zoom/`rem` diferente do padrão.

## Rodada 2

Binário `~/.hangar/orq/2026-09-23-native-parity/bin/hangar-native-task6-r2`
(sha256 `bb19aaf3b4f42a5f65d4a1a16153e597d795ca6a7d00b4f5f48443aac50606e6`). Capturas em
`visual/task6-r2/native/` e folhas em `visual/task6-r2/side/`, todas desse binário. Janela de
prova no DP-1 (antigo DP-3), ws9, que ficou no ws9 no fim; HDMI-A-1 e eDP-1 intocados.

- **BLOCKER 1** (aviso e "Ler do terminal" fora do popover): texto em `flex_1().min_w_0()` e
  botão `flex_shrink_0()` na linha do aviso e na de erro (`controls.rs`). Mesmo defeito
  corrigido nos irmãos achados por busca (`flex_1()` de texto sem `min_w_0()`): pergunta do
  agente, plano do Codex, confirmação, erro de comandos, erro da lista, aviso de envio
  (`app.rs`) e plano sem terminal (`controls.rs`). Provas: `r2-04-mode-picker.png` (Claude,
  modos não lidos) e `r2-11-codex-permission.png` (Codex com terminal): texto quebra dentro da
  borda, botão à direita.
- **BLOCKER 2** (linhas dos seletores): a atual deixa de ser desabilitada, ganha fundo accent e
  tique à direita, e clicar nela só fecha; detalhe em segunda linha; esforço capitalizado com a
  dica "Do mais rápido ao mais inteligente." (`native_effort_hint`); padding 10×7 (6 no
  esforço). Provas: `r2-02`, `r2-03`, `r2-04` e as folhas `side/composer-*-picker.png`.
- **NOTED 1** (comandos): título "Comandos", linha com nome em mono e descrição embaixo,
  argumentos e selo da origem à direita (`r2-05-commands.png`, `side/commands.png`).
- **NOTED 3**: `merge_history` aplica a mesma regra da prévia gravada ao reinstalar o histórico
  (`drop_recorded_preview`), e o app limpa a prévia visível junto; teste novo
  `history_that_records_the_preview_drops_it` (compilado, não executado).
- **NOTED 6** (quebras no painel): custo e linhas do contexto sem quebra (truncam no limite),
  legendas curtas do web ("desde que subiu", "Estimativa API") e largura padrão do painel 320
  (web ~300 + margem); limites voltam a duas colunas (`r2-15-real-arbiter.png`,
  `side/rightbar.png`). Teste `panel_never_squeezes_the_chat` ajustado a 320.

Diferenças declaradas acrescentadas:

| Diferença | Motivo |
|---|---|
| Sem "Aplicar nesta sessão"/"Salvar como padrão" no seletor de modelo | o clique já aplica na sessão; padrão global está fora do escopo do contrato |
| Legendas longas truncam com o painel no mínimo (240 px) | o web tem a mesma legenda em uma linha; aqui o limite corta em vez de quebrar |

Rolagem suave da conversa: pedido do usuário nesta rodada, virou a Task 7 (fora desta Task).
