# Pesquisa de referências do Hangar — 13/08/2026

**Arquivo único, de propósito.** Tudo o que a pesquisa produziu está aqui: a análise, o backlog
pronto pra virar plano, e o apêndice com as decisões que já foram tomadas e não devem ser
relitigadas. Se algo desta pesquisa não estiver neste arquivo, se perdeu.

| Parte | O que tem |
|---|---|
| **1 e 2** | Orca — layout e funcionalidades, com o que já temos, o que falta e o que descartar |
| **3** | Os outros ~30 produtos da categoria, e os **padrões que se repetem em 3+** |
| **4** | Paseo item por item, com o arquivo do Hangar onde cada coisa encaixa |
| **5** | **O backlog** — 13 tarefas em 5 trilhas, com foto, arquivos que tranca e mapa de colisão |
| **6** | Apêndice: modo headless, o termo legal, detecção de CLI, e o que os apps mostraram ao vivo |

Fontes: repositório `stablyai/orca` (MIT, 44,6k estrelas), `getpaseo/paseo` (13,6k, clonado e lido),
os sites `onorca.dev` e `paseo.sh`, ~30 outros produtos varridos, e **os dois apps instalados e
rodando nesta máquina** — 29 prints do Orca e 26 do Paseo, este último com sessão de verdade.

Imagens em `.refs/` (fora do git): `orca/` e `outros/` (marketing), **`orca-live/` e
`paseo-live/desktop/` (ao vivo, com dados reais)**.

Nada aqui propõe reescrever o Hangar. O modelo do Hangar — sessão de verdade rodando em tmux, na sua
máquina, dirigida pelo celular — continua sendo a base. O que segue são ideias de **layout** e de
**funcionalidade** que cabem dentro desse modelo.

## O que o Orca é, tecnicamente

| | |
|---|---|
| Desktop | **Electron** (`electron.vite.config.ts`, `electron-builder`) + **React 19** + TypeScript |
| Interface | Tailwind + shadcn/ui (Radix) + `lucide-react`, estado em **zustand** |
| Terminal | **xterm.js** com WebGL, ligaduras, busca, unicode11 + `node-pty` |
| Editor / diff | **Monaco** (o editor do VS Code) + `@sanity/diff-match-patch` |
| Celular | **React Native / Expo** (`mobile/`, metro + fastlane), com módulos nativos em Swift e Kotlin |
| Remoto | `ssh2` (worktrees em máquina remota), `ws`, `qrcode` (pareamento) |
| Também | tiptap (editor rico), mermaid, katex, pdfjs, `agent-browser` (o mesmo CLI que eu uso aqui) |

Distribuição: dmg, exe (NSIS), AppImage, Homebrew cask, AUR. Empresa com Y Combinator por trás,
telemetria via posthog, release quase diária (versão vista: 1.4.178-rc.2).

O Hangar é PWA Svelte + FastAPI + tmux. **Isso não é desvantagem** — é outra aposta: o Orca precisa
instalar um app de 200 MB em cada máquina, o Hangar abre no navegador do celular e fala com uma
sessão que já existe. As ideias abaixo são de interface e de comportamento, não de stack.

## Vocabulário deles (importa, porque organiza a tela)

Hierarquia: **Host** (local / SSH / servidor Orca remoto / VM) → **Project** (um repo, ou um grupo de
repos) → **Workspace** = **Worktree** → **Pane** → **Tab** → **Agent session**.

Estados de agente, com um glifo fixo cada um: **Working** (spinner) · **Needs You** (interrogação
âmbar) · **Done** (check verde) · **Idle** (ponto cinza) · **blocked/failed** (ponto vermelho) ·
**concluído-mas-não-lido** (só nas abas de terminal). O Hangar tem `working` / `idle` /
`awaiting_input` / `dead` — quase o mesmo vocabulário, mas sem o "pronto e você ainda não viu".

---

# Parte 1 — Layout

## 1.1 A linha da lista de sessões (`02-barra-lateral-sessoes.png`)

É a peça mais copiável do produto inteiro. Cada linha da barra lateral tem **três andares**:

```
● nome-do-workspace                                    ⇄ 📄
  [orca] Jinwoo-H/nome-da-branch
  ◔ 🤖 "última frase do agente, cortada…"           1h
```

1. ponto de estado + nome (negrito quando **não lido** — eles não usam badge de contagem, usam peso
   da fonte);
2. **chip colorido do repositório** + a ref da branch, com ícones de PR / arquivo alterado à direita;
3. **prévia da última mensagem** com avatar do agente e **idade relativa** (`3m`, `1h`).

O Hangar hoje mostra nome + estado; a última mensagem só aparece no `HoverPreview` (passando o mouse,
desktop) e nos cards do Board/Canvas. Trazer a prévia + tempo para a linha da barra lateral e da lista
do celular é a mudança de maior efeito por menor esforço deste documento — resolve "o que essa sessão
está fazendo?" sem abrir nada.

Dois detalhes do mesmo print: sessões **aninhadas** aparecem com um recuo e um botão `⌥ 2 children ⌄`
(no caso deles, worktrees filhos; no Hangar o análogo direto são os **pares 🤝** e as sessões do mesmo
grupo, que hoje viram cluster mas sem contador colapsável do lado). E o item selecionado é uma
superfície arredondada por trás da linha inteira, não uma barra na lateral.

## 1.2 Agrupamento por estado, no celular (`03-celular-lista-agrupada.png`)

A lista do celular é **agrupada por situação, com cabeçalho colapsável e contagem**:

```
⌄ PINNED 1
> DONE 23
⌄ IN REVIEW 3
⌄ IN PROGRESS 18
```

Barra de ferramentas no topo: `Filter (2)` (com o número de filtros ligados), `Smart`, `PR`, conta, `+`,
busca. O contador dentro do botão de filtro é o detalhe que evita o pior bug de usabilidade de lista
filtrada — sumiu uma sessão e você não sabe por quê.

No Hangar, o Board já agrupa por estado em três colunas (Precisa de você / Trabalhando / Pronto), mas
**isso é desktop**. No celular a lista é plana e ordenada. Portar o agrupamento colapsável com contagem
para `SessionList.svelte` daria ao celular a mesma leitura do quadro sem precisar de colunas.

## 1.3 A tela de conversa no celular (`04-celular-chat-e-teclas.png`)

Três coisas:

- **Cabeçalho de duas linhas**: título = a branch (`feat/mobile-page`), subtítulo = o estado em
  palavras — `● 2 terminals · claude active`. O Hangar põe o nome da sessão e um chip de estado; a
  frase curta lê melhor.
- **Abas dentro da sessão**: `claude | shell | PLAN.md | +`. Uma aba por painel: o agente, um shell, e
  **um arquivo aberto**. No Hangar isso hoje são coisas separadas (terminal no rodapé do desktop,
  espelho no celular, plano no `PlanPanel`, arquivos em lugar nenhum).
- **A fileira de teclas acima do teclado**: `📋 Paste · Esc · Tab · ⌫ · ↑ · ↓ · ← · → · Ctrl+C`. O
  `TerminalMirror` do Hangar já tem teclas de navegação; o que falta é elas estarem **ao lado do
  composer normal**, não só no espelho do terminal — é o que permite mandar Esc ou Ctrl+C sem trocar
  de modo.

Além disso: mic de ditado no próprio composer (o Hangar já tem, com VAD e mãos-livres — aqui estamos
na frente) e envio de imagem virando miniatura removível.

## 1.4 Configurações: painel de assuntos à esquerda (`09-tela-de-config.png`)

A tela de configurações do Orca ocupa a janela inteira, com **navegação de assuntos numa coluna
esquerda** (General, Git, Appearance, Terminal, Notifications, Shortcuts, Stats & Usage, SSH) e, embaixo,
uma seção **Repositories** com um item por repositório. Tudo buscável por `Cmd-,` + palavra.

O `CLAUDE.md` do Hangar já registra a direção acordada e não feita: juntar as configurações num modal
único com abas. Esta imagem é o desenho concreto disso, com dois acréscimos que valem: **busca dentro
das configurações** e **uma seção por repositório** (cor do chip, nome de exibição, base padrão) —
que é exatamente o chip colorido de repo que aparece na lista de sessões.

## 1.5 Densidade e cor de estado

Regra explícita deles: **só duas cores no quadro** — âmbar para "precisa de você", verde para
"pronto", o resto neutro. "Tinta quer dizer olhe aqui". O Board do Hangar hoje colore as três colunas;
vale testar apagar a cor da coluna "Trabalhando".

---

# Parte 2 — Funcionalidades

## 2.1 O que o Hangar já tem e o Orca não (ou tem pior)

Vale registrar, porque muda a leitura da lista abaixo:

- **Dashboard de custo de verdade** (`screens/Costs.svelte`) — o Orca só mostra uso contra o plano e
  um custo estimado no popover; não tem tela de custo com filtro por projeto/modelo/máquina.
- **Ditado com detecção de voz e mãos-livres**, com limpeza do texto por LLM e "↩ original".
- **Perguntar ao histórico** (busca lexical + `claude -p` responde onde você falou sobre X).
- **Pareamento entre sessões** com contrato compartilhado — o Orca tem orquestração (coordenador →
  workers), que é hierárquica; o pareamento simétrico do Hangar não tem equivalente lá.
- **Motores de modelo** (rodar a sessão em outro provedor mantendo `~/.claude`).
- **TTS** com seleção de trecho.
- Suporte a **Pi e Kimi** com leitura estruturada, não raspagem de tela.

## 2.2 Alto valor, cabe no modelo do Hangar

### (a) "Pronto e não lido" como quarto estado
Hoje uma sessão que terminou volta para `idle` e some no meio das outras. O Orca separa **Done** de
**Idle** (ocioso há mais de ~30 min) e marca **não lido em negrito**. No Hangar isso é backend
(`state.py` já sabe a transição working→idle; falta guardar "você viu?") + peso de fonte na lista.
Resolve o problema real de "terminou faz 3 horas e eu não percebi".

### (b) Feed de atividade — uma tela cronológica de tudo (`docs/activity`)
Uma lista única, no estilo Slack, com: agente terminou o turno, agente bloqueado numa pergunta,
sessão nova criada, agente esperando input tempo demais — cada entrada com **prévia curta da última
resposta** e badge de não-lidas. Clicar salta para a sessão.

O Hangar tem `WorkspaceAttentionStrip` / `AttentionFeed`, que é a metade "precisa de você". Falta a
metade "o que aconteceu enquanto eu estava fora". No celular isso é mais útil que no desktop.

### (c) Comentar linha de diff e mandar em lote para o agente (`05-comentario-no-diff.png`)
Passa o mouse na gutter, aparece `+`, escreve uma nota em markdown ancorada naquela linha, e um botão
**Send to agent** manda **todas as notas de uma vez**. A justificativa está escrita na doc deles e é
boa: mandar um comentário por vez faz o agente balançar; em lote é uma rodada de pensamento e uma
revisão. As notas **seguem a linha** quando o diff se desloca, e continuam visíveis depois do conserto
para você conferir (`Resolve` colapsa).

O Hangar tem `DiffView` (unificado) e o split só no `EditDiff` do chat. Esta é a funcionalidade que
mais muda o dia a dia de quem revisa código de agente pelo celular — e o backend já tem diff por
arquivo e envio de texto para a sessão. O que falta é a âncora de linha e a caixa de notas.

### (d) Automações agendadas (`docs/cli/automations`)
Rodar um prompt em agenda (`daily`, `weekdays`, cron, RRULE, com fuso), com **histórico de execuções**,
botão **Rerun** e três detalhes que valem copiar: criar **desabilitado** por padrão; **precheck** (um
comando shell barato — saída não-zero registra a execução como *pulada* em vez de rodar); e
**`--missed-run-grace-minutes`** para o caso da máquina ter ficado desligada.

O Hangar tem o *loop runner*, que é a versão reativa (terminou → verifica → re-prompta). Automação
agendada é o eixo que falta, e reusa quase tudo: sidecar, guardrails, kill-switch `automations_enabled`.

### (e) Trocar de conta a quente (`docs/agents/codex-hot-swap`)
Chip na barra de status abre a lista de contas com **rótulo amigável** ("pessoal", "trabalho"), uso e
limite de cada uma, e uma linha explícita **"System default"**. Trocar não exige re-login.

O Hangar já tem múltiplos `CLAUDE_CONFIG_DIR` (`listClaudeConfigs`/`criarConta`), mas isso vive
escondido dentro de criar sessão. Subir para um chip ao lado do ⚡5h/📅7d — junto com o uso de cada
conta — é reaproveitamento quase puro.

### (f) Painel de uso ordenado pelo limite mais apertado
O popover de uso lista `ícone · nome · plano · reset mais próximo · barras por janela`, **ordenado com
o limite mais apertado primeiro**, com modo Detailed/Compact e um **chip de aviso ao cruzar 80%**. O
`UsageSheet` do Hangar mostra as janelas, mas não ordena por aperto nem avisa em 80%.

### (g) Comentário de estado no card (`docs/cli/worktree-checkpoints`)
Cada workspace carrega um **campo de texto livre visível na lista**, que o próprio agente atualiza por
CLI: `"reproduzi a falha de auth; testando o conserto da cadeia de credencial"`. Mais um **status do
card** (`todo`, `in-progress`, `in-review`, `completed`) setável pelo agente.

Para o Hangar isto é barato: um comando `cp-send`-like que escreve num sidecar, e o texto aparece na
linha da sessão. É a resposta para "o que essa sessão está fazendo agora" quando a última mensagem do
transcript não diz.

### (h) Paleta de salto com dígitos (`docs/model/quick-open`)
A paleta abre, com busca vazia, mostrando até 6 sessões **ordenadas por atenção** (needs-you, depois
done, depois idle), com atalhos `Cmd-1…Cmd-6`. O detalhe fino: **a ordem congela quando a paleta
abre**, para a linha não se mexer sob o cursor. O `WorkspaceCommandPalette` do Hangar já existe;
faltam os dígitos, a ordem por atenção e o congelamento.

## 2.3 Vale, mas é obra maior

- **Painéis divididos por sessão** (`08-terminais-divididos.png`): árvore de painéis por workspace,
  salva entre reinícios, com barra colorida na aba ativa. O Hangar tem split view de N chats no
  desktop; o que falta é a árvore persistida com terminal e arquivo dentro da mesma sessão.
- **Editor de arquivo no app**: hoje o Hangar lê arquivos (plano, contrato, README) mas não abre um
  arquivo qualquer do repositório para editar. É a aba `PLAN.md` do print do celular.
- **Painel de PR / checks** (`10-board-github-linear.png`): o Hangar não tem nada de forge — nem
  `gh`, nem PR, nem checks. Isso é uma área nova inteira. O pedaço barato e útil é só o **chip
  vermelho quando um check falha**, com a ação "manda o log do check falho para o agente".
- **Modo de desenho** (`07-design-mode-anotacao.png`): clicar num elemento da página aberta no
  navegador e mandar HTML, CSS e recorte de tela para o prompt. O Hangar tem `PreviewSheet` (iframe do
  dev-server) — mas iframe não deixa inspecionar o que está dentro se for outra origem. Ficaria mais
  fácil pelo `agent-browser`, que já está instalado nesta máquina.

## 2.4 Descartar (não combina com o Hangar)

- **Worktree por tarefa como modelo central.** É a aposta do Orca inteiro e é incompatível com "dirigir
  uma sessão tmux que já existe". O `orchestrating-idea-to-push` do Hangar já trata worktree paralela
  como exceção declarada, e essa decisão está certa.
- **Daemon próprio dono dos PTYs.** O Orca precisa disso porque o app pode fechar; o tmux já faz isso
  para o Hangar, de graça, há 20 anos.
- **`--dangerously-skip-permissions` por padrão em todo agente.** É o padrão de lançamento deles
  ("o worktree é o sandbox"). Sem worktree isolada, no Hangar isso seria dar permissão total numa
  árvore de trabalho de verdade. Fica registrado como coisa que **não** se copia.

---

# Parte 3 — Os outros produtos da mesma categoria

Varredura de ~30 produtos. Imagens em `.refs/outros/`.

## 3.1 Mudanças de estado (ler antes de usar qualquer um como referência)

| Produto | Situação em 13/08/2026 |
|---|---|
| **Crystal** (stravu) | **Arquivado em fev/2026.** Virou **Nimbalyst** (MIT, ativo, Electron + React + Monaco, com app iOS em SwiftUI) |
| **Vibe Kanban** (Bloop) | **Em encerramento** (anúncio 10/04/2026), vira mantido pela comunidade. A documentação de interface é a melhor da varredura — vale ler antes de sumir |
| **Omnara** | **Pivotou.** O repo virou plataforma de agentes; o app que pilota Claude Code pelo celular agora é `remote.omnara.com` e é **fechado** |
| **Terragon Labs** | **Encerrado** (16/01/2026), código liberado em Apache-2.0 |
| **Roo Code** | **Arquivado** em 15/05/2026 — o time declarou que não acredita mais em IDE |
| **Uzi** | Parado desde jun/2025 |
| **Sketch** | Descontinuado (22/07/2026) |
| **Async** (async.build) | Pré-lançamento; site é manifesto + lista de espera, nada público |

Sobrevivem e vale acompanhar: Orca, Conductor, Sculptor, Emdash, mux, Kanban Code, Garcon,
OpenChamber, T3 Code, agent-of-empires, Happy, Nimbalyst, Cline, opencode, Crush.

## 3.2 Quem faz como o Hangar (tmux + máquina própria)

Só **quatro** produtos usam tmux, e nenhum o usa como isolamento — usam como **persistência**, o mesmo
argumento do README daqui:

- **Kanban Code** (langwatch) — macOS SwiftUI + Windows em Tauri. **tmux + worktree**, kanban de
  6 colunas (Backlog → In Progress → **Waiting** → **In Review** → Done → All Sessions), busca BM25 no
  histórico, push por Pushover, adaptadores por integração (o mesmo desenho do `adapters/` daqui).
- **agent-of-empires** — Rust + axum + React, **uma sessão tmux por agente** e PWA. A decisão de
  interface deles é a tese do Hangar validada por outro time: no celular o padrão é **visão
  estruturada** (plano, cards de tool-call, aprovar por gesto) e **o terminal cru é um toggle**.
- **Claude Squad** — TUI em Go, tmux + worktree.
- **Uzi** — parado, mas tinha a ideia boa de **porta por agente** (`$PORT` numa faixa) para dev
  servers não colidirem.

Os outros ~20 usam **git worktree** como primitiva de isolamento. É o consenso da categoria. Não é
motivo para mudar de rumo — é motivo para saber que a colisão de porta e de banco entre sessões no
mesmo diretório é um problema que eles não têm e o Hangar tem.

## 3.3 O concorrente mais direto: Garcon (`cfal/garcon`)

Bun + SPA self-hosted, roda na máquina do código, sem worktree e sem container, usando os CLIs que já
estão instalados. Suporta Claude Code, Codex, Cursor, OpenCode, Amp, Droid **e Pi**. É a mesma aposta
do Hangar, feita por outra pessoa.

O que ele tem e o Hangar não:

- **Até 4 sessões vivas em painéis redimensionáveis**, com marca de ativo / não-lido / esperando.
- **Aprovação de tool call no celular** com Allow/Deny e o detalhe da chamada
  (`garcon-celular-permissao.png`) — e uma **barra de abas no rodapé: Menu · Chat · Git · PRs ·
  Files · Terminal**. Essa barra é a resposta mais simples que vi para "como caber Git, terminal e
  arquivos no celular sem virar menu escondido".
- **Staging por linha, hunk, arquivo ou pasta** no diff.
- **Transcript compartilhável e revogável.**
- **Prompts agendados** e notificação por Telegram.

## 3.4 Padrões que se repetem em 3 ou mais produtos

Ordenados por quantos produtos independentes convergem. Esta é a parte que separa moda de consenso.

**1. Comentário ancorado em linha do diff que vira prompt — 7 produtos.**
Conductor, Claude Code na web, Cursor, Orca, Vibe Kanban, opencode, Factory, Cline. **Ninguém trata
revisão como canal separado: revisão é composição de prompt.** É a única funcionalidade de review em
que todos convergiram, e o Hangar não tem. Já estava na Parte 2 como item (c); a varredura confirma
que é a lacuna número um.

**2. Lista agrupada por estado, com "precisa de você" como estado próprio — 7 produtos.**
Conductor (backlog · in progress · **in review** · done), Vibe Kanban (**Needs Attention / Idle /
Running**), Kanban Code (6 colunas), Devin, Cursor, Garcon, Omnara ("indicador de prompt pendente na
lista, para achar agente travado sem abrir cada conversa"). **O eixo que falta no Board daqui é
"em revisão / pronto"** — é ele que fecha o ciclo.

**3. Uma sessão = uma branch = um PR, e ninguém faz merge sozinho.**
O ponto final é sempre abrir o PR e arquivar. O Conductor vai além e **bloqueia o merge** enquanto
houver tarefa aberta ou check falhando.

**4. "Objetivo até terminar" com faixa acima do composer — 3 produtos, e é o `loop.py` daqui.**
OpenChamber (**Session Goals**) e Conductor (**goal bar**) fazem o mesmo que o loop runner, com dois
detalhes que faltam aqui: um **modelo leve audita o resultado depois de cada turno** (o OpenChamber só
trava depois de 3 vereditos "travado" seguidos), e **orçamento de token é estado de parada de
primeira classe**, ao lado de max_iters.

**5. Terminal embutido existe em todos — e no celular o padrão é escondê-lo.**
Os dois produtos que mais pensaram em celular (agent-of-empires e Happy) chegaram na mesma conclusão
independentemente: **no telefone o terminal cru é opt-in**, o padrão é a visão estruturada.

**6. Preview do app rodando dentro do painel, com clicar-no-elemento mandando contexto — 5 produtos.**
Vibe Kanban (inspetor que identifica o componente **sem instalar pacote**, com modos Desktop /
Mobile 390×844 com moldura de celular / Responsivo arrastável), OpenChamber (**detecta o dev server
lendo a linha `Local:` do terminal**), Orca, Cursor, Emdash. **URL pública de preview: ninguém
oferece.**

**7. Estado da sessão em duas camadas — 5 produtos.**
Badge condensado na lista, painel rico dentro. A lição é **o que** condensar: Claude Code na web põe
`+42 −18` na linha; o Vibe Kanban põe `📄 4 +134 −2`; o Hangar põe só o estado do processo.

**8. Aprovação de tool call por categoria, com memória de sessão — 5 produtos.**
Happy (pausa a execução no CLI, Allow/Deny com o detalhe exato, **"lembrar nesta sessão"**), T3 Code
(4 modos mapeados por provedor), Roo, Crush, Cline, agent-of-empires (**aprovar por gesto**).

**9. Pareamento remoto por QR de uso único, e túnel em vez de porta aberta — 5 produtos.**
OpenChamber (QR que expira se não usado, token por dispositivo, lista com **revogar**), T3 Code,
agent-of-empires (QR + frase secreta sobre Tailscale), Happy (QR carregando a chave; o servidor guarda
só o hash da pública), Vibe Kanban. O Hangar já tem QR e token; **falta revogação por dispositivo**.

**10. Um único fluxo agregado, com o servidor como fronteira de execução — 4 produtos.**
O T3 Code escreve a regra: *"todo processo de provedor, terminal, operação de git e leitura de arquivo
acontece no servidor, nunca no cliente"*, com autorização **por método**, não por conexão. Valida a
decisão do `openSessionsStream` por servidor.

**11. Checkpoint / voltar atrás existe, e três usam git escondido.**
T3 Code emoldura **cada turno com dois checkpoints gravados como refs git ocultos** — daí sai o diff
exato do turno e o desfazer do workspace **e** da conversa. Cline e Roo usam *shadow git* (repositório
git separado, o seu fica intocado, pega até arquivo não rastreado), e o **Restore** tem três opções:
só arquivos, só conversa, ou os dois. O Hangar não tem nada disso.

## 3.5 Cinco coisas concretas que eu traria da varredura

1. **Comentário em linha do diff virando anexo do composer** — 7 produtos, é o consenso.
2. **Botão "lista plana ↔ acordeão por estado" na própria lista** (Vibe Kanban,
   `vibekanban-sidebar-acordeao.png`) — resolve na mesma tela a tensão entre `SessionList` e `Board`,
   e ataca de lado a dívida de unificar as duas views.
3. **Sessão em execução fica esmaecida e só-leitura** (Devin) — o retorno de estado mais barato da
   varredura inteira.
4. **Mensagem na fila editável antes de sair** (Devin) — o `pqueue.py` hoje só cancela ou envia.
5. **O diff escolher sozinho entre lado-a-lado e empilhado pela largura** (`diff_style: auto`, do
   opencode) — é exatamente o problema celular↔desktop que aqui está resolvido em dois arquivos
   separados.

**Não achado, apesar de procurar:** (a) **nenhum produto compara N tentativas concorrentes e ajuda a
escolher** o melhor resultado — listam o tamanho do diff e a escolha é manual; (b) nenhum oferece URL
pública de preview do app rodando. Se algum dia sobrar fôlego, essas duas são espaço vazio de
verdade.

---

# Parte 4 — Paseo, item por item, com o lugar dele no Hangar

O Paseo (`getpaseo/paseo`) é o parente mais próximo: daemon dono da execução, clientes burros, celular
e web do mesmo código. Esta parte não lista o que eles têm — lista **onde cada coisa encaixaria aqui**,
com o arquivo. Uma advertência de método: a documentação pública deles cobre pouco; **Histórico,
composer, subagentes e o painel de git não têm página nenhuma** — o que segue veio do código.

## 4.1 A linha da lista, e por que o `+188 −133` não precisa custar caro

Hoje `SessionCard.svelte:286` preenche a terceira linha só com `question` (esperando) ou `label` (o
texto do spinner). **Sessão parada: linha vazia.** É onde a última mensagem entra.

O `SessionInfo` (`lib/types.ts`) já carrega branch, pergunta, opções, travada, limitada, par, plano,
motor e loop. Falta: última mensagem, não-lido e o diff. O `git_summary` já roda por sessão a cada
varredura com cache de 3s (`git_ops.py:101`) — o `+N −M` seria **um `git diff --shortstat` a mais por
sessão por varredura**.

A resposta do Paseo para esse custo é boa: **um menu "Display preferences"** onde quem usa liga e
desliga cada coluna (Host, Pull request, Checks, Services, Diff stats, Last activity) e escolhe o
agrupamento (Project | Status) e o título (Title | Branch). Ou seja: **o diff é opcional, e quem
liga paga**. O Hangar já tem metade disso — o `cp_group_by` da `Sidebar.svelte:130` guarda
`none | server | project`; falta `status` como quarto modo e falta expor os liga-desliga.

Detalhe do slot da direita: é **um** de três coisas, nunca as três — estatística de diff, tempo
relativo, ou nada.

## 4.2 Cinco estados, não três

O Paseo tem, nesta ordem exata (e em inglês chumbado, fora da tradução):

```
Needs input → Failed → Ready to review → Working → Done
```

O Board daqui (`Board.svelte:41`) tem três: *Precisa de você · Trabalhando · Pronto*. Faltam dois, e
os dois já têm matéria-prima no `SessionInfo`:

- **Falhou** — o `stalled` (travada há mais de `CP_STALL_SECONDS`) e o `limited` (bateu limite de uso)
  hoje só tingem a linha. Viram uma faixa própria.
- **Pronto para revisar** — "terminou e você ainda não viu". É o buraco do não-lido: hoje a terceira
  coluna se chama "Pronto" mas é `idle`, então quem terminou há 10 segundos e quem terminou há 3 horas
  caem no mesmo lugar.

E **"Fixado" não é estado** — é uma seção separada acima dos grupos, com "Mostrar mais"/"Mostrar menos".

Duas ordenações diferentes, de propósito, e o código deles diz que é escolha: na lista plana por
estado, *precisa de você* vem antes de *trabalhando*; na **linha colapsada do projeto**, *trabalhando*
vem antes — para um projeto ocupado manter o indicador girando. O `sortSessions` de `lib/format.ts:112`
já põe `awaiting_input` primeiro; o `byRecency` do Board ordena por atividade.

## 4.3 Agendamento: o `loop.py` é metade do caminho

O Paseo separa duas coisas no mesmo motor de cron, e a separação é o insight:

- **Schedule** — no horário, roda este prompt neste repo: **cria uma sessão nova a cada disparo**.
- **Heartbeat** — no horário, manda um prompt **de volta para uma sessão que já existe**, para ela
  reavaliar e continuar.

**O loop runner daqui é o heartbeat, sem relógio.** O `TickCtx` (`loop.py:80`) já tem `deliver`,
`enqueue`, `notify`, `branch`, `last_assistant` e o `automations_enabled` como chave-geral. O que falta
é o disparo por tempo — o `schedule_tick` de hoje é antirrebote do disparo reativo, não relógio.

Três decisões deles que eu copiaria inteiras:

1. **Máquina desligada não recupera disparo perdido.** Na subida, o serviço avança o próximo horário
   em laço até cair no futuro; as ocorrências perdidas são **puladas**, nunca executadas em lote. E um
   run que estava rodando vira `failed` com o texto `"Daemon restarted before the scheduled run
   completed"`. É o oposto do Orca, que tem tolerância de atraso — e para o Hangar (máquina pessoal
   que dorme) o comportamento do Paseo é claramente o certo.
2. **A cadência tem atalhos nomeados** — *A cada minuto · A cada hora · Diário 9h · Dias úteis 9h ·
   Segundas 9h · Cron personalizado* — e a humanização do cron **devolve a expressão crua quando não
   reconhece o padrão**, em vez de inventar uma descrição errada.
3. **A linha meta lê identidade → passado → futuro**: `cadência · Criado há 3d · Última há 2h ·
   Próxima em 12m`. O comentário no código deles: *"o estado vive no selo, nunca se repete aqui"*. E
   **"Próxima" só aparece quando está ativo** — agendamento pausado não mente sobre o futuro.

O `max_runs` deles tem placeholder **"Ilimitado"**, e o prompt tem placeholder **"O que o agente deve
fazer a cada execução?"**.

## 4.4 Histórico: o Arquivo daqui já é isso, e em busca está na frente

O `screens/Archive.svelte` navega pasta → conversa → leitor. O Paseo agrupa **por data**, nunca por
repositório: *Recentes · Hoje · Ontem · Esta semana · Este mês · Mais antigos*; sob busca a lista fica
**plana**.

E a busca deles é **mais fraca** que a daqui: casa só workspace, título, branch e projeto —
**não busca conteúdo de mensagem**. O Hangar tem busca literal cross-servidor (`rg`) e o "perguntar ao
histórico". Nesse ponto não há o que copiar.

O que vale copiar são três textos:

1. **Três vazios diferentes, um por causa.** Buscando → *"Nenhuma sessão corresponde"*; sem filtro →
   *"Nenhuma sessão ainda"*; servidor filtrado → *"Nenhuma sessão neste servidor"*. O comentário deles:
   *"uma lista vazia significa outra coisa quando há uma busca estreitando ela"*.
2. **Faixa de servidor que falhou**: `{servidor}: não foi possível carregar o histórico`. O motivo está
   escrito no código: sem ela a lista subnotifica em silêncio e *"'nenhuma sessão corresponde' vira uma
   afirmação que o app não tem base para fazer"*.
3. Quando o ranking trunca, o rodapé troca "Carregar mais" por **"Resultados demais — estreite a
   busca"**, em vez de oferecer paginar um resultado que já não é confiável.

## 4.5 A política de notificação — o melhor achado de toda a varredura

O Hangar tem push com VAPID, silenciar por sessão e horário silencioso (`lib/push.ts`, `sw.ts`). O que
não tem é **presença**. O `computeNotificationPlan` deles decide assim:

1. Se algum cliente está **presente** (atividade nos últimos **180s**), **visível** e **focado
   exatamente naquela sessão** → **nenhum aviso**, nem na tela nem push. Você já está olhando.
2. Senão, se há cliente presente → **só o mais recente** recebe o aviso na tela, e **ninguém** recebe
   push.
3. Só quando **ninguém** está presente é que sai push.

E mais: **erro nunca vira push** — só aviso na tela.

Isso é portável direto pro `push.py`: o backend já sabe quem tem SSE aberto e em qual sessão. Resolve o
incômodo real de receber notificação no celular da sessão que você está lendo no desktop.

O corpo da notificação deles: prévia do último texto do assistente, sem markdown, espaços
normalizados, cortado em **220 caracteres**; títulos *"Agente precisa de permissão"*, *"Agente precisa
de atenção"*, *"Agente terminou"*, com textos de reserva quando não há prévia.

## 4.6 Composer: modo de permissão vem do provedor

O `provider-manifest.ts` deles não tem uma lista fixa de modos — **cada provedor declara os seus**, com
rótulo, descrição, ícone, um **nível de risco** (`planning | safe | moderate | dangerous`) e a marca
`isUnattended`:

| Provedor | Modos | Padrão |
|---|---|---|
| Claude | Plano · Sempre perguntar · Aceitar edições · Automático · **Bypass** (sem supervisão) | automático |
| Codex | Padrão · Auto-revisão · **Acesso total** (sem supervisão) | auto-revisão |
| OpenCode | Build · Plano | nenhum |
| Pi | nenhum | — |

Três caminhos para trocar: **Shift+Tab cicla**, botão de escudo no composer, e o grupo "Modo" na paleta.
A cor e o aviso saem do **dado** (`colorTier`), não de um `if` espalhado pela tela.

No Hangar isso encaixa como uma terceira pílula ao lado de modelo e esforço no `Composer.svelte`. Hoje
só existe o `OptionButtons.svelte:21`, que **detecta** o pedido de permissão quando ele aparece — não
há como escolher o modo antes.

**`@files`, `/commands` e `/skills` são um autocomplete só**, e a regra é elegante: `/` no **começo**
lista tudo; `/` no **meio** da mensagem filtra só para as skills. O `@` insere o caminho **entre aspas,
com escape** — nome com espaço não quebra o prompt. O `SlashSuggest` daqui já faz a metade dos
comandos; `@arquivo` não existe.

## 4.7 Subagentes: a faixa acima do composer

A string é montada assim, e vale copiar o comportamento: `"19 subagents · 1 running"` — e **a parte
"rodando" simplesmente não aparece quando é zero**. Conta os dois tipos juntos: subagentes gerenciados
por eles e as execuções-filhas nativas do provedor.

Nasce **colapsada**; expandida, a lista é rolável e limitada a 200px. E a distinção que importa: clicar
num subagente **gerenciado** abre uma sessão completa; clicar num **nativo do provedor** abre uma linha
do tempo **só-leitura, sem composer** — porque o ciclo de vida dele pertence ao provedor.

O rótulo da linha usa a **descrição** da tarefa, não o título, *"porque a tarefa é o que distingue
irmãos num fan-out"*. E um título literal `"new agent"` é tratado como **ausência** de título, para não
virar dezenove linhas idênticas.

O `ActivitySheet.svelte:98` já deriva `runningAgents`. A diferença é onde mora: aqui é um botão na
NavBar com badge; lá é uma faixa permanente acima do composer.

## 4.8 Git: 25 frases explicando por que o botão está desligado

Este é o item mais barato e mais valioso da Parte 4. O Paseo tem cerca de **25 textos** que explicam,
em frase inteira, por que uma ação está indisponível:

> *"Pull não está disponível enquanto há mudanças locais — commite ou guarde no stash primeiro"*
> *"Push ainda não está disponível porque há mudanças novas para trazer antes"*
> *"Criar PR não está disponível porque esta branch ainda não tem commits novos"*
> *"Arquivar não está disponível aqui porque este workspace não foi criado como worktree do Paseo"*

**Nenhum botão cinza mudo. Todo bloqueio se explica e diz a saída.** O `GitStatusBar` daqui mostra erro
depois que a operação falha; isso é o contrário — explica antes.

Duas outras coisas do painel deles: **não há campo de mensagem de commit no cliente** (quem escreve é um
modelo pequeno no servidor, com estilo configurável por repositório — placeholder *"Use Conventional
Commits com escopo"*), e a aba do PR **é rotulada com o número do PR**, não com a palavra "PR".

Sobre o comentário em linha de diff, uma correção ao que eu disse antes: no Paseo esses comentários são
**rascunhos locais que viram anexo do prompt** ("Revisão · 3 comentários") — **não** existe caminho que
os poste como review no GitHub. É revisão *para o agente*, não para o PR. Que é exatamente a versão que
cabe aqui.

## 4.9 O que NÃO copiar do Paseo

- **Fechar o app desktop mata o daemon, por padrão.** A documentação deles diz textualmente que
  *"sair do app desktop para o daemon que ele iniciou, então 'reinicie o app' é um conserto de
  verdade"*. Há um interruptor "manter o daemon rodando ao sair", desligado por padrão. Aqui esse
  problema não existe: o backend é systemd e o tmux é dono dos processos.
- **Automação de navegador só no desktop** — o daemon não roda navegador, ele encaminha para o app
  desktop conectado e devolve erro quando não há nenhum.
- **Estados de sessão em inglês chumbado, fora da tradução.** Erro deles, não modelo.

## 4.10 Ordem que eu seguiria

1. **Última mensagem + tempo na terceira linha** do card e da barra lateral — o buraco já existe.
2. **Não-lido** ("pronto para revisar") como quarto estado, e "falhou" a partir de `stalled`/`limited`.
3. **Presença antes do push** (`push.py`) — não notificar a sessão que você está lendo.
4. **Frases de indisponibilidade** no Git, no lugar de botão cinza mudo.
5. **Agendamento** reusando o `TickCtx`, com o "pulou, não acumula" do Paseo.
6. **Modo de permissão** como pílula no composer.

Os quatro primeiros são de front e texto. O quinto é backend e mexe no `loop.py`. O sexto depende de
como cada provedor expõe os modos, e é o único que não dá para estimar sem medir.


---

## 4.11 Onde as duas referências discordam — e o que escolher

Orca e Paseo resolvem a mesma coisa de formas diferentes em doze pontos. **Quando as referências
colidem, não se mescla por padrão — escolhe-se.** A coluna da direita é a escolha, com o motivo.
Onde as duas ideias somam em vez de brigar, está escrito "mescla".

| # | Assunto | Orca | Paseo | Escolha |
|---|---|---|---|---|
| 1 | **Permissão ao lançar** | tudo com `--dangerously-skip-permissions` por padrão ("o worktree é o sandbox"), e a caixa vem marcada no onboarding | modos por provedor, com nível de risco e padrão seguro (`auto` / `auto-review`) | **Paseo, sem discussão.** Sem worktree isolada, o padrão do Orca é permissão total numa árvore de trabalho real |
| 2 | **Nomes dos estados** | Working · Needs You · Done · Idle · blocked/failed, com Idle escondido por padrão | Needs input · Failed · Ready to review · Working · Done | **Paseo.** "Ready to review" nomeia melhor o não-lido, e "Failed" é estado, não cor |
| 3 | **Agrupar a lista** | por projeto, com filtro separado | deixa escolher: Projeto \| Status | **Paseo** — e some o menu de preferências junto (é a tarefa A4) |
| 4 | **Comentário no diff** | vira lote pro agente **e** as threads do GitHub aparecem dentro do app | rascunho local que vira anexo do prompt; **não posta no forge** | **Paseo.** A versão do Orca exige integração com forge, que o Hangar não tem |
| 5 | **Mensagem de commit** | campo de texto + "Generate with AI" como receita editável com variáveis | **não há campo**: um modelo pequeno no servidor escreve, você configura só o estilo por repo | **Orca.** O campo é previsível; a receita editável é o extra que vale |
| 6 | **Terminal** | central: splits infinitos, WebGL, scrollback que sobrevive ao restart | existe, mas o chat estruturado é o padrão | **mescla.** Terminal no desktop (você já tem), estruturado por padrão no celular — é o que agent-of-empires e Happy também concluíram |
| 7 | **Árvore de arquivos** | painel direito permanente, busca com abas `Names`/`Contents`, marcador `M` que sobe pela árvore | aba *Arquivos* ao lado de *Alterações* | **Orca.** A busca em duas abas e o `M` herdado são mais ricos (tarefa C1) |
| 8 | **Onde vive o uso das contas** | barra do rodapé, sempre visível, todas as contas | dentro de Configurações → Uso | **Orca.** Cota é coisa de olhar de canto de olho, não de ir procurar (tarefa C3) |
| 9 | **Histórico** | escopo Workspace/Project/All + agrupar por Project/Folder/Agent, busca em título/cwd/branch/modelo/prévia | agrupa **por data**, busca só metadados | **mescla:** agrupamento por data do Paseo + escopo do Orca. A busca do Hangar já é melhor que as duas (busca conteúdo e tem "perguntar ao histórico") |
| 10 | **Subagentes** | filhos expansíveis sob a linha do pai | faixa acima do composer — **mas medido: não apareceu com o provedor Pi**, então pode ser só do Claude | **Orca.** É o que funciona igual em todo provedor |
| 11 | **QR de pareamento** | `orca serve --mobile-pairing` imprime QR no terminal | só cria QR se o **relay** estiver ligado | **nenhum dos dois.** Os dois precisam de QR porque o endereço é imprevisível; no Hangar é LAN/VPN conhecida — o QR é do endereço + token e pronto (tarefa C4) |
| 12 | **Configuração** | tela cheia, nav de assuntos à esquerda | igual | **concordam** — e bate com a decisão de modal já registrada no `CLAUDE.md` |

Duas leituras que saem da tabela:

- **Em segurança e vocabulário, o Paseo ganha; em densidade de informação, o Orca ganha.** O Paseo é
  mais cuidadoso com o que o agente pode fazer e como as coisas se chamam; o Orca põe mais informação
  na tela sem pedir clique.
- **Onde os dois precisam de infraestrutura que você não tem** (forge para o item 4, relay para o 11),
  a escolha é a versão simples — e ela costuma ser mais barata do que as duas.

---

# Parte 5 — O backlog, pronto para virar plano

Daqui pra baixo é a lista de tarefas: cada uma com a foto de referência, os arquivos que ela toca e do
que depende — para abrir um plano por item e tocar vários em paralelo. A análise que justifica cada
uma está nas Partes 1 a 4, acima.

---

## Mapa de colisão — o que pode andar junto

A regra é simples: **duas tarefas que tocam o mesmo arquivo não vão em paralelo.** O ponto quente é o
par `Sidebar.svelte` + `SessionList.svelte` — a mesma tela escrita duas vezes, 3.601 linhas somadas.

| Trilha | Tarefas | Arquivos que ela tranca |
|---|---|---|
| **A — a linha da sessão** | A1 · A2 · A3 · A4 | `Sidebar.svelte`, `SessionList.svelte`, `SessionCard.svelte`, `lib/format.ts` |
| **B — backend** | B1 · B2 · B3 | `push.py` · `loop.py`+`api.py` · `git_ops.py` |
| **C — telas novas** | C1 · C2 · C3 · C4 | arquivos novos, quase nenhuma colisão |
| **D — texto** | D1 · D2 | strings dentro de `git/` e `Archive.svelte` |

**Podem rodar ao mesmo tempo:** a trilha A inteira (como um trabalho só) + B1 + B2 + C1 + C2 + C3 +
C4 + D1 + D2. Dá **nove frentes**, com duas amarras:

- **A3 depende de B3** (a linha só mostra o `+N −M` depois que o backend souber calcular).
- **C2 depende de C1** (o diff por arquivo abre a partir da árvore).

Dentro da trilha A, a ordem é A1 → A2 → A4 → A3. Se quiser paralelizar A, a única divisão limpa é
**uma pessoa no `Sidebar` e outra no `SessionList`** — mas aí as duas precisam combinar o markup
antes, senão as views divergem de novo (é o item do `polish-backlog.md` que sobrevive a toda varredura).

---

# Trilha A — a linha da sessão

## A1. Última mensagem e tempo na terceira linha

/home/jefferson/pessoal/hangar/.refs/orca/02-barra-lateral-sessoes.png

Três andares por linha: ponto de estado + nome · chip do repo + branch · **prévia da última mensagem
com avatar e idade relativa** (`3m`, `1h`). No celular do Orca é igual:

/home/jefferson/pessoal/hangar/.refs/orca/03-celular-lista-agrupada.png

**O buraco já existe no seu código.** `SessionCard.svelte:286` preenche a terceira linha só com
`question` (esperando) ou `label` (texto do spinner). Sessão parada: linha vazia.

- **Toca:** `SessionCard.svelte`, `Sidebar.svelte`, `SessionList.svelte`; backend precisa expor a
  última fala do assistente por sessão (o `transcript.last_assistant_text` já existe, usado pelo
  `loop.py`) e a idade — o `last_activity` já vem no `SessionInfo`.
- **Depende de:** nada.
- **Tamanho:** pequeno no front, pequeno no backend. É o melhor retorno da lista inteira.

## A2. Não-lido, e "pronto para revisar" como estado

/home/jefferson/pessoal/hangar/.refs/outros/vibekanban-sidebar-acordeao.png

Hoje **não existe conceito de não-lido em lugar nenhum** — conferido, zero ocorrência no backend e no
front. E a terceira coluna do `Board.svelte:44` se chama "Pronto" mas é `idle`: quem terminou há 10
segundos e quem terminou há 3 horas caem no mesmo lugar.

O Orca separa **Done** (terminou) de **Idle** (parado há mais de ~30 min) e marca não-lido **em
negrito, sem badge de contagem**. O Paseo chama esse estado de *Ready to review*.

- **Toca:** backend (guardar "você viu?" por sessão — sidecar, no padrão dos outros marcadores),
  `sse.py` (`_list_sig`), `SessionInfo`, e as três telas de lista + `Board.svelte`.
- **Depende de:** nada. Anda em paralelo com A1 se for outra pessoa, mas colide no mesmo arquivo —
  melhor sequenciar depois de A1.
- **Tamanho:** médio. É o item com mais valor por trás do A1.

Junto com este, quase de graça: **"falhou" como quinto estado**, a partir do `stalled` e do `limited`
que já estão no `SessionInfo` e hoje só tingem a linha.

## A3. Estatística de diff na linha

/home/jefferson/pessoal/hangar/.refs/outros/paseo-hero-zoom.png

`+188 −133` à direita do nome, e no Vibe Kanban `📄 4 +134 −2`. O slot da direita é **um** de três
coisas, nunca as três: diff, tempo relativo, ou nada.

- **Toca:** `Sidebar.svelte`, `SessionList.svelte`, `SessionCard.svelte`.
- **Depende de:** **B3**.
- **Tamanho:** pequeno, depois que o backend entrega o número.

## A4. Agrupar por estado + preferências de exibição

/home/jefferson/pessoal/hangar/.refs/paseo-live/desktop/16-menu-mostrar.png

/home/jefferson/pessoal/hangar/.refs/outros/vibekanban-sidebar-acordeao.png

O primeiro print é o menu do Paseo, ao vivo e em português: **Agrupamento › Projeto**, **Título ›
Título**, e **Mostrar ›** com `Host ✓ · Pull request ✓ · Serviços ✓ · Verificações › · Estatísticas de
diff · Última atividade`. Repare que **Verificações tem submenu** — são três níveis (ícone e texto,
só ícone, escondido), não um liga-desliga.

Duas coisas no mesmo item:

1. **Acordeão por estado com contagem** — `Precisa de você · Trabalhando · Em revisão · Pronto`, cada
   cabeçalho colapsável com número. É o que resolve a lista plana do celular sem precisar de colunas.
   O Vibe Kanban deixa **quem usa escolher** entre lista plana e acordeão, na mesma tela.
2. **Menu de preferências de exibição** (do Orca e do Paseo): ligar e desligar cada coluna — host, PR,
   estatística de diff, última atividade — e escolher o título entre nome e branch. **É isto que faz o
   A3 não custar caro:** o diff vira opcional, e quem liga paga o fork a mais.

O `cp_group_by` da `Sidebar.svelte:130` já guarda `none | server | project` em `localStorage`. Falta
`status` como quarto modo e falta a folha de preferências.

- **Toca:** `Sidebar.svelte`, `SessionList.svelte`, `lib/format.ts` (`effectiveGroupBy`).
- **Depende de:** nada, mas convive melhor depois de A2 (o acordeão precisa dos estados novos).
- **Tamanho:** médio.

---

# Trilha B — backend

## B1. Presença antes do push

Sem foto — é comportamento, não tela.

O melhor achado da varredura inteira, do Paseo (`agent-attention-policy.ts`):

1. Se algum cliente está **presente** (atividade nos últimos **180s**), **visível** e **focado
   exatamente naquela sessão** → **nenhum aviso**, nem na tela nem push. Você já está olhando.
2. Se há cliente presente mas não focado → **só o mais recente** recebe aviso na tela, **ninguém**
   recebe push.
3. Só quando **ninguém** está presente é que sai push.

E: **erro nunca vira push**, só aviso na tela.

O seu backend já sabe quem tem SSE aberto e em qual sessão — falta o "visível" e o "focado", que o
front manda (`visibilitychange` + rota atual).

- **Toca:** `push.py`, `sse.py`, e um sinal novo do front.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** pequeno-médio. Resolve o incômodo real de receber no celular o aviso da sessão que você
  está lendo no monitor.

## B2. Agendamento

/home/jefferson/pessoal/hangar/.refs/paseo-live/Agendamentos.png

O Paseo separa duas coisas no mesmo motor de cron, e a separação é o insight:

- **Agendamento** — no horário, roda este prompt neste repo: **cria sessão nova a cada disparo**.
- **Heartbeat** — no horário, manda um prompt **de volta para uma sessão que já existe**.

**O seu loop runner é o heartbeat, sem relógio.** O `TickCtx` (`loop.py:80`) já tem `deliver`,
`enqueue`, `notify`, `branch` e o `automations_enabled` como chave-geral.

Três decisões deles que eu copiaria inteiras:

1. **Disparo perdido é pulado, nunca executado em lote.** Na subida, avança o próximo horário em laço
   até cair no futuro; run que estava rodando vira `failed` com o texto *"Daemon restarted before the
   scheduled run completed"*. Para máquina pessoal que dorme, é o comportamento certo.
2. **Cadência com atalhos nomeados** — *A cada minuto · A cada hora · Diário 9h · Dias úteis 9h ·
   Segundas 9h · Cron personalizado* — e a humanização do cron **devolve a expressão crua quando não
   reconhece**, em vez de inventar descrição errada.
3. **A linha meta lê identidade → passado → futuro**: `cadência · Criado há 3d · Última há 2h ·
   Próxima em 12m`, e **"Próxima" só aparece quando está ativo** — pausado não mente sobre o futuro.

- **Toca:** `loop.py` (ou módulo novo `schedule.py`), `api.py`, sidecar novo, sheet nova no front.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** o maior da lista. É o único que vale abrir como plano do superpowers.

## B3. `git diff --shortstat` no `git_summary`

Sem foto — alimenta o A3.

O `git_summary` (`git_ops.py:101`) já roda por sessão a cada varredura, com cache de 3s, e devolve
`{dirty, ahead, behind}`. Falta o par `+N −M`, que é **um fork a mais por sessão por varredura** no
mesmo cache. Por isso o A4 (ligar/desligar a coluna) vem junto.

- **Toca:** `git_ops.py`, `models.py`, `registry.py` (a decoração), `lib/types.ts`.
- **Depende de:** nada. **Paralela com tudo.** Destrava o A3.
- **Tamanho:** pequeno.

---

# Trilha C — telas novas

## C1. Gerenciador de arquivos

/home/jefferson/pessoal/hangar/.refs/orca-live/13-arvore-arquivos.png

/home/jefferson/pessoal/hangar/.refs/outros/paseo-painel-changes-files-pr.png

Você não tem isso hoje, e os dois têm. No Orca é a árvore do painel direito; no Paseo é a aba
**Files** de um painel de três abas (`Changes · Files · #60` — e a terceira é rotulada com o **número
do PR**, não com a palavra "PR").

Dois detalhes do print ao vivo do Orca (feito nesta máquina, num repositório descartável):

- O campo de busca tem **duas abas: `Names` e `Contents`** — procurar por nome de arquivo e procurar
  dentro do conteúdo são o mesmo campo, não duas telas.
- O marcador **`M`** de modificado fica na **direita da linha** e **sobe pela árvore**: `notas.md` está
  marcado, e `docs`, `src` e `lib` herdam a marca. Você acha o que mudou sem expandir nada.

**Metade do backend já existe:** `fs.py` tem `scan_dir` e `getRoots`, hoje usados pelo
`FolderScanner` da criação de sessão. Falta a árvore navegável e o leitor de arquivo.

- **Toca:** componente novo + rota/aba nova; `fs.py` ganha leitura de arquivo.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** médio.
- **Cuidado:** desktop e celular são caminhos diferentes. No celular o Paseo põe a árvore como um dos
  três destinos arrastáveis; no Orca é painel lateral. Decida antes de começar.

## C2. Diff do arquivo inteiro, a partir da árvore

/home/jefferson/pessoal/hangar/.refs/paseo-live/desktop/09-alteracoes-diff-arquivo.png

/home/jefferson/pessoal/hangar/.refs/orca-live/15-diff-arquivo.png

O primeiro é o Paseo ao vivo, com uma sessão de verdade. A anatomia do painel direito, de cima pra
baixo: abas **Alterações | Arquivos** · branch (`master`) · seletor **"Sem commit ⌄"** (é o *Diff
mode*: não-commitado ou commitado) · a linha do arquivo com **`+14 −0` à direita** · o diff com
numeração e cabeçalho de hunk · e no rodapé a seção **Commits**, colapsável. Os quatro controles do
canto são expandir tudo, lado a lado, árvore ↔ lista, e quebra de linha.

O que você tem hoje é o `EditDiff.svelte` **dentro da mensagem** — mostra só o trecho que o agente
editou naquele turno. O que falta é: **clicar no arquivo e ver tudo que mudou nele**, somando todos os
turnos.

**Boa notícia: o backend já faz.** `git_ops.file_diff(cwd, path)` e `changed_files(cwd)` existem e já
alimentam o `GitChangesTab`. A tarefa é ligar a árvore do C1 a essa chamada — não é escrever diff novo.

Do Paseo, o que vale copiar de comportamento: alternância unificado ↔ lado a lado, esconder espaço em
branco, e vazios específicos por causa (*"Nenhuma mudança não commitada"*, *"Nenhuma mudança visível
depois de esconder espaço em branco"*, *"Este diff é grande demais para pré-visualizar"*).

- **Toca:** componente novo, reusando `git/DiffView.svelte`.
- **Depende de:** **C1**.
- **Tamanho:** pequeno-médio, já que o backend está pronto.

## C3. Barra com os limites de todas as contas

/home/jefferson/pessoal/hangar/.refs/orca-live/09-barra-uso-contas.png

Print ao vivo, com os dados reais desta máquina. O formato é melhor do que eu tinha descrito antes:

```
☀ ▬▬  24% used 2h 8m · 66% used 2d 4h · 0% used Fable
◍ ▬▬  0% used 6d 20h
◔  ⚠ Run Kimi to refresh                    │  836.2 MB · >_ 0 · ⚡ 0
```

Três coisas que valem copiar:

1. **Cada janela é `percentual + quanto falta para o reset`** — `24% used 2h 8m` — não os rótulos
   `5h`/`wk`. Uma string só responde "quanto sobrou" e "quando volta".
2. **Provedor sem dado mostra o que fazer**, não um espaço em branco: `⚠ Run Kimi to refresh`.
3. **Um modelo com cota própria aparece separado**: `0% used Fable`, ao lado das janelas da conta.

No celular deles é a seção **ACCOUNT USAGE**:

/home/jefferson/pessoal/hangar/.refs/orca/11-desktop-com-celular.png

Você tem `RateChips` (⚡5h/📅7d/💵) **por sessão** e `RateStrip` por servidor. O que falta é
**todas as contas ao mesmo tempo** — e você já tem múltiplos `CLAUDE_CONFIG_DIR`
(`listClaudeConfigs`/`criarConta`), então o dado existe. Do Orca, a **ordenação com o limite mais
apertado primeiro** e o **aviso ao cruzar 80%**.

- **Toca:** componente novo + um endpoint que leia o uso de cada config dir.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** médio — a parte cara é ler o uso de uma conta que não é a da sessão ativa.

## C4. QR para parear o celular

/home/jefferson/pessoal/hangar/.refs/paseo-live/qr-parear.png

Você já tem **metade**: o `QrScanner.svelte` na tela de login **lê** QR. Falta **gerar** — o servidor
mostrar um QR com endereço + token para o celular escanear.

E aqui um achado que simplifica a sua vida: no Paseo o QR **só existe no caminho do relay**. A própria
tela diz: *"Sem relay, conecte diretamente por TCP, Tailscale ou outra VPN. **Nenhum código QR é
criado.**"* Ou seja — eles precisam do relay porque o endereço não é previsível. **No seu caso é**:
LAN ou VPN, endereço e token que o backend já conhece. Seu QR é mais simples que o deles.

- **Toca:** endpoint novo que devolve o QR (PNG ou SVG) + uma tela nas Configurações.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** pequeno. É a tarefa mais barata da trilha C.

**Sobre o app Expo:** o Paseo e o Orca têm app nativo, você tem PWA instalável. Antes de escrever app,
vale listar o que o app daria que o PWA não dá — notificação fora do navegador você já tem via VAPID;
sobra ícone na bandeja, atalho global e leitura de QR pela câmera nativa. O QR do C4 funciona no PWA.

---

## C5. Detectar os CLIs de agente instalados

/home/jefferson/pessoal/hangar/.refs/orca-live/02-onboarding-1-agente.png

O Orca abriu aqui e **detectou sozinho 7 CLIs já instalados** (Claude, Claude Agent Teams, OpenClaude,
Codex, OpenCode, Pi, Kimi), com um *"Show 28 more agents"* pro resto do catálogo. O Paseo faz igual e
mostra **"Não instalado"** no provedor que falta.

### Como eles fazem — `packages/server/src/executable-resolution/executable-resolution.ts`

Dois passos:

**1. Enumerar TODOS os candidatos, não o primeiro.** `/usr/bin/which -a <nome>` no POSIX (com a lib
`which` como reserva), deduplicado. O `-a` importa nesta máquina: com fnm existe shim em
`~/.local/state/fnm_multishells/…` além de `/usr/local/bin` e `~/.local/bin`, e o primeiro da lista
pode ser um shim de uma versão de Node que nem está instalada.

**2. Sondar cada candidato rodando `<caminho> --version`**, com timeout de 2s, buffer de 64KB e
`SIGKILL`. O primeiro que responde vence.

A parte boa é a classificação do erro (`classifyProbeError`), que separa "não existe" de "existe e não
gostou":

| Resultado da sonda | Veredito |
|---|---|
| Morto por timeout (`killed`) | **disponível** — trava no `--version`, mas existe |
| Saiu com código numérico, mesmo ≠ 0 | **disponível** — rodou, só não implementa `--version` |
| `ENOENT` · `EACCES` · `ENOEXEC` · `UNKNOWN` | **indisponível** |

Ou seja: **"o programa existe" não é "o programa saiu com 0"**. É isso que evita falso negativo em CLI
lento ou que não implementa `--version`. O catálogo é estático — eles não descobrem agente novo
sozinhos, sondam uma lista conhecida.

### Por que vale aqui

Resolve uma falha já registrada no `CLAUDE.md`: *"motor configurado pelo celular abre um pane que morre
na hora (tmux new-session ainda retorna 0, o app reporta sucesso calado)"*. Detectar antes de criar é o
conserto desse sucesso calado.

O encaixe existe: o `Adapter` já tem `spawn_command` (`adapters/base.py:28`), então cada provider já
sabe qual binário chama. Falta a sonda.

**Ressalva de Python:** `shutil.which()` devolve **só o primeiro** — para o comportamento do `-a` é
preciso percorrer `os.environ["PATH"]` na mão. O resto é `subprocess.run([caminho, "--version"],
timeout=2)` classificando igual à tabela, **com cache**, porque isso roda na tela de criar sessão e não
a cada varredura.

- **Toca:** módulo novo no backend + `CreateSessionSheet.svelte` + a tela de Motores.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** pequeno. Duas entregas: marcar provider indisponível na criação (com "instale X" em vez
  de deixar criar uma sessão que morre) e a mesma sonda servindo a tela de Motores.

---

# Trilha D — texto (zero colisão)

## D1. Frases de indisponibilidade no Git

Sem foto — é texto.

O item mais barato e mais valioso de toda a varredura. O Paseo tem cerca de **25 frases** que explicam
por que uma ação está desligada:

> *"Pull não está disponível enquanto há mudanças locais — commite ou guarde no stash primeiro"*
> *"Push ainda não está disponível porque há mudanças novas para trazer antes"*
> *"Criar PR não está disponível porque esta branch ainda não tem commits novos"*

**Nenhum botão cinza mudo. Todo bloqueio se explica e diz a saída.** O `GitStatusBar` daqui mostra o
erro **depois** que a operação falhou; isso é o contrário — explica antes.

- **Toca:** `components/git/*` (strings e o `disabled` de cada ação).
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** pequeno. Cabe numa tarde.

## D2. Três vazios por causa, no Arquivo e nas listas

Sem foto — é texto.

O Paseo tem **três textos de vazio diferentes, um por motivo**: buscando → *"Nenhuma sessão
corresponde"*; sem filtro → *"Nenhuma sessão ainda"*; servidor filtrado → *"Nenhuma sessão neste
servidor"*. O comentário no código deles: *"uma lista vazia significa outra coisa quando há uma busca
estreitando ela"*.

Mais dois: **faixa de servidor que falhou** (`{servidor}: não foi possível carregar`) — sem ela a
lista subnotifica em silêncio — e, quando o resultado trunca, trocar "Carregar mais" por **"Resultados
demais — estreite a busca"**.

- **Toca:** `Archive.svelte`, `SessionSwitcherSheet.svelte`, as listas.
- **Depende de:** nada. **Paralela com tudo.**
- **Tamanho:** pequeno.

---

# Trilha E — transversal

## E1. Inglês (internacionalização)

**Decidido em 13/08/2026: o app deve ter inglês.** Hoje não tem nada — conferido no
`frontend/package.json`: as dependências são `@xterm/*`, `qr-scanner`, `shiki` e `uplot`, e mais nada.
Zero biblioteca de tradução, zero sistema de chaves. Strings escritas à mão em português nos 122
arquivos `.svelte`. O único lugar com noção de idioma é o `lib/fmt.ts:27`, e é fixo:
`new Intl.NumberFormat('pt-BR', …)`.

### Esta é a única tarefa que NÃO paralelza com nada

E é o fato que mais importa pro planejamento. **Toda tarefa deste backlog escreve texto novo de
interface** — a linha de três andares (A1), os estados novos (A2), o menu de preferências (A4), o
gerenciador de arquivos (C1), a barra de contas (C3), a tela de agendamento (B2), e a D1 sozinha
acrescenta cerca de 25 frases.

Duas ordens possíveis, e uma é claramente melhor:

- **i18n primeiro** → monta o mecanismo, extrai o que existe hoje, e daí em diante **toda string nova
  já nasce com chave**. Paga a extração uma vez.
- **i18n depois** → escreve tudo em português cru e extrai de novo no fim. Paga duas vezes, e a
  segunda passada mexe em todos os arquivos que as outras trilhas acabaram de tocar.

**Recomendação: fazer o E1 antes de abrir A, C e D.** O B1/B3 (backend puro) pode andar em paralelo,
porque não escreve texto de interface.

### O que a pesquisa ensina

- **As duas referências usam `i18next`** — o Paseo com `react-i18next` (dicionário em
  `packages/app/src/i18n/resources/en.ts`), o Orca com `i18next-cli` pra extrair. Foi por isso que a
  interface do Paseo apareceu em português sozinha nesta máquina.
- **No mundo Svelte 5 + Vite**, o alinhado é o **Paraglide (inlang)**, que compila as mensagens em vez
  de carregar dicionário em runtime. Alternativas clássicas: `svelte-i18n`, `typesafe-i18n`.
- **A disciplina importa mais que a biblioteca.** O Orca tem i18n completo, seis idiomas, e uma busca
  de configurações que casa a palavra nativa (você acha "Language" digitando "Idioma" com a interface
  ainda em inglês) — **e mesmo assim os cinco estados de sessão estão chumbados em inglês, fora do
  sistema de tradução** (`sidebar-status-view-model.ts`). Vazou justo no texto mais visível.
- **Detalhe do Orca que vale copiar:** a busca das configurações casa o termo no idioma nativo
  (`语言` / `언어` / `言語` / `Idioma`), pra você achar a opção de idioma sem saber o idioma atual.

### São três camadas, e só a primeira é sua

1. **A interface do front** — o escopo desta tarefa.
2. **As mensagens que o backend manda pra tela** — `git_ops.py`, `loop.py` e os erros da API respondem
   em português hoje. Ou viram chave também, ou o backend passa a devolver código de erro e o front
   traduz (é o caminho certo, e é o que permite a frase de indisponibilidade da D1 ser traduzível).
3. **A saída do próprio agente** — fala o idioma que quiser. Não tem o que fazer, e não deve ter.

- **Toca:** os 122 `.svelte`, `lib/fmt.ts`, e a decisão da camada 2 no backend.
- **Depende de:** nada. **Mas tranca A, C e D.**
- **Tamanho:** o mecanismo é pequeno; a extração é o custo real. Vale medir quantas strings são antes
  de estimar.

---

# Fora desta lista, de propósito

Discutido e **descartado por ora**:

- **Modo headless (SDK / stream-json).** Analisado a fundo. É aditivo — `adapters/base.py` é um
  `Protocol` de 8 métodos e o `ClaudeAdapter` tem ~50 linhas de delegação, então um modo novo é uma
  pasta ao lado de `codex/`. Mas **não conserta nada que esteja quebrado hoje**, apaga a statusline
  (`--print` não renderiza nenhuma) e tira do tmux a posse do processo. Passa a valer se o canal de
  permissão por chamada for confirmado, ou se alguma versão do Claude Code quebrar a leitura do pane.
- **Worktree por tarefa.** É a aposta central do Orca e do Paseo, incompatível com "dirigir uma sessão
  tmux que já existe". Decisão já registrada no `orchestrating-idea-to-push`.
- **Painel de PR / checks.** Área nova inteira. O pedaço barato seria só o **chip vermelho quando um
  check falha** com a ação "manda o log do check falho para o agente".
- **Comentário em linha de diff virando anexo do prompt.** Sete produtos têm; é a lacuna nº 1 da
  categoria. Ficou fora **desta rodada** por depender do C1/C2 estarem prontos primeiro.

---

# Ordem sugerida, se for tocar sozinho

0. **E1** — o inglês. Antes de A, C e D, senão toda string nova nasce em português cru e é extraída
   duas vezes. O B1 e o B3 podem andar junto, porque são backend puro.
1. **A1** — última mensagem na linha. Muda o dia a dia na hora.
2. **D1** — frases de indisponibilidade. Uma tarde, retorno imediato.
3. **B1** — presença antes do push.
4. **B3 → A3** — diff na linha.
5. **A2** — não-lido.
6. **C4** — QR.
7. **C1 → C2** — arquivos e diff por arquivo.
8. **A4**, **C3**, **D2**.
9. **B2** — agendamento, como plano próprio.

---

# Parte 6 — Apêndice: o que já foi investigado e decidido

Esta parte existe para **não refazer investigação**. Cada seção fecha uma pergunta que já foi
respondida com medição, não com opinião.

## 6.1 Modo headless (stream-json) — investigado a fundo, adiado

**A pergunta era:** vale rodar o Claude por `--print --output-format stream-json` em vez do TUI em
tmux, como o Paseo e o Orca fazem?

### O encaixe existe e é aditivo

`adapters/base.py` é um `Protocol` de 8 métodos, e a docstring diz: *"o Adapter troca só a FONTE de
cada evento, nunca o shape"*. O `ClaudeAdapter` tem **~50 linhas de pura delegação**. O Codex é a prova
de que funciona: `adapters/codex/` tem 837 linhas, sobe um app-server próprio, e o caminho do Claude em
tmux não foi tocado. Um modo headless seria uma sexta pasta ao lado de `codex/`, `kimi/` e `pi/`.

### Não use o SDK — as flags já estão no CLI

O adaptador Claude do Paseo tem **6.046 linhas de TypeScript**, e a maior parte reimplementa o que o
Hangar já tem. Não existe SDK do Agent em Python. E o SDK é um embrulho: ele **spawna o binário
`claude` instalado** (`pathToClaudeCodeExecutable` + `spawnClaudeCodeProcess`) e lê
`settingSources: ["user","project","local"]` — mesma credencial, mesmos hooks, mesmas skills.

Medido no `claude` **2.1.231** instalado aqui:

| Flag | O que resolve |
|---|---|
| `--print --input-format stream-json --output-format stream-json` | o canal nos dois sentidos — é o que o SDK usa por baixo |
| `--include-partial-messages` | o texto em voo → alimentaria o `preview.py` sem raspar pane |
| `--include-hook-events` | ciclo de vida dos hooks dentro do próprio fluxo |
| `--replay-user-messages` | *"re-emite as mensagens vindas do stdin de volta no stdout para confirmação"* — é a confirmação de entrega que o `pqueue.py` faz na mão |
| `--permission-mode` | `acceptEdits · auto · bypassPermissions · manual · dontAsk · plan` |
| `--session-id` / `--resume` | mesmo transcript de sempre, mesmo `~/.claude/projects/` |

Do Python é `asyncio.create_subprocess_exec` com pipes. Sem Node, sem dependência nova.

### A incógnita que decide o formato

O `--help` da 2.1.231 mostra o `--permission-mode`, mas **não** mostra flag que devolva um pedido de
permissão individual para quem chamou. O `canUseTool` do SDK — o que faz aparecer Allow/Deny no celular
— usa um canal de controle por cima do mesmo stdio que **não está documentado no `--help`**.

**Isso precisa ser medido antes de decidir.** Sem ele, você escolhe o modo na largada e pronto: não há
aprovação por chamada, e o principal motivo para fazer headless evapora.

### As duas perdas reais

1. **A statusline apaga.** Em `--print` o Claude não renderiza statusline nenhuma, e o `statusline.py`
   depende de o agente publicá-la. Modelo, contexto, ⚡5h/📅7d e 💵 sumiriam nas sessões headless — a
   não ser que o próprio Hangar calcule a partir do `usage` que vem no fluxo.
2. **O tmux deixa de segurar o processo.** Hoje a sessão sobrevive ao SSH cair, ao app fechar e ao
   backend reiniciar porque o dono é o tmux. Em headless o processo é filho do backend. Foi exatamente
   por isso que o Paseo precisou de um daemon separado e o Orca também. O `systemd-run --user --scope`
   que o `registry.py` já usa é o candidato óbvio.

### Pareamento sobrevive

O `cp-send` fala com a **API do backend**, não com o tmux, e o caminho nativo entrega **por socket**
(comentário na linha 29 do script: *"some a classe de bug do send-keys — texto cortado no teto de 16KB
do tmux, `\n` submetendo linha, reenvio concatenando no composer"*). Em headless fica mais simples
ainda: uma linha JSON no stdin. Quebram só duas coisas menores: **`cp-send --new`** (cria sessão tmux
gerenciada) e a detecção de "quem sou eu" (usa o `CP_SESSION_NAME` carimbado pelo tmux).

### Veredito

**Adiado.** Não conserta nada que esteja quebrado hoje — o pane já virou reserva, não fonte: a
statusline vem de sidecar, a prévia vem de sidecar, o estado vem de hook. O ganho real é **sumir uma
classe de bug** (o picker de modelo que dirige o terminal, o glifo de spinner que engoliu o painel de
tarefas, a statusline morrendo em `cache…` num pane de 99 colunas), mas essa superfície já encolheu.

**Volta à mesa em dois cenários:** se o canal de permissão for confirmado e você quiser Allow/Deny no
celular; ou se alguma versão do Claude Code quebrar a leitura do pane de um jeito que remendar saia
mais caro que escrever o adaptador.

## 6.2 O termo legal — onde o Hangar cai

Fonte: `https://code.claude.com/docs/en/legal-and-compliance`, lido em 13/08/2026.

**O Agent SDK está nomeado como coberto pela assinatura:**

> *"Advertised usage limits for Pro and Max plans assume **ordinary, individual usage of Claude Code
> and the Agent SDK**."*

**O corte não é "SDK ou CLI" — é de quem é a credencial:**

> *"**OAuth authentication** is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and
> Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native
> Anthropic applications."*

> *"**Developers** building products or services that interact with Claude's capabilities, including
> those using the Agent SDK, **should use API key authentication** through Claude Console or a
> supported cloud provider. Anthropic **does not permit third-party developers to offer Claude.ai login
> or to route requests through Free, Pro, or Max plan credentials on behalf of their users**."*

> *"Anthropic reserves the right to take measures to enforce these restrictions and may do so without
> prior notice."*

**O Hangar como é usado hoje está do lado certo, com folga:** uma máquina, um `~/.claude`, um usuário,
e o app é controle remoto do Claude Code que já roda ali. O token bearer autentica contra o próprio
backend; a credencial do Claude nunca sai da máquina. O hub na VPS guarda o cofre cifrado da lista de
servidores, não encaminha requisição de modelo.

**Duas coisas moveriam a linha**, e vale ter isso claro antes de qualquer decisão futura:

1. **Uma instalação servindo várias pessoas pela sua assinatura** — é exatamente o *"route requests
   through Pro or Max plan credentials on behalf of their users"*.
2. **Publicar não é problema; hospedar por outros é.** Cada pessoa rodando na própria máquina com a
   própria conta continua sendo uso individual. O que não pode é uma instância central com uma
   credencial só.

Os **motores de modelo** (Kimi e afins) não entram nessa conta — provedor diferente, termos do
provedor. Isto é leitura da página, não parecer jurídico; para hospedar por terceiros, a própria página
aponta o `contact sales`.

## 6.3 O que os apps mostraram rodando ao vivo

Instalados e executados nesta máquina em 13/08/2026. Prints em `.refs/orca-live/` (29) e
`.refs/paseo-live/desktop/` (26).

### Orca

- **O app funciona inteiro sem conta.** Nenhum login foi feito; existe "Orca Account" nas
  configurações, é opcional.
- **Detectou sozinho 7 CLIs de agente** já instalados aqui, com *"Show 28 more agents"* pro resto. É a
  base da tarefa C5.
- ⚠️ **A caixa "Yolo / Dangerously skip permissions" vinha MARCADA por padrão** no onboarding. Coerente
  com a doutrina deles (*"o worktree é o sandbox"*), mas sem worktree isolada seria permissão total numa
  árvore de trabalho real. Segue na lista do que **não** copiar.
- **A barra de limites usa `percentual + quanto falta para o reset`**, não os rótulos `5h`/`wk` que o
  print de marketing sugeria: `24% used 2h 8m · 66% used 2d 4h · 0% used Fable`. Provedor sem dado
  mostra ação (`⚠ Run Kimi to refresh`), não espaço em branco. Modelo com cota própria aparece em linha
  separada (`0% used Fable`).
- **A árvore de arquivos** tem busca com duas abas — `Names` e `Contents` — e o marcador **`M`** de
  modificado **sobe pela árvore**: o arquivo marcado faz `docs`, `src` e `lib` herdarem a marca.
- **Automations vem com 4 modelos prontos**: *Weekday repo audit · Release readiness · Daily change
  review · Hourly queue check*.

### Paseo

Rodou sessão de verdade pelo provedor **Pi** com o modelo **`opencode-go/deepseek-v4-flash`** — o
`opencode` não está instalado nesta máquina e o Paseo marca "Não instalado". A conta Claude não foi
usada.

- ⚠️ **A faixa de subagentes acima do composer NÃO apareceu com o provedor Pi.** Os subagentes saíram
  como linhas clicáveis dentro da própria conversa. Ou seja: aquela string `"N subagents · M running"`
  descrita na Parte 4.7 **pode ser específica do provedor Claude** — não é comportamento geral.
- **A aba de PR é condicional a existir forge.** No repositório de demo só apareceram *Alterações* e
  *Arquivos*.
- **A pílula do modelo mostra o multiplicador de custo**: `DeepSeek V4 Flash (2x usage)`. O preço do
  modelo aparece no seletor, antes de você mandar.
- **O composer traz `Ctrl+L para focar`** escrito à direita, dentro do próprio campo.
- **Um pedido morreu por falha do provedor**, não do app: `[System Error] Stream ended without
  finish_reason` ao disparar três subagentes de uma vez.
- **O menu de modo de permissão** apareceu como o manifesto descreve: *Plan mode · Always ask · Accept
  file edits · Auto mode · Bypass*.

### Estado deixado na máquina

Paseo continua instalado, **daemon parado**, relay **nunca ativado**. Sobrou o projeto `demo-paseo`
registrado e o worktree `~/.paseo/worktrees/3d7fuln4/`. Limpar tudo é
`rm -rf ~/.paseo` + `npm rm -g @getpaseo/cli`. O Orca é um AppImage no scratchpad, sem instalação no
sistema.
