# Hangar

Drive live Claude Code, Codex, Pi, omp, and Kimi sessions from your phone over LAN/VPN. Terminal
sessions run in `tmux`; Claude and Codex can instead run as Hangar-managed headless processes.
Single-user, LAN/VPN-only by design. Backend: Python 3.14 + FastAPI
(`backend/`). Frontend: Svelte 5 PWA (`frontend/`).

- **Architecture + full API table + run guide:** [`README.md`](README.md).
- **End-user / setup guide** (pairing, Tailscale, install as PWA, every feature): [`docs/USAGE.md`](docs/USAGE.md).
- Other docs in `docs/`: design brief, onboarding/network, polish backlog, tmux setup, future features.

## Architecture at a glance

The app never scrapes the terminal for chat content — it reads structured transcripts/events.
Only terminal sessions use the tmux pane for live **state** and input. Backend pieces (`backend/app/`):

- `registry.py` — SessionRegistry: joins terminal sessions from tmux with durable Claude/Codex
  headless sidecars and their JSONL/rollout history.
- `transcript.py` — tails `~/.claude/projects/<cwd>/<uuid>.jsonl` (the chat content).
- `state.py` — classifies live state from `tmux capture-pane`: `working` / `idle` / `awaiting_input` / `dead`
  (reserva Python de Claude/Codex; o estado de Pi, omp e Kimi é do ator Rust em `runtime/other_terminal/`).
- `terminal_input.py` + `tmux.py` — input via `tmux send-keys` (prompt / option select via `(n-1)×Down`+`Enter` / `Esc`).
- `adapters/codex/` — um app-server WebSocket de loopback por sessão Codex; o backend
  consome eventos JSON-RPC enquanto a TUI `codex --remote` da mesma thread roda no tmux.
  O app-server é do PANE, não do backend. Sem terminal (`sem_terminal.py`), ele roda em stdio
  como filho do mesmo cano do Claude sem terminal, e o backend abre a thread e responde as
  aprovações por cartão. Decisões e armadilhas:
  [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md).
- `difusor.py` — **uma fonte por chave, não uma por conexão**: monitor de estado e
  acumulador de estatísticas são compartilhados entre os SSE abertos.
  Por quê e o que quebrou antes: [`docs/decisoes/plataforma.md`](docs/decisoes/plataforma.md).
- Voz ao vivo — roda no `hangar-server` (`crates/hangar-server/src/voice/`), **disponível em todo
  servidor com Codex instalado e ao menos uma conta Codex**, sem interruptor; o microfone só abre
  quando a pessoa liga a chamada. Uma chamada por
  servidor, só do dono (`/api/voice`); o áudio fica no
  aparelho. Clientes: nativo (`desktop-native/src/voice/`) e PWA no celular
  (`lib/liveVoice.ts`). Contrato: "Regras vigentes" de
  [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md).
- `crates/hangar-server/src/runtime/other_terminal/` — atores Pi, omp e Kimi: identidade da vida,
  estado, prévia, perguntas, fila, entrada, fechamento e troca de nome. Os leitores estruturados
  ficam em `hangar-transcript`; hooks e guarda do Kimi/Pi são comandos Rust em `provider_hooks/`.
- `sse.py` — merges the above into the SSE stream. `api.py` — FastAPI routes. `auth.py` — bearer token / `cp_token` cookie.
- Also: `pqueue.py` (durable input queue), `preview.py` (live in-flight block), `askquestion.py`
  (native AskUserQuestion stepper), `uploads.py`,
  `model_picker.py`, `config.py`, `hook_installer.py`.
- `crates/hangar-server/src/commands/` — catálogo de comandos do Claude, sonda compartilhada e
  filtro em processo; o Python só autentica e repassa nas portas que atende.
- `site/` — landing page de `hangar.dev.br` (HTML estático em PT/EN gerado por `site/build.py`,
  publicado no Cloudflare Pages); vídeos do app gravados por `site/tools/record/`. Ver `site/README.md`.

Frontend (`frontend/src/`): `screens/` (Chat, Board, …), `components/` (MessageList, NavBar, Composer,
bubbles, sheets, Spinner/Lottie, …), `lib/` (`api.ts` SSE client, `activity.ts`, `markdown.ts`,
`format.ts`, `types.ts`), `app.css` (design tokens + shared keyframes).

**Three sibling desktop views**, toggled by the grid button in the sidebar header/rail:
- **list + chat** — `Sidebar` + `Chat` (the original).
- **board** (`#/board`) — `screens/Board.svelte`: a kanban of sessions in 3 columns by state
  (`awaiting_input` / `working` / `idle`), each card a live mini-chat (`components/BoardCard.svelte`):
  tail of the conversation, an inline input, and option buttons for a pending picker. Clicking a card
  opens the **real `Chat`** as an overlay via the route `#/board/<serverId>/<name>` (so deep-link, browser
  back and reload all work). Entering the board auto-collapses the sidebar to its rail.
  - **The board must never open an SSE per card** — browsers cap ~6 per host. Live state comes from the
    aggregated `openSessionsStream` (one per *server*); the card's conversation comes from
    `GET /history?limit=N` on mount only. There is no `dead` column: `classify()` never returns `dead`
    for a live terminal row (only the per-session SSE does). The list does carry `dead` rows for a headless
    process that died (`headless_processo_caiu`) and for a terminal session the systemd killed
    (`terminal_out_of_memory`/`terminal_died`, `session_open/fallen.rs`: Reabrir and Dispensar); the board,
    home and canvas leave those out.
- **canvas** (`#/canvas`) — `screens/Canvas.svelte`: the free-form sibling of the board. Same
  `BoardCard` (now with a `fill` prop), but each card is a floating tile you drag by its handle and
  resize with the native CSS resize corner (a `ResizeObserver` captures the size). No columns and no
  auto-grouping by state — the trade-off the user chose for full position/size freedom. Same
  invariants as the board: **never an SSE per card** (state comes from the shared `sessionsStore`),
  and clicking a card opens the real `Chat` as an overlay via `#/canvas/<serverId>/<name>` (peek
  covers the canvas, Esc restores). Layout is persisted in `localStorage` under `cp_canvas_layout`,
  keyed `serverId::name`; first-seen cards get an initial slot in per-server columns via
  `lib/canvasLayout.ts` (`placeNew`). Mobile falls back to `SessionList` (canvas is desktop-only).

## Dev commands

Requirements: `tmux`, `claude` (Claude Code), a current `codex` CLI with `--remote`,
Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+. Optional, one per provider you use:
`pi`, `omp` (oh-my-pi), `kimi`.
Frontend uses **npm** (has `package-lock.json`).

**Instalação web seletiva:** `npm ci --workspace=@hangar/core --workspace=frontend`, inclusive no
CI e nos instaladores. `mobile/` continua nos workspaces da raiz para o EAS incluir o core.
Para trabalhar no app, use `npm install` em `mobile/`, que tem lockfile próprio; preserve os
caminhos de workspace no Metro e `react-dom` dos testes na versão de `react`.
Ao alterar instalação ou empacotamento, consulte a [evidência em instalacao.md](docs/decisoes/instalacao.md#instalação-seletiva-dos-workspaces).

```bash
# Backend — binds http://127.0.0.1:8765 (set CP_LAN_BIND_IP to a LAN IP for phone access)
cd backend && CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main
cd backend && uv run pytest -v             # backend test suite

# Frontend (run from repo root with --prefix, or cd frontend first)
npm --prefix frontend run dev              # Vite dev server
npm --prefix frontend run build            # production build — does NOT typecheck
npm --prefix frontend run check            # svelte-check + tsc — THIS is the type gate

scripts/cargo-pinned build -p hangar-server   # target comum da máquina, trava, jobs e teto pela memória livre; fixa o binário em crates/target da worktree
scripts/verificar-local                    # só o ligado aos arquivos mudados (o que a trava pre-push exige)
scripts/verificar-local --completo         # o que o CI rodaria para o commit (Linux + VM Windows)
scripts/verificar-local --passos rust      # repete só um passo (o que já passou nesta árvore fica gravado)
./scripts/test-wrappers.sh                 # claude-engine (bash/zsh/fish) against a fake `claude`, no tmux
./scripts/test-statusline.sh               # statusline.js contract (engine sessions suppress cost), needs node
node scripts/test-pi-hangar-state.mjs          # hangar-state.ts: fork de subagente do Pi não rouba o pane
```

**A verificação da tarefa é o uso real**: abrir a tela, clicar, rodar o fluxo. **Teste
automatizado avulso (vitest, pytest, check) só roda quando o usuário pedir** — ele pede no fim, antes do
push; nem entre edições, nem ao terminar a tarefa. Pedido o focado: num comando só, os testes dos
arquivos tocados (`cd backend && uv run pytest tests/test_x.py tests/test_y.py`,
`cd frontend && npx vitest run src/x.test.ts src/y.test.ts`); falhou → repita só o que falhou.
Pedido o completo: `npm run check` na raiz, `vitest` e `pytest` inteiros. **`scripts/verificar-local` é exigido ao
abrir PR e ao subir para a `main` ou para branch com PR aberto**: ele roda, numa árvore fixa da
máquina e em fila única, só o ligado aos arquivos que o commit muda (`--completo`: o que o CI rodaria),
no Linux e, com arquivo de Windows no diff, na VM (`git config hangar.verificarWindows <host>`, teto de
10 min), e grava pela árvore cada passo que passou. De Rust o padrão só confere se o workspace
compila (`cargo check`) e testa os crates menores: os testes do `hangar-server` são do workflow
`Server` e do `--completo`. No Linux os
passos rodam em faixas paralelas e o pytest em vários processos: `git config hangar.verificarProcessos <n>`
(padrão 4; `1` = tudo em fila). O Node é o do `.node-version`, o mesmo do CI: com o `fnm` instalado, o
script e o `ci-gates.sh` trocam sozinhos; sem ele, o aviso diz que front e mobile podem falhar por isso. O hook do
`gh pr create` (`.claude/settings.json`) confere o PR inteiro desde a base; a trava `pre-push`, cada
push. Branch sem PR sobe livre. Emergência explícita: `HANGAR_SEM_VERIFICACAO=1` no comando. O que ela cobre e o que só o CI cobre:
[instalacao.md](docs/decisoes/instalacao.md#verificação-local-antes-do-push-06102026). `npm run build` só para servir o `dist` local. Ao reportar, diga o que foi conferido no uso real e que os testes automatizados não rodaram.

Claude sessions with a terminal must run as `claude --session-id <uuid>` **inside tmux** —
`scripts/install-claude-wrapper.sh` sets this up. Headless Claude/Codex sessions are created by the
app or `hangar-send --new ... --headless` and are discovered from their durable sidecars.
The same installer also wraps interactive `codex`: it calls the local backend through `scripts/hangar-codex`,
creates a managed Codex app-server/TUI pair, and attaches the caller to that tmux session. Codex
subcommands/advanced flags remain raw; `command codex` is the explicit bypass.

## Sessões-irmãs (hangar-send) + pareamento

Sessões usam `scripts/hangar-send` para recados, pareamento e criação; consulte `--help` antes
dessas operações. Recados e pareamento alcançam outros servidores por `servidor::sessao`; o
pareamento forma um grupo entre as máquinas, e `--group` chega a todas. A sessão nasce no
modelo/esforço/permissão pedidos, validados por `app/model_args.py`. Pareamento = grupo gravado
pelo Rust (`crates/hangar-server/src/groups/`, sidecars em `<config>/.hangar-pair/`) + prompt de
protocolo injetado em quem estava sem grupo; a UI mostra chip 🤝 (Composer), badges nas listas,
PairSheet (conversa do par + contrato compartilhado `grupo-<gid>.md` + split view desktop). O
contrato fica na máquina dona; de outra máquina, `hangar-send --contrato` ou a tool MCP `contract`.

**Par noutro modelo:** `--engine <motor>` faz a sessão nova nascer num motor de
`~/.claude/engines.json` (ver "Model engines" nas convenções). Vale pra parear uma sessão Claude com
uma Kimi/GPT no mesmo trabalho: o par continua no MESMO `~/.claude` — skills, hooks, contrato
compartilhado, PairSheet, tudo igual —, só o motor difere, e o consumo vai pra conta do provedor.
O flag só repassa `engine` pro `POST /api/sessions`, então motor inexistente volta `400 motor
invalido` e a sessão **não** nasce (nunca uma sessão que parece estar no motor e não está). O texto
do protocolo que as sessões leem vive no heredoc de `scripts/install-hangar-send.sh` — editar o
`~/.claude/CLAUDE.md` direto é perdido no próximo sync.

Skills do repo em `skills/` (symlinkadas em `~/.claude/skills/` pelo installer):
[`orquestrar`](skills/orquestrar/SKILL.md) conduz um trabalho em um ou vários repositórios,
quando o usuário pedir o fluxo ou o kick-off mandar invocá-lo com `Role:`. A rota é decidida na
fase 1 e só escala: `audit` (quem planejou escreve, uma revisão fresca do diff inteiro fecha) ou
`full` (após o planejamento aprovado, executor e revisor independente trabalham com portão entre
Tasks e revisão final da branch). A linha do executor na tabela do time pode ser escolhida pelo
`Risk:` da Task (`vez` = `low`/`high`) em vez de rodízio. Push depende de autorização do usuário. Um escritor por árvore; Tasks independentes rodam em paralelo por padrão, uma por
worktree. **Só planejador e árbitro invocam a skill**: executor, revisor, revisão final e
retrospectiva recebem no kick-off o caminho da página do papel
(`~/.claude/skills/orquestrar/references/<papel>.md`) e leem só ela e as irmãs que ela nomeia
no passo. Cada página é escrita como
regra imperativa, sem motivo nem caso, com teto de tamanho conferido por
`scripts/checar-orquestrar.sh`. Contrato visual, exceções de paralelismo e demais etapas ficam
na skill.

Skills de terceiros para quem trabalha no código moram em `.agents/skills/` (Codex e outros
agentes), com atalho em `.claude/skills/` (Claude Code), e o installer não as leva às máquinas.
Entram e se atualizam por `npx skills add <origem>@<skill>` sem `-g`, que fixa origem e hash no
`skills-lock.json`; o atalho de skill nova só entra no git com uma linha `!.claude/skills/<nome>`
no `.gitignore` (lá fica só o que foi escolhido para o repo). No Windows os atalhos só viram pasta
com `core.symlinks` ligado. Rust:
`rust-crate-architecture` decide crates, camadas e ciclos, com os crates em `crates/`;
`rust-code-style`, visibilidade e módulos; `rust-workspace` é leitura de apoio e, quando
diverge, vale a `rust-crate-architecture`.

**Instalar/atualizar numa máquina** (após `git pull`):

```bash
./scripts/install-hangar-send.sh          # symlink ~/.local/bin/hangar-send + skills/* + bloco "Sessões-irmãs" no ~/.claude/CLAUDE.md (idempotente)
./scripts/install-claude-wrapper.sh   # symlink ~/.local/bin/hangar-engine + wrapper claude-engine — sem isto,
                                       # motor configurado pelo celular abre um pane que morre na hora
                                       # (tmux new-session ainda retorna 0, o app reporta sucesso calado)
systemctl --user restart hangar-backend.service   # API de pareamento/preview
npm --prefix frontend run build                          # só se o front for servido estático (vite dev pega via HMR)
```

Sessões Claude já abertas não releem o CLAUDE.md global — só as novas conhecem o hangar-send.
Escopo: recados, pareamento, `--group` e `--list` alcançam OUTROS servidores via
endereço `servidor::sessao`: `backend/peers.json` (id → base_url+token, gitignored;
ver `peers.json.example`) + `CP_SERVER_ID` no `backend/.env`. Peer com `"enabled": false` sai da
VARREDURA (painel e `--list`) mas segue endereçável por `servidor::sessao` — é pra máquina que
você sabe que está desligada, senão cada poll paga o timeout de 4s esperando ela (id desta máquina, endereço de
resposta do `[de: id::sessao]`). Recados usam o script; o pareamento remoto passa pelo backend
local e pela máquina dona do grupo (`/api/groups/{gid}/*`); `/pair-remote` só atende máquina
ainda na versão antiga.

## SSE event model

The frontend `EventSource` (`screens/Chat.svelte`) listens for:

- `message` — transcript events: `user_msg` / `assistant_msg` / `tool_use` / `tool_result`.
- `state` — live state + status line (model / context / cost / rate badges).
- `preview` — live in-flight assistant text (full-replace; dropped when the real block commits).
- `ask_question` — opens the native AskUserQuestion sheet.
- `ping` — liveness heartbeat; resets a 25s watchdog that reconnects on half-open connections.
- `reset` — transcript swapped (e.g. `/clear`) → wipe and reload history. Troca de provider
  também refaz parser, monitor e prévia via `__reprovider__`, sem esperar os polls de troca de
  arquivo. O drain resolve o adapter na hora. Ao mexer nesse fluxo, leia a
  [evidência de troca de provider](docs/decisoes/harnesses.md#troca-de-provider-durante-o-sse).

## Regras

Cada marcador abaixo é **a regra**. A medição que a sustenta — data, versão de CLI, o bug que
ela evitou, o número que a provou — está em `docs/decisoes/<arquivo>.md`, na entrada de mesmo
título. **Antes de contrariar uma regra, leia a entrada dela**: quase todas as que estão aqui
custaram um bug caro, e o arquivo de decisões diz qual.

Decisão nova entra assim: a regra aqui, a medição em `docs/decisoes/<assunto>.md`. Em Harnesses,
Windows e Instalação a regra entra na seção "Regras vigentes" do próprio doc, e aqui fica só o
gatilho de leitura: essas áreas são tocadas em poucas sessões e pesavam em todas. Mecanismo que
deixou de existir vai para [`docs/decisoes/superado.md`](docs/decisoes/superado.md) — fica
registrado, fora do caminho de leitura, para não competir com o que vale hoje.

### Frontend — telas, CSS, i18n → [`docs/decisoes/frontend.md`](docs/decisoes/frontend.md)

- **Duas interfaces, não uma: front web (`frontend/`, Svelte) e app nativo (`mobile/`, Expo).**
  Lógica (API, formatação, parser, tipos) entra em `packages/core` e serve as duas. Tela é por
  interface, escrita duas vezes. O check completo, quando pedido, é `npm run check` **na raiz**,
  que cobre as três.
- **Melhoria visual ou recurso que o usuário vê entra no desktop nativo (`desktop-native/`) e no
  app Expo (`mobile/`), quando faz sentido nos dois.** O nativo costuma estar à frente e é a
  referência de comportamento. Quem implementa num cliente leva ao outro no mesmo trabalho ou deixa
  a pendência dita no relato.
- **A web (`frontend/`, PWA incluído) não recebe recurso novo: fica com o que já tem.** Desktop
  evolui no Rust (`desktop-native/`) e celular no app `mobile/`. No Svelte entra só conserto do que
  já existe e o necessário para ele continuar funcionando com o servidor. O Electron e o desktop
  web estão em descontinuação: preserve o que existe; remoção exige pedido explícito. A paridade
  entre a web e os outros clientes deixou de ser requisito.
  O nativo não importa `packages/core`; compartilha a API do backend e `messages/*.json`.
  Decisão e alcance em [frontend.md](docs/decisoes/frontend.md#a-web-fica-com-o-que-já-tem-30102026).
  O nativo é o único app de desktop do instalador: ele o baixa da release `native-latest` no
  pacote da máquina (`scripts/install-native.sh`/`.ps1`); sem o nativo, a reserva é o navegador.
  O instalador não instala mais o Electron. No Windows e no Linux o nativo atende o
  `hangar-preview` e a tela remota do navegador no celular (um navegador por sessão, sem abas,
  repasse em `/cdp`): no Windows por CDP dentro do WebView2; no Linux por um Chromium sem janela
  com CDP por pipe, pintado no painel pelo screencast. Os passos `2026-10-02-electron-removido-*`
  tiram o Electron de quem ainda o tem. No macOS, quem já o tinha fica com ele.
- **Mudança numa lista ou barra de sessões é conferida em todas as outras.** Nativo: barra
  lateral (cheia e trilho), abas em cima ou embaixo, que não têm barra lateral, com a bandeja
  "Máquinas e convidados", seletor de sessões e busca. Expo: lista, barra do tablet e seletor. A
  `SessionList` do PWA entra só para conserto, pela regra acima. Cada uma recebe a mudança ou
  fica fora com o motivo dito no plano.
  [Motivo](docs/decisoes/frontend.md#mudança-numa-lista-de-sessões-vale-para-todas-31102026).
- **A web existente tem duas vistas (820px):** `Sidebar` (desktop legado) e `SessionList`
  (mobile). Nenhuma das duas ganha recurso novo.
  Preserve as duas ao tocar código compartilhado. Lógica da lista vai
  no `lib/sessionListModel.svelte.ts`, e a agregação SSE no `lib/sessionsStore.svelte.ts` — uma
  por servidor, nunca uma por card.
- **Todo texto de interface vem de `m.<chave>()`** (Paraglide). `pt.json` e `en.json` no mesmo
  commit. Dado do servidor não vira chave. A trava `i18nGuard.test.ts` só desce. Rótulo de
  stub/fixture de teste que vive em árvore varrida pela trava é identificador (`abrir-term`),
  nunca frase. O Paraglide compila com `locale-modules` em todo lugar (scripts e `vite.config.ts`):
  um módulo por mensagem deixava os testes 3 a 4 vezes mais lentos.
  [Medição](docs/decisoes/frontend.md#paraglide-compila-um-módulo-por-idioma-11102026).
- **Markdown NUNCA aparece cru.** Todo `.md` exibido passa por `renderMarkdown`. Um `<pre>` com
  `**` e `##` à mostra é bug, não estilo.
- **Tela ou lista nova trata os quatro estados: carregando, vazio, erro e sucesso.**
- **O padrão de criação marcado no desktop é do servidor.** Modelo, esforço e permissão são
  publicados em `/api/session-defaults`, por provedor, motor e conta do motor; o APK lê o mesmo
  registro. Importar o arquivo local antigo nunca sobrepõe um padrão publicado ou removido.
  Falha de leitura bloqueia a criação e mostra o erro. Evidência em
  [frontend.md](docs/decisoes/frontend.md#padrões-de-criação-compartilhados-com-o-apk-19102026).
- **Config e opção moram em MODAL, não em painel docado.** Só o `DesktopSessionContext` fica
  docado. Tela de config usa **container query**, nunca media query — quem aperta a linha é a
  largura do painel.
- **Configurações do Expo usam listas de ajustes agrupados.** Cada linha mostra o nome e o
  valor atual; o toque edita ou abre os detalhes. Agrupe opções relacionadas, com rótulos únicos
  e explicações curtas quando ajudarem a decidir. Ações frequentes ficam à vista; diagnóstico
  e manutenção entram em telas próprias. Preserve as funções ao reorganizar, sem cartões por
  informação, cabeçalhos duplicados ou botões repetidos em cada item. Ao melhorar outras
  configurações, siga a [direção aprovada em Servidores](docs/decisoes/frontend.md#configurações-do-expo-listas-de-ajustes-agrupados-23102026).
- **Transparência é padrão, não enfeite.** Superfície dentro de painel de vidro é `transparent`;
  precisando de material próprio, `--surface-raised` / `--surface-inset`. `--bg-elevated` cru só
  para realce de estado. Verificação: ligue um papel de parede e procure retângulo opaco.
- **Bloco com overflow próprio é `position: relative`.** Um `.sr-only` absoluto dentro dele vaza
  para a área rolável da conversa e trava o chat no fim.
- **Animação de `transform` NUNCA em `<svg>`/`<g>`/`<path>` — só em elemento HTML.** O Chromium
  não compõe isso e cada quadro repinta a página inteira. Compor é aninhar spans.
- **iOS: nada de `backdrop-filter`/`transform`/`translateZ` no vidro da NavBar/Composer.**
  Promovem camada que renderiza preto durante o scroll.
- **A lista de mensagens é janelada** (`WINDOW=120`). Não renderize o transcript inteiro.
- **O app renderiza `AskUserQuestion` nativamente**; use à vontade. Texto numerado é só fallback.
- **Mods no nativo começam minimizados e têm altura estável entre abas; Minimizar é local.**
  Perguntas ocupam o mesmo espaço do mod e só devolvem o painel quando a interação termina. Minimização manual, aba e
  rolagem pertencem à sessão. Evidência em [frontend.md](docs/decisoes/frontend.md#mods-altura-estável-e-pergunta-no-mesmo-espaço-16102026).
- **Tour, dicas e "Mostrar no app" do nativo são balões presos a peças reais, pelo módulo `tour/`.**
  A peça se registra com `popup::anchor` e um id de `tour/anchors.rs`; a posição vale só enquanto a
  área que a desenhou não redesenhou sem ela, e é lida no render da raiz. O balão avança pela ação
  real no ponto do código onde ela acontece, nunca por polling. O que foi visto fica em `tour.json`
  e em `/api/tour-progress` de cada servidor (junção que nunca desfaz). O primeiro acesso de um
  celular chega pelo parâmetro `device` da lista ao vivo do dono, lido só no ramo do dono; o aparelho
  que entra pelo Connect com credencial própria também é registrado. Evidência em [frontend.md](docs/decisoes/frontend.md#tour-de-primeiro-uso-do-nativo-30102026).
- **Mensagem esperando na fila do Hangar aparece no painel "Na fila", não como bolha.** A lista e a
  ordem vêm do evento `queue`; o modelo é `packages/core/src/queuePanel.ts` (o nativo repete em
  `desktop-native/src/queue_panel.rs`).
- **Dedup de fila/pending é delicado** — bolhas `pending`/`queued-` reconciliam por texto
  normalizado contra o transcript real.
- **Vite HMR pode servir componente VAZIO** (stub <1KB, sem erro no console): a tela some sem
  nada quebrar. Remédio: reiniciar o serviço do front. Verificação de front SEMPRE inclui abrir
  a tela e confirmar que montou.
- **Fechar uma sessão e criar outra com o MESMO nome**: a chave do `Chat` e a marca de exclusão
  não podem ser só o nome — sem a época de recriação, a conversa da morta fica montada; sem o
  `jsonl` na marca, a nova nasce escondida para sempre.
- **Terminal real no rodapé e no celular**: um PTY por WebSocket, backend não interpreta nada.
  Um painel por sessão em todas as portas; xterm com fundo `rgba(0,0,0,0)`, nunca `'transparent'`.
  Com o painel aberto, quem conta linha de pane responde 409 — e o app tem que MOSTRAR esse texto.
  Com o Rust de pé, o PTY é só dele (Windows incluído): o `termsock` faz a porta de entrada de quem
  chega ao Python (convidado) e liga os bytes a `/__hangar_server/term`; o 409 pergunta
  `term.active` e erro da ponte é 503 (fechamento 1013 no socket), nunca PTY no Python.
- **WebSocket do dono com `?token=` não confere Origin** (terminal, voz e navegador, Rust e
  Python). A Origin (`_origem_aceita`, `CP_TERM_ORIGINS`) só barra credencial que o navegador manda
  sozinho: hoje, o login de convidado no `termsock`. [Motivo](docs/decisoes/plataforma.md#websocket-do-dono-com-token-não-confere-origin-17102026).
- **Aba ativa do navegador embutido é UMA só, compartilhada entre painel e CLI**; `--aba` age em
  outra sem trocar o que está na tela. O sidecar do navegador é ADITIVO: `url`/`targetId` no topo
  são os da aba ativa, e é só isso que o backend lê.
- **O shell autenticado mantém a lista SSE mesmo com janela estreita ou outra tela aberta.**
  Pedidos de navegador de outras sessões chegam por ela; reutilize o `sessionsStore` compartilhado.
- **Aba escondida que NAVEGA para de compor quadro, e aí o Chromium engole mousedown e keydown.**
  Reemitir a mesma medida não ressuscita; quem reancora é uma medida DIFERENTE (ou um `shot`). Por
  isso `click`/`press` conferem a entrega com ouvinte em captura, reancoram, tentam UMA vez e só
  então respondem `erro:` — resposta de sucesso sem o evento ter chegado é o que custou uma
  sessão inteira de investigação. A sonda do clique escuta `pointerdown` antes de `mousedown`:
  `preventDefault` no primeiro suprime o segundo (todo combobox do Radix), e sonda só de
  `mousedown` fazia a retentativa alternar o componente de volta. Antes de concluir que a página
  não reage, prove com `document.addEventListener('click', …, true)` que o evento chegou.
- **Aba escondida NÃO pode navegar congelada**: o documento `frozen` é descartado na troca e leva
  junto a sessão do `webContents.debugger` (`Not attached to an active page`, e o navegador só
  volta com `close` + `open`). Quem for navegar abre a janela `navegando(true)` ANTES do
  `loadURL` e fecha no `did-stop-loading`.
- **Aba que NASCE com a sessão fora da tela vem 0×0**, apesar da skill prometer 1280×800
  (`main.cjs:688` assume visível quando não há aba anterior). Saída: `layout 1280 800`;
  `layout desktop` não serve, porque limpa a emulação. Confira com `eval 'innerWidth'`.
- **Arrastar sessão sobre sessão (Sidebar/Board/Canvas/celular) abre diálogo, nunca pareia
  direto**, porque parear funde os grupos inteiros e sair avisa quem ficou — sem desfazer. No
  Canvas o alvo é só a faixa de cabeçalho do card (tiles se sobrepõem livremente); no celular a
  alça mora na trilha do swipe; em tablet na largura desktop o arrasto HTML5 não responde ao
  toque, e ali o caminho é o `PairSheet` (também a alternativa exigida pela WCAG 2.2 SC 2.5.7).
- **Página da conversa no nativo do Linux: o quadro do screencast nunca estica.** Ele é pintado no
  tamanho lógico do `metadata` dele, e o screencast reinicia quando o resize assenta. A densidade do
  quadro é a do processo do Chromium (`--force-device-scale-factor`), nunca a emulada por alvo: as
  páginas têm um Chromium próprio em escala 2, e o painel fica no de escala 1.
  [Medição](docs/decisoes/frontend.md#página-da-conversa-no-linux-quadro-no-tamanho-dele-e-chromium-próprio-em-escala-2-15102026).
- **Página da conversa no nativo do Windows: o quadro é desenhado em fator inteiro, `ceil(escala × 2)`.**
  O dobro exato gasta menos pixels e deixa o texto pior em tela a 125%. Textura perdida com o device
  reinicia o screencast; o último quadro não fica guardado.
  [Medição](docs/decisoes/frontend.md#quadro-em-fator-inteiro-e-sem-guardar-o-último-quadro-10112026).
- **Bandeja do nativo: fechar ESCONDE a janela, nunca a destrói, e só com bandeja presente.** A tela
  não sobrevive a fechar e reabrir; quem esconde é `Window::set_hidden`, ajuste nosso no GPUI
  vendorizado, chamado de dentro do `should_close` (por isso ele não toca nos callbacks da
  janela). Sem ícone de pé ou sem hospedeiro de bandeja, fechar encerra: o app nunca fica vivo e
  invisível. No Wayland esconder troca os objetos xdg na mesma superfície; só desmapear com
  buffer nulo não volta no Hyprland.
- **App iOS (`mobile/`)** — interface e desempenho. Medição e exemplos em
  [frontend.md](docs/decisoes/frontend.md#app-ios-mobile-interface-e-desempenho-16102026).
  - A borda esquerda (0–20 pt) é o voltar do iOS: gaveta e arrasto próprio começam mais para dentro (`SessionsDrawer` `edgeStart`).
  - Alvo de toque mínimo 44×44 pt; visual menor ganha `hitSlop`.
  - Voltar e setas são `Icon` (`ChevronLeft`…), nunca caractere (‹ ▸).
  - Formulário em folha usa o `KeyboardAvoidingView` do `react-native-keyboard-controller` com `automaticOffset`.
  - Vibração só por `src/ui/haptics.ts` (iOS): seleção ao marcar, sucesso/erro no resultado; `toast.ok`/`toast.erro` já vibram.
  - Tela simples usa o cabeçalho nativo da pilha (`useGlassHeader`); chat e rota em `formSheet` usam cabeçalho local.
  - Conteúdo sob cabeçalho transparente: ScrollView com `contentInsetAdjustmentBehavior="automatic"` no iOS; o resto recua por `useHeaderHeight`.
  - Tamanho de texto vem de `theme.base.text.*`, mínimo 11 pt; `fontScaleGuard.test.ts` barra número solto.
  - Ícone Lucide novo ganha SF Symbol em `src/ui/sfSymbols.ts` quando houver um de mesmo sentido.
  - Texto ao vivo (prévia) publica no máximo a cada ~50 ms; nunca um render por token.
  - Composer e ChatHeader ficam em `memo` com callbacks estáveis; quem lê a prévia é só quem a desenha.
  - Animação contínua para fora da tela e com a gaveta fechada. Na thread de UI, sem trabalho
    por quadro no JS, acompanha a taxa da tela; animação que passa pelo JS fica em no máximo ~20 fps.
  - Lista, folha e vidro escondidos não montam: nascem na primeira abertura.
  - Linha que não mudou mantém o mesmo objeto (store e lista): evento novo não redesenha as outras.
- **App mobile chega por OTA (expo-updates), canal `preview`, iOS e Android.** Mudança só de
  JavaScript/assets em `mobile/`, `packages/core` ou `messages/` vai por OTA, sem build: o workflow
  `Mobile OTA` publica sozinho quando entra na main (`[skip ota]` no commit pula). À mão, de
  `mobile/`: `EXPO_TOKEN=… npx eas-cli@latest update --channel preview --environment preview
  --platform all --message "…"`. Mudança nativa (pacote com código nativo, permissão, plugin,
  versão do Expo) não vai por OTA: sobe `runtimeVersion` (`native-N`) no `mobile/app.json` e gera
  binário novo (iOS: `ios-dev.yml` Release + SideStore; Android: `eas build --profile preview`).
  Nunca publicar OTA de um push que falhou. Passo a passo em `mobile/README.md`.

### Harnesses — Claude, Codex, Pi, omp, Kimi → [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md)

**Antes de mexer em** `backend/app/adapters/` (claude_headless, codex, kimi, omp, pi), `codex_*.py`,
`engines.py`, `model_picker.py`, `permission_mode.py`, `skill_bridge.py`, `omp_dirs.py`, `loop.py`,
`terminal_input.py`, `state.py`, `crates/hangar-server/src/runtime/` (atores Claude/Codex,
lançamento, relançamento e religação ao mesmo cano em `cano_reconnect.rs`), statusline/prévia, hooks de estado, ou em sessão sem terminal:
**leia a seção "Regras vigentes" de `docs/decisoes/harnesses.md`**. Cada regra ali já custou um
bug calado.

### Windows → [`docs/decisoes/windows.md`](docs/decisoes/windows.md)

**Antes de mexer em** `install.ps1`, `scripts/*.ps1`, `procinfo.py`, `atomico.py`, psmux, encoding de
arquivo gerado, `subprocess`/código de retorno, ou teste que simula `os.name`: **leia "Regras
vigentes" de `docs/decisoes/windows.md`**.

### Instalação, atualização e serviços → [`docs/decisoes/instalacao.md`](docs/decisoes/instalacao.md)

- **Reiniciar o backend**: sem `--reload`; mate `-9` o pid da porta e suba destacado. No Linux é
  `systemctl --user restart`.

**Antes de mexer em** `install.sh`/`install.ps1`, `scripts/install-*`, `atualizar.py`,
`atualizacoes.py`, `docs/atualizacoes/`, `VERSION`, `frontend/dist` servido pelo backend, na
criação de sessão sob escopo do systemd ou no pacote por versão (`crates/hangar-package`,
`crates/hangar-server/src/update/`, `scripts/montar-pacote.py`, `scripts/migrar-pacote.*`,
`scripts/hangar-python`, `~/.hangar/config`): **leia "Regras vigentes" de `docs/decisoes/instalacao.md`**.

### Plataforma — nomes, pareamento, planos, ditado → [`docs/decisoes/plataforma.md`](docs/decisoes/plataforma.md)

- **TTS, narração, tradução de pensamento/descrições e sugestão de tarefa do grupo são do Rust.**
  `speech/` lê a configuração viva diretamente, preserva o cache de áudio e atende as rotas sem
  bridge ou motor Python. A voz ao vivo também não conserva transporte privado para o Connect do
  Python. A configuração geral mantém seus campos e escritor compartilhado; a capacidade
  de tradução é calculada no Rust. Contrato e capturas em
  [plataforma.md](docs/decisoes/plataforma.md#leitura-narração-e-traduções-no-rust-29102026).

- **Orquestração: um time por trabalho, editado pela tela ou pelo LLM.** Sem grupo usa
  rascunho da identidade atual; não importa time padrão/contrato anterior. Ao lançar, associa
  esse registro ao grupo por `/orq/grupo`, preservando escolhas. O vigia fecha executor e
  revisor concluídos; desarmar somente após conferir a limpeza. Evidência em plataforma.md.

- **Provas de orquestração usam temporário fora da pasta durável.** `orq proof` isola builds,
  caches e sandboxes; só resultados publicados permanecem no histórico. Depois da retrospectiva,
  `orq podar --apply` remove os diretórios regeneráveis de execução encerrada, e `--check`
  confere a limpeza. A atualização poda também execuções antigas válidas e registra as que
  preservou. Contrato em [plataforma.md](docs/decisoes/plataforma.md#armazenamento-das-provas-de-orquestração).

- **O nome antigo (`claude-pocket`) só existe em ponte de compatibilidade.** Escreva com o nome
  novo; nunca leia o antigo em código novo. O que resta é migração, e ela nunca funde duas pastas.
- **Comentário explica o PORQUÊ, e é curto.** Medição, versão de CLI e data envelhecem: num
  comentário viram afirmação falsa que ninguém revisa. Elas moram em `docs/decisoes/` e na
  mensagem de commit, datadas por construção.
- **Identificador NOVO é em inglês; comentário e texto de tela continuam em português.** Vale
  para função, variável, classe, campo, nome de tool MCP, chave de evento e arquivo novo. O que
  já existe em português FICA: este repositório nasceu assim, e renomear em massa não é tarefa —
  só entra no rename o que você já ia mexer por outro motivo. Nome público que muda (tool, rota,
  evento) leva ponte para o nome antigo enquanto houver sessão viva que o carregou no catálogo,
  e a ponte sai do catálogo para não cobrar contexto de quem abre depois.
- **Grupo: protocolo reinjetado no `SessionStart`, saída por UMA esteira, anti-loop no backend.**
  Só quem estava sem grupo recebe o protocolo, sem lista de membros; entrada, saída e troca de
  tarefa não acordam ninguém, e o grupo se consulta (`sessions`, `--list`). Grupo `orq` não recebe
  nem o protocolo: o kick-off traz canal e contrato. O texto é do Rust (`groups/text.rs`) e o hook
  é o `hangar-server pair-context`, sem rede, que responde com o servidor parado; só o Claude
  reinjeta.
  Varredura de sessão morta confirma ausência por TEMPO, nunca por número de polls. Aviso do app
  sai como `[painel: <rótulo com espaço>]`, nunca `[de: …]`: o modelo responde a quem assina.
- **Grupo de 1 só existe no grupo `orq` com execução `auto` viva.** Viva = `orq.json` com `auto`
  e sem `execucao_fim`, nunca o batimento do vigia; acabada (ou não iniciada em 1 h), a varredura
  do `list()` dissolve o grupo e arquiva o contrato.
- **Sessão orq: a conversa e o painel saem dos arquivos da execução, por um parser só**
  (`orchestration/timeline.rs`). O `ChatEvent` leva `orq` (texto cru mantido); o painel é um `GET` por
  execução em `/api/sessions/{name}/orq/panel`, fora do alcance do convidado e sem escrita, e o
  Time lê o estado da lista de sessões. O `orq.py` só grava o que não dá para derivar.
- **Modelos Pi/omp/Kimi, sincronização omp e backend de orquestração pertencem ao Rust.**
  `provider_models/` decide catálogo, troca e confirmação no agente, incluindo o inbox Pi;
  `omp_sync/` reconcilia plugins e contexto e conduz o sincronizador em background;
  `orchestration/` lê execuções, watchdog, política, time, plano, timeline e painel e lança pelo
  lançador único. O motor externo da skill usa esses leitores pelo CLI Rust. As portas do Python
  só autenticam e repassam; sem Rust, respondem 503, sem implementação de reserva. Os fatos
  internos `orq/*` foram removidos. [Contrato](docs/decisoes/plataforma.md#provedores-e-orquestração-no-rust).
- **Worktrees mescladas usam a origem e a base publicada.** Base explícita vence; sem ela,
  referência nomeada de criação no reflog, com fallback para a branch principal. Base local usa
  seu upstream ou o único remoto correspondente. Ambiguidade e falha de leitura impedem marcar
  mesclada; upstream apagado nunca comprova merge. Pontas iguais continuam protegidas. Evidência
  e limites em [plataforma.md](docs/decisoes/plataforma.md#worktrees-mescladas-usam-a-origem-e-a-base-publicada).
- **Worktree registrada com pasta existente sem `.git` é residual.** A identidade vem da
  administração do repositório principal; apagar exige confirmação de todos os arquivos
  remanescentes e preserva a branch. Nunca entra na limpeza automática. Registro só sai depois
  da pasta, e interrupção admite retomada. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#recuperação-manual-de-worktrees-residuais).
  Descoberta global incompleta avisa sobre repositórios ainda não encontrados; não bloqueia a
  limpeza de worktrees conhecidas, cuja identidade, arquivos, locks e uso são reconferidos.
- **Plan progress lê o `.md` do plano**, sem arquivo de estado: blocos cercados são removidos
  preservando offsets. A lista, as rotas, o bastão e a prévia do plano nativo usam os leitores
  Rust e o mesmo rastreador da lista, para o painel e a linha elegerem o mesmo plano.
  A decoração e o parser Python foram retirados. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#plan-progress).
- **Ditado: a transcrição não é o problema, o que vem depois é.** Vocabulário vai para a Whisper
  (consertar depois é impossível por construção); regra de prompt só funciona com exemplo de
  entrada e saída; raciocínio no modelo de limpeza piora e não é calibragem. Quem manda no estilo
  é a pill que a pessoa leu antes de falar, não a config.
- **Transcrição, organização do texto e leitura são capacidades separadas.** A tela não leva nome
  de provedor no rótulo da transcrição; endpoint próprio de áudio nunca compartilha sua chave com
  o LLM padrão, e controles de voz só aparecem dentro do provedor que os oferece.
- **Organização do ditado nasce desligada e pertence ao Rust.** None conserva o STT; harness usa
  conta e modelo de organização próprios da conversa de destino; API externa usa sua chave própria.
  Falha conserva o cru e mostra a causa, sem reserva Python ou troca de conta/modelo. Referência das
  três últimas mensagens é opt-in e congelada por tentativa. O executor mínimo não leva ferramentas,
  plugins, skills, hooks, MCP nem instruções globais ou do projeto; Claude usa `--safe-mode` com
  a autenticação original da conta de destino, inclusive a assinatura OAuth. O SDK Claude acrescenta
  metadados de ambiente e identificação da conta; a captura não contém literalmente só o texto do Hangar.
  Contrato e prova em [plataforma.md](docs/decisoes/plataforma.md#organização-opcional-do-ditado-no-rust).
- **Transcrição é do Rust; Enviar encerra o ditado.** Configuração local aponta para executável e
  modelo instalados pelo usuário na máquina do servidor selecionado. O Hangar inicia e encerra
  o whisper.cpp em loopback, sem instalar componentes. Texto, imagem, arquivo e ditado passam
  pela mesma parada, esperando transcrição e organização antes de enviar. Ditado sem sessão,
  reorganização, catálogo e status/teste de provedores são atendidos só pelo Rust: as portas do
  Python não os repassam e não há motor STT de reserva. A tela web segue a mesma regra. Contrato e evidência em
  [plataforma.md](docs/decisoes/plataforma.md#transcrição-no-rust-e-envio-encerra-o-ditado).
- **Cota tem cache em disco e respeita 429**: restart do backend não relê todas as contas, e
  fonte que levou 429 espera 10 min antes de insistir.
- **Redefinição guardada do Codex só vale com a janela de 7 dias em 100%.** O backend relê a cota
  antes de consumir, usa UUID idempotente por tentativa e força nova leitura após resultado definitivo.
- **Registro de peer nunca grava um endereço loopback, e endereço torto tem frase própria.**
  `127.0.0.1` gravado no outro lado aponta para ele mesmo: a volta bate nele, volta com o nome
  dele e o par falha PARECENDO registrado. Loopback → o endereço sai de `/api/alcance` do dono.
  `estranho` (atendeu OUTRA máquina) é tipo próprio antes do `parcial` e abre a correção; e
  "sem identificador" separa token recusado, máquina fora do ar e nome vazio — este último com
  campo no detalhe de qualquer servidor com token aqui.
- **Servidor que não responde sai da lista de sessões e volta a ser procurado automaticamente**:
  espera de 2 s, 5 s, 30 s, 1 min, 2 min, 4 min, 5 min, 10 min e depois 30 min, mantendo esse
  teto. A primeira queda NUNCA espera 30 s: backend reiniciando volta em segundos. O servidor
  ativo e o que serve a página nunca recebem prazo: tentam em 1, 2, 4, 8, 16 e 30 s.
  Prazo e contagem são persistidos para sobreviver à recarga.
  Só uma resposta confirma a recuperação; expirar o prazo apenas permite nova tentativa.
  Erro HTTP e cancelamento da página não são queda de rede. Reconectar permite tentar antes.
  Quem respondeu nas últimas 24 h para em 30 s (2 s, 5 s, 30 s…) e é tentado na hora quando o app
  abre ou volta a ficar visível: a queda dele é a suspensão do aparelho, não a máquina. No Expo,
  com três falhas seguidas, servidor inativo segue no prazo ao voltar e não abre SSE após erro de
  rede. O diário também recua por destino sem rede, até 30 min. Com outro servidor respondendo, esse
  inativo fica parado (`parkServer`): nada automático sai do aparelho para ele. Só uma resposta, abrir
  o servidor, Reconectar ou uma máquina ligada dizer por `/api/peers/check` que ele voltou o liberam.
  Todos falhando é o túnel do aparelho, e ninguém para.
  Falha com o app em segundo plano não conta (nem marca nem grava prazo): tenta a cada 30 s
  fixos e reconecta todos na volta.
  O aviso vermelho "offline" no topo da lista só aparece nos 30 s após a primeira falha da queda
  (`OFFLINE_NOTICE_MS`) e some enquanto a máquina continua fora; o início da queda tem registro
  gravado próprio, fora do prazo de tentativas (`startOutage`/`endOutage`), e só uma lista boa o encerra.
- **Rede local antes do Tailscale: `baseUrl` é identidade, a rota é `baseOf(s)`.** Toda URL de
  chamada a um servidor sai por `baseOf` (ou `getBaseUrl` do `ApiEnv`); `s.baseUrl` cru ignora a
  rede local calado. O token só vai ao endereço local depois da prova HMAC (`/api/peers/prova`),
  e o local só vence se provar antes de o principal responder (IP local via VPN é mais lento).
- **Atalho No Hangar é uma cópia só por atalho, e de nenhuma sessão.** Dono vazio +
  `@cp_shortcut_key` no multiplexador: `close_all`/lista da sessão não o alcançam; clicar de novo
  reaproveita. A aba dele fica no painel de terminal de toda sessão. O estado chega pelo stream da
  lista (`shortcut_terminals`), nunca por SSE próprio; convidado não vê.
- **Revisão de código:** revisão local e as verificações do projeto antes do PR; no PR, o app
  CodeRabbit do GitHub revisa sozinho (`.coderabbit.yaml`); PR aberto antes da instalação só com
  o comentário `@coderabbitai review`. Os comentários dele são achados a tratar, não ruído. A CLI
  local do CodeRabbit continua fora deste repositório.
- **MCP `hangar` (`/mcp`) é do Rust (`crates/hangar-server/src/mcp/`); identidade do chamador vai no
  cabeçalho e o servidor resolve.** Assinatura da voz vence chave, chave vence pane, pane vence
  nome, pane ambíguo não resolve, nada resolvido é erro (nunca `cli`). O bearer do dono é conferido
  antes do servidor MCP e nunca entra no ambiente do pane: Claude via `headersHelper`, Codex via
  `http_headers`. Tool mapeia 1:1 numa rota que já existe, chamada dentro do processo; o CLI continua
  como fallback e resolve identidade sozinho. O Python não tem rota `/mcp`.
- **O git da sessão é o da worktree onde o agente trabalha (`git_cwd`), não o da pasta de
  abertura.** O Claude Code não grava a worktree irmã no `cwd` do transcript: o sinal são as
  chamadas recentes (`cd`, `git -C`, arquivo editado). Nada na principal tira a sessão da
  worktree: consultar a principal é rotina e faria o rótulo alternar. Arquivos e execução seguem
  no `cwd`.
  Evidência em [plataforma.md](docs/decisoes/plataforma.md#git-da-sessão-segue-a-worktree-onde-o-agente-trabalha).
- **Arquivo citado na conversa é LEGÍVEL e EDITÁVEL; a citação é o consentimento.** Fora da raiz
  da sessão a política de caminho é a citação no transcript verificado, não a raiz — e é a mesma
  para o `GET` e para o `POST` de `/file/text`. Leitura e escrita usam o núcleo Rust:
  digest da leitura, tmp+rename, `.git` fora por componente do realpath. Citação transferida
  exige snapshot, digest e fronteira verificados. Escrita nova fora da raiz entra por aqui,
  nunca afrouxando o `/files/write`.
- **HTML servido como arquivo executa isolado e sem o token na URL do documento interno.**
  Arquivos citados e uploads usam `file_response`; SVG/XML mantêm o MIME com scripts bloqueados.
- **Citação e elemento escolhido vão no texto da mensagem, como blocos `<hangar-ref>`.** O transcript
  é a única fonte. Três leitores seguem os mesmos casos (`packages/core/src/contextRefs.fixtures.json`):
  `packages/core`, `desktop-native/src/context_refs.rs` e `plugins/hangar`; formato novo muda os três e
  os casos no mesmo commit. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#citação-e-elemento-no-texto-da-mensagem-15102026).
- **Mermaid é desenhado pelo `hangar-server` (`POST /api/mermaid`), em PNG, num Chromium sem rede**,
  com a biblioteca embutida no binário; os clientes só exibem a imagem. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#mermaid-desenhado-no-servidor-15102026).
- **Plugins do Claude são do Rust (`hangar-plugins`), pela CLI oficial na pasta principal.** Vale
  para todas as contas da máquina: ligar e remover gravam no `~/.claude/settings.json` (remover
  grava `false`, porque o espelho não apaga nas contas), e toda subida de processo Claude em conta
  do Hangar reconcilia a conta antes. Uma mudança por servidor, numa tarefa que sobrevive ao
  cliente; argumento da CLI sempre validado e depois de `--`. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#plugins-do-claude-na-tela-a-cli-oficial-na-pasta-principal-09112026).
- **O que uma atualização trouxe é gravado pelo servidor NOVO, na subida, e cada aparelho vê uma
  vez.** `update_arrived` compara a versão em uso com a última vista (`~/.hangar/update-news.json`,
  crate `hangar-update-news`) e grava as novidades novas; nunca o executor da atualização, que roda o
  binário antigo. Nativo e Expo leem `GET /api/update/arrived?device=<id>` na abertura, na troca de
  versão e no fim do "Atualizar?", e marcam visto ao abrir o modal; não há lista local. Aparelho novo
  recebe só o que chegou nas últimas 24 h. Servidor sem a rota (404) não mostra nada. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#novidades-da-atualização-automática-gravadas-pelo-servidor-novo-10112026).
- **Busca, Perguntar e Arquivo são do Rust, com um índice de todos os agentes e contas.** A passada
  é por evento de arquivo; palavra inteira vem antes de pedaço (`whole`); o id do trecho é o do
  histórico. Sem Rust, 503 `erro_busca_precisa_rust`. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#busca-perguntar-e-arquivo-no-rust-01112026).
- **Página da conversa mora no Rust e some com a sessão.** `html_render` publica dentro do processo
  do Rust (`pages::routes::publish_value`); lê o dono, convidado não vê. Link e URL do rascunho nunca
  levam token ao transcript; o documento da página nunca tem `allow-same-origin` nem token na URL
  (PWA `srcdoc` com fetch autenticado, casca `blob:`, nativo em contexto de navegador próprio).
  Limpeza só pela varredura contra a lista; conjunto incerto não apaga nada. A fonte exibida,
  copiada ou salva vem do servidor, sem o que ele injetou; link da página e do chat passam pela
  mesma `links::open_link`. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#página-da-conversa-mora-no-rust-e-some-com-a-sessão).
- **Configuração compartilhada leva o conteúdo, e o destino resolve caminho e programa.**
  A API, a aplicação e o pós-aplicar são do Rust (`config_sync/`), incluindo espelhos de skills,
  hooks e o agendamento da integração Codex; não há rota nem executor Python de reserva.
  Caminho vira marcador `⟦HOME⟧`/`⟦CLAUDE⟧`/`⟦CODEX⟧`/`⟦HANGAR⟧` (nunca `{HOME}`); quem envia vence;
  hooks e skills do Hangar, MCP `hangar`, credenciais e o login do `.claude.json` são sempre do
  destino. Regras e motivo em [plataforma.md](docs/decisoes/plataforma.md#configuração-compartilhada-leva-o-conteúdo-o-destino-resolve-caminho-e-programa).
- **Compartilhar sessão: só a porta do convidado (8766, por Funnel 8443 ou pelo site
  `<máquina>--guest.<conta>.hangar.dev.br` do Connect) vai à internet, ela é do Rust e recusa o
  token do dono.** Convites, chaves, porteiro e Funnel moram em `crates/hangar-server/src/share/`;
  Rust fora = 8766 fechada, sem reserva Python. `ShareState::public_guest_url` escolhe o endereço e
  não liga nada (a página e o resgate não têm token); `open_guest_url` abre a entrada e é só de rota
  do dono. Connect vale ligado e sem Tailscale confirmado (`tailscale` não instalado ou sem login), nunca por falha de um instante; com os dois fica o Funnel. O proxy do `frpc` e o site
  do Caddy nascem no primeiro convite (`guest_published` em `connect.json`) e ficam: cada host gasta
  um certificado do limite semanal de todo o `hangar.dev.br`. O `frpc` e o Caddy voltam uma vez, em
  tarefa própria, porque quem pediu pelo Connect perde a conexão ali. O site só entra no Caddyfile
  com a 8766 aberta por este servidor, senão o Caddy entregaria a internet a quem segura a porta;
  a porta abrindo depois volta só o Caddy. O que o Python
  ainda atende recebe o convidado já decidido em `x-hangar-guest` assinado. Confiança = usuário
  do sistema do dono. No servidor de convite, 410 e 401 são "encerrado" (nunca apagar o servidor
  nem abrir login); 502, 503 e conexão recusada são tentar de novo. Motivo em
  [plataforma.md](docs/decisoes/plataforma.md#compartilhar-sessão-a-porta-do-convidado-é-a-única-na-internet).
- **Link de convite pela internet só aparece conferido de fora.** A criação (Rust) espera o nome no
  DNS público e a página do Funnel ou do Connect igual à de `127.0.0.1:8766`; não abriu, revoga e responde 409
  `share_reach_failed` com a etapa. Nativo e PWA não chamam mais `/api/share/reachability`. Evidência em [plataforma.md](docs/decisoes/plataforma.md#link-de-convite-só-aparece-conferido-de-fora).
- **Par externo: o token de entrada é a raiz `Share` de `kind: "pair"`, o de saída mora
  em `external_pairs.json`; os dois arquivos são só do Rust.** O Python pede por
  `/__hangar_server/share` (fechar, renomear, trocar de modo). Só endereço `https://*.ts.net:8443` ou
  `https://<máquina>--guest.<conta>.hangar.dev.br`, sem porta (`<máquina>.<conta>` é a porta do dono e
  recebe recusa, porque o link de par leva o token desta máquina); quem assina o recado é o token
  (`[de fora: alias::sessao]`), nunca o texto; só 410 desfaz o par; nome de sessão de fora é
  `[A-Za-z0-9._-]{1,64}`; chamada à outra máquina nunca segue redirect. Um token de convidado cobre
  várias sessões da mesma máquina. Cada lado escolhe o nível que DÁ (`full`/`view`), guardado como
  filho por sessão do grupo sob a raiz (`Kind::Share` = completo, `Kind::Pair` = ver e recado),
  nunca variante nova de `Kind`; resposta sem `level` = Hangar antigo = `view`. Toda entrada em
  grupo dá `view`; `full` só pela rota do dono `pair-access`, chamada pelos clientes (nunca por
  `hangar-send`/MCP). A âncora sair ou morrer desfaz o par; grupo `orq` recusa membro de fora;
  aviso de grupo atravessa o par com um salto só. Sessão de fora aparece dentro do grupo, nunca em
  seção própria. Motivo em
  [plataforma.md](docs/decisoes/plataforma.md#par-externo-sessões-de-pessoas-diferentes).
- **Sugestão de nome só para o nome que a pasta deu, e nunca aplicada sozinha.** `name_auto` no
  `SessionSpec` marca o nome genérico; o executor da organização do ditado gera a sugestão a pedido
  do cliente do dono; a faixa aplica a rota de renomear. Sem modelo de organização escolhido, o
  modelo sai do catálogo da conta (padrão marcado; no Codex, o do `config.toml`); vazio registra `rename_suggestion_no_model`.
  Sugestão `fraca` (pouco contexto) se refaz na próxima mensagem da pessoa; "Outra" (`?again=1`) refaz
  na hora. A vida da sessão é conferida pela mesma fonte da leitura inicial. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#sugestão-de-nome-para-sessão-com-nome-genérico-28102026).
- **Etiqueta de máquina e conta mora no servidor da máquina** (`/api/tags`, `~/.hangar/tags.json`,
  Rust) e chega aos clientes pelo evento `tags` da lista do dono, já resolvida por `conta` da linha.
  A `conta` da linha e o `id` de `/api/credenciais` saem de `accounts::catalog::resolved`; não crie
  outra canonização de caminho de conta. Cor e ícone são chave, nunca valor de cor. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#etiquetas-de-máquina-e-conta-moram-no-servidor-da-máquina-28102026).
- **Logs pertencem ao Hangar, não à conta.** Use `log_paths.base()`; diário exportável registra
  etapas, códigos e origem da falha. Texto de conversa, credenciais e saídas brutas ficam fora
  dele. O shell Electron também escreve lá (`privado/shell.log`): lançado pelo `.desktop`, o
  console dele vai pro `/dev/null`. Detalhes e compatibilidade em [plataforma.md](docs/decisoes/plataforma.md#diário-de-uso-causa-e-contexto-no-arquivo-exportado).
- **Reportar problema: o servidor monta o pacote e o envia ao worker `hangar-relatorio`.** Rota só
  do dono no Rust (`diag_report/`); dos logs privados só saem campos conhecidos (o log do Python, só
  contagem), e o `scrub` passa em cada parte do texto, antes de qualquer corte, e em cada linha do diário anexo. O diário vai
  completo, compactado, num segundo envio que o worker grava sem abrir. Motivo e invariante em
  [plataforma.md](docs/decisoes/plataforma.md#reportar-problema-o-pacote-de-diagnóstico-vai-ao-dono-27102026).
  Exceção única ao "só contagem": quando um reinício não volta (atualização, a volta dela ou
  reinício avulso) ou um passo de máquina da atualização falha, o executor grava como `update: saída`, com `scrub`, tudo o que explica a falha:
  tempo de cada etapa, cada mudança na resposta da saúde, a saída da versão nova reduzida
  (`startup_output::lines`: traceback, lançador, uvicorn e aviso/erro inteiros; INFO só nível e
  logger) e o retrato do fim do prazo (lançador, pacote, processos do Hangar, tarefa ou unit, porta).
  Atualização que falha manda o relatório sozinha apenas com o consentimento
  da máquina (`~/.hangar/auto-report.json`, perguntado uma vez pelos clientes; sem resposta, nada
  sai); o resultado mora em `.hangar-update/relatorio.json`, fora do `estado.json`. Contrato em
  [plataforma.md](docs/decisoes/plataforma.md#relatório-automático-quando-a-atualização-falha-30102026).
- **Aviso na tela só para ação que falhou ou bloqueio real.** Falha de acompanhamento, dado velho
  ou condição que some sozinha vai ao diário de uso e ao log, nunca a faixa, anel ou linha. A
  classificação é uma só, `problem_gate::diagnostic`; os dois portões de saída (`gate_problem` da
  conversa em `side.rs`, `gate_problems` da lista em `list/bridge.rs`) tiram o código de
  diagnóstico do que publicam e o registram (`rust.problem_hidden`), e ele continua no estado
  interno. No produtor, código de tela vence diagnóstico. Nenhum cliente desenha estado
  "reconectando". `list_error` continua emitido; o cliente só o desenha com a lista vazia.
  Código novo de diagnóstico entra em `DIAGNOSTIC` de `problem_gate.rs`. Critério e tabela em
  [plataforma.md](docs/decisoes/plataforma.md#aviso-na-tela-só-para-ação-que-falhou-ou-bloqueio-real-20102026).
- **Chaveiro trancado é do Rust (`keyring.rs`): estado no evento `keyring` da lista do dono,
  senha só pelo `POST /api/keyring/unlock`.** Só o token do dono, nunca repassado ao Python; a
  senha vai pelo socket de controle do gnome-keyring, nunca por `gnome-keyring-daemon --unlock`
  (com daemon de pé ele sobe outro e não abre nada), linha de comando, log ou disco. Os três
  clientes mostram o aviso por servidor. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#chaveiro-trancado-no-boot-o-rust-detecta-e-o-dono-destranca-de-outra-máquina-26102026).
- **Cofre de senhas: um só, na máquina dona, chave no chaveiro dela; o valor nunca entra em
  transcript, evento, diário nem log.** As outras máquinas pedem pelos peers. Senha **pedir
  sempre** só sai com aprovação do dono num cartão; **liberada** sai direto. O agente usa por
  login preenchido no navegador da sessão, comando com variável de ambiente e saída apagada, ou
  arquivo 0600 de vida curta. Convidado não vê nem aprova. A aprovação não é barreira contra um
  agente decidido: ele alcança o chaveiro. Decisão e limites em
  [plataforma.md](docs/decisoes/plataforma.md#cofre-de-senhas-uma-máquina-dona-chave-no-chaveiro-uso-aprovado-por-senha-30102026).
- **Chave secreta nunca vai no argv do `new-session`.** O escopo leva `--description`, e o
  `ProcessMux::run` passa `*_API_KEY`/`*_TOKEN`/`*_SECRET` por um arquivo 0600 que o pane lê e
  apaga: o systemd grava a linha de comando do escopo no journal. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#chave-secreta-do-pane-fora-da-linha-de-comando-26102026).
- **Uma configuração do Jev para navegador, orquestração, voz, Computer Use e escolha de envio.** `jev_api_key`,
  `jev_endpoint` e `jev_model` no `runtime_config`; no ambiente `TYPESAFE_API_KEY`, `JEV_ENDPOINT`
  e `JEV_MODEL`, já resolvidos. Não existe chave do Jev só para o Computer Use. A regra do destino
  (chave `sk-or-` ou endereço do OpenRouter) tem cinco cópias que mudam juntas: `destino_jev`,
  `destination` em `crates/hangar-server/src/voice/jev.rs`, `jev::destination` em
  `desktop-native/src/app/server_config.rs`, `destinoDoJev` em `shell/jev_objetivo.cjs` (só o CLI
  fora de sessão) e `jev_destination` de `runtime/codex_launch.rs` (ambiente do Codex que o Rust
  lança). Precedência única: runtime-config acima do ambiente. O modelo auxiliar (texto e
  imagem) também é um só, `jev_aux_*`, lido pelo navegador e pelo Computer Use. O hangar-preview
  manda o print ao Jev só quando a decisão sem ele sai fraca (o `confere`, só na faixa de dúvida); o
  Computer Use manda em todo passo; o print vai ao provedor do Jev sem exceção. Pelo OpenRouter a
  imagem só vale como parte `image_url` no `state` (o `assets` da Typesafe é ignorado calado); o
  formato segue o `tipo` do `destinoDoJev` e, no `hangar-computer-control`, o
  `url.contains("openrouter.ai")` de `crates/hcc-laco/src/jev.rs`, cópia que muda junto com a regra
  do destino. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#uma-configuração-do-jev-para-os-quatro-usos) e
  [plataforma.md](docs/decisoes/plataforma.md#jev-vê-o-print-21102026).
- **Hangar Connect entra por `127.0.0.1:8768`, que é do Rust, e nada que chega por ela é local.**
  O porteiro (`crates/hangar-server/src/connect/`) nega por padrão, recusa a senha do dono ali e só
  deixa passar o aparelho com credencial própria, aprovada na máquina; nunca decidir "local" por
  cabeçalho. A supervisão do `frpc` e do Caddy, a configuração e os aparelhos moram em
  `crates/hangar-connect`. O Python não tem porta nem rota do Connect. Cookie de login só autoriza
  `GET`/`HEAD` e, em https, só o `__Host-cp_token`: máquinas do Connect dividem o mesmo site.
  Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#hangar-connect-a-entrada-é-do-rust-e-cada-aparelho-tem-credencial-própria-07112026).
- **Prévia de porta pelo Connect: `127.0.0.1:8769` é um listener próprio (`hangar-connect/src/preview.rs`),
  fora de `hangar_auth`.** O link de uso único abre uma página com botão, e só o `POST` dela o gasta
  e vira cookie `__Host-` por 303, com sessão de 8 h. O projeto nunca vê credencial do Hangar e guarda
  os cookies dele, mas não grava cookie do Hangar, `Domain` nem `Clear-Site-Data`. O destino é o
  `127.0.0.1:<port>` gravado na criação, conferido a cada pedido contra as portas do próprio Hangar, e
  redirecionamento do projeto nunca é seguido. O site nasce na primeira prévia (`preview_published`),
  conferido de fora, e aponta sempre para a 8769; como o do convidado, só entra no Caddyfile com a
  porta aberta por este servidor. `SameSite` não separa as máquinas do Connect: `Origin` de fora é
  recusada, e `Sec-Fetch-Site` de fora também, salvo abrir o site numa aba. Script de service worker é
  recusado. A prévia pelo Tailscale é do crate `hangar-port-preview`, sem caminho Python. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#prévia-de-porta-pelo-connect-a-8769-é-um-listener-próprio-fora-de-hangar_auth-07112026).
- **Cookie do OpenCode, webhook de deploy e prévia Tailscale são do Rust.** Os crates
  `hangar-opencode-cookie`, `hangar-deploy` e `hangar-port-preview` atendem os caminhos inteiros;
  o cookie invalida a cota em processo e a prévia só altera o slot 10000. Não há rotas ou pontes
  Python dessas responsabilidades. Contrato em
  [plataforma.md](docs/decisoes/plataforma.md#cookie-deploy-e-prévia-tailscale-no-rust-11112026).
- **Nada nasce no Python: o objetivo é tirá-lo do projeto.** Rota, ponte, poller, integração e
  qualquer recurso novo vão para o Rust (`crates/`). Sem pedido explícito do usuário, no
  `backend/app/` só se apaga código. Toda linha que ACRESCENTA no Python — lógica, releitura,
  cache, ponte, rota, repasse ao Rust, condição de modo, compatibilidade — é perguntada ao usuário
  ANTES de escrever, dizendo o que é, por que o Rust sozinho não basta e o que se perde sem ela.
  Vale para migração, correção e subagente (que para e pergunta pela sessão). Linha nova no Python
  sem essa autorização é defeito na revisão, mesmo que "só repasse".
- **Módulo Rust novo nasce como crate próprio sobre o `hangar-core`.** Vale também
  para migração do Python. O `hangar-core` existe (`crates/hangar-core`): todo módulo novo nasce
  como crate próprio sobre ele, e o servidor só o liga (rota, estado). O que o módulo precisar do
  `hangar-server` desce antes para o `hangar-core`, ou vira trait no core que o servidor
  implementa. Nenhum crate depende do `hangar-server`. Motivo e plano da divisão em
  [plataforma.md](docs/decisoes/plataforma.md#módulo-novo-nasce-como-crate-próprio-06112026).
- **Motores, login de convidados e peers são do Rust.** Catálogo, CLIProxy, espelhos e instalação
  ficam em `engines/`, `harness_setup/` e `codex_setup/`; identidade, permissões, raízes e lista do
  convidado em `login_guests/`; configuração, descoberta e canal dos peers em `machine_peers/`.
  `guests.json` ilegível conserva a última leitura válida. A administração usa criar, alterar e
  excluir convidados e `/api/me`; não existe `GET /api/guests`. Não há ponte Python dessas rotas.
  A inicialização anuncia saúde antes dos passos demorados, mas bloqueia ações e adoção até
  concluir a preparação. Evidência em [plataforma.md](docs/decisoes/plataforma.md#motores-convidados-e-peers-no-rust).
- **Migrar do Python para o Rust é migrar o caminho inteiro.** Antes de dar a migração por feita,
  procure nos dois lados todo chamador da responsabilidade (rotas `/internal/*`, `*_bridge`,
  `to_thread` que esperam o Rust) e apague o que ficou no Python. O Rust nunca pergunta ao Python o
  que ele mesmo já é dono, e o Python nunca repergunta ao Rust dentro de uma chamada que o Rust
  fez: a volta Rust → Python → Rust prende o pool do Python e trava o backend na subida. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#migrar-é-migrar-o-caminho-inteiro).
- **O navegador remoto da sessão é do Rust (`remote_browser/`).** As rotas
  `/api/sessions/{name}/navegador` e `/api/sessions/{name}/nav-remoto` não têm motor Python:
  CDP, entrada, layout e descoberta são do Rust. As portas Python só autenticam e transportam,
  com o binding nativo do contador compartilhado; sem Rust, HTTP responde 503 e WS fecha 1013.
  Não há callback HTTP de autenticação nem volta Rust → Python → Rust. Contrato e provas em
  [plataforma.md](docs/decisoes/plataforma.md#navegador-remoto-e-contador-compartilhado-de-autenticação-28102026).
- **Pasta `hangar-auth-*` só sai com o marcador `state.alive` velho e sem trava.** Cada
  `SharedLimiter` aberto segura a trava compartilhada nele; a subida do Rust apaga as órfãs
  (`hangar_auth::sweep_orphans`, só Unix). Pasta sem marcador nunca sai; na dúvida, fica. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#pastas-órfãs-do-contador-de-autenticação-10112026).
- **Paleta e papel de parede da máquina são do Rust (`hangar-desktop-theme`).** O SCSS Material
  You e o ponteiro do wallpaper são relidos a cada pedido; no Windows, o ponteiro vem do registro
  do usuário. As duas rotas `/api/desktop/*` exigem o dono e cliente efetivo local. A imagem
  conserva `no-store`, metadados e intervalos; não há leitor ou testes Python desse recurso.
  Transporte compartilhado em `hangar-http-file`. Contrato e provas em
  [plataforma.md](docs/decisoes/plataforma.md#paleta-e-papel-de-parede-no-rust-10112026).
- **Workflows são lidos no Rust (`crates/hangar-workflows/`).** Lista, detalhe da execução e
  detalhe do agente leem os artefatos irmãos do transcript a cada pedido, sem cache por nome.
  As mesmas rotas atendem dono e convidados nos roteadores Rust, com as permissões vigentes;
  não há leitor, handler ou teste Python dessa responsabilidade. Contrato e provas em
  [plataforma.md](docs/decisoes/plataforma.md#workflows-lidos-no-rust-10112026).
- **O que ainda passa pelo Python com o Rust de pé (modo `rust`).** Esta lista é o inventário
  vivo: migração que tira um item o apaga daqui no mesmo commit; item novo só entra com o arquivo
  e a linha. Conferir antes de afirmar que algo "é do Rust" ou "ainda é do Python".
  - Chat dos cinco provedores do dono: nada. `/history`, `/events`, mensagens, estado, prévia, fila, faixa
    `stats` (`stats.rs`), faixa e avisos de mod (`mods`) e pedido de navegador (`nav_requests.rs`)
    são do hub do Rust, sem conexão ao Python. O `sse.merged_events` só serve o `/events` do próprio
    Python no modo `python` de Claude/Codex; o `stats.py` serve esse `/events`. O consumo da orquestração é calculado no Rust.
  - Lista do dono: montada pelo Rust; Pi/omp/Kimi publicam o estado pelos atores em processo,
    e a última resposta vem dos leitores nativos. Não existe `/internal/list/facts`.
  - Outras perguntas do Rust ao Python (`/internal/*`): `accounts/prepare` (só contas Codex; a
    reconciliação das contas Claude é do Rust, `accounts/claude_config.rs`),
    `accounts/codex-invalidate`, `migration/status` e `ping`.
    Transcrição e organização do ditado leem a configuração efetiva diretamente no Rust.
  - Abrir editor, shell separado e terminal externo: do Rust (`hangar-session-tools`). As três
    rotas conservam os clientes existentes; sem Rust, não há lançador Python de reserva. O shell
    continua no multiplexador, com `@cp_hidden`, e usa os fechamentos e renomeações existentes.
    Evidência em [plataforma.md](docs/decisoes/plataforma.md#editor-shell-e-terminal-externo-no-rust-11112026).
  - Partes ainda no Python: transferência Claude→Codex e runners.
  - Mods (`press`/`close`/`show`/`input`), `~/.hangar/plugin.json` e `plugin-dir` publicados na
    subida e o download dos preços do models.dev: nada; são do Rust (`mods/`,
    `config_sync/plugin_publish.rs`, `costs/pricing_refresh.rs`).
  - Atalhos de projeto: identidade, GET/PUT, `shortcut-shell`, `run-code` e reinício são do Rust.
    Lista, fechamento, foco, respostas e WebSocket dos terminais ainda usam os consumidores
    genéricos Python existentes; não consultam Git. O bastão é montado, opcionalmente reescrito,
    gravado e entregue ao lançador pelo Rust, sem montador Python.
  - Git, arquivos e administração de worktrees são do Rust, inclusive contexto, validação,
    autorização, criação, exclusão e limpeza automática. O fetch usa credenciais configuradas,
    mas não pergunta login no terminal. Os handlers Python foram retirados, sem motor de reserva:
    sem Rust, o transporte autenticado preexistente conserva seu 503; onde ele não existe,
    a rota retirada recebe o 404/405 padrão dessa porta. Não existem
    `/internal/worktrees/context`, `/internal/workspace/context` ou a porta privada genérica
    `/__hangar_server/workspace`. Evidência em
    [plataforma.md](docs/decisoes/plataforma.md#contexto-e-fetch-de-worktrees-sem-python-05112026).
  - Arquivo e busca são do Rust. O `archive.py` conserva os leitores genéricos de conversas
    (`list_folders`, `_contas`, `_head_info`, `archive_jsonl`, `archive_cwd`). O localizador e o motor
    Python de worktrees foram retirados; `worktrees.py` conserva leitores genéricos, o mapa de
    redirecionamento e os auxiliares de arquivos ainda usados, sem consultar Git ou interpretar `.git`.
    Não existe `/internal/workspace/context`.
  - Atualização: rotas, passos iniciais, fetch e atualização automática são do Rust.
    O pacote usa `update/http.rs` e `update/periodic.rs`; o checkout usa `update/git_engine/`.
    O `rust_bins.py` lança o Rust pelo caminho real da versão.
  - Tarefas próprias do Python em modo `rust`: os leitores legados de Claude/Codex não observam
    Pi/omp/Kimi. `watch_sessions` do Codex a cada 2 s fica desligada quando todo Codex é do Rust. Travada, limite e queda são avisados
    pelo Rust (`push/watch.rs`); o vigia Python foi apagado e, no modo `python`, esses avisos não
    saem. A retomada dos loops ativos na subida também é só do Rust (`loops/watch.rs`): no modo
    `python` eles não voltam depois do reinício.
- **Qual conversa é a da sessão é o Rust que decide** (`locate.rs`, a descoberta da lista), e a
  troca de transcript só vale lida duas vezes seguidas; provider, modo do Codex ou outra vida da
  sessão trocam na hora. Com o chat ao vivo aberto, o `/history` segue o transcript que o hub
  confirmou. A reabertura em erro também é do Rust
  (`RuntimeRegistry::request_reopen`), sem pedir ao Python. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#a-conversa-da-sessão-é-escolhida-pelo-rust-e-a-troca-espera-duas-leituras).
- **A porta 8765 é do `hangar-server` (Rust); o Python escuta atrás, numa porta de loopback.**
  O Rust atende `/history` e `/events` de Claude/Codex, `/api/costs`, `/api/uso`, `/api/cotacao`, o
  custo de sessão Codex, a busca (`/api/search`, com o índice das conversas), o Perguntar
  (`/api/ask-history`) e o Arquivo (`/api/archive*`) com o token do dono, e o terminal real do dono (só pelo
  `?token=`, sem conferir Origin). Custos e uso têm índice próprio
  (`custos-rust.sqlite3`) no cache local; a faixa `stats` do chat também é do Rust. Contas Claude/Codex e
  cotas são só do Rust: no modo `python` respondem `accounts_need_rust_server`, sem reserva
  Python. Anexos, o `/transcribe` da sessão, a saída ao vivo de ferramenta (`bash-output`,
  `tool-progress`) e a imagem colada no terminal são só do Rust, com a sessão tirada da lista dele:
  as portas do Python autenticam e repassam em fluxo por `/__hangar_server/forwarded`, e no modo
  `python` respondem 503 `erro_midia_precisa_rust`. Só a imagem de conversa transferida
  (`transfer:`) segue no Python. Evidência em
  [accounts-uploads-rust.md](docs/decisoes/accounts-uploads-rust.md#anexos). O resto, convidado
  incluído, é repassado com `X-Forwarded-For`. A 8766 e a 8768 são do Rust. Sem binário
  (`CP_RUST_SERVER_BIN`, `bin/` da versão do pacote, `crates/target/release`, `~/.hangar/bin`), com `CP_RUST_SERVER=0`, com
  `protocol` da saúde diferente de `RUST_SERVER_PROTOCOL`, sem endereço privado válido ou com 3
  quedas em 60 s, um segundo `uvicorn.Server` com `lifespan="off"` assume a porta: nunca um segundo
  lifespan. O processo tem um modo só (`pending`/`rust`/`python`): queda 1–2 deixa as sessões
  Claude sem dono por segundos (`pending`, até 30 s) e o Rust novo as reabre; a desistência as
  passa ao Python, cada uma uma vez, e o Supervisor religa o Rust sozinho com espera crescente
  (`RETRY_DELAYS`, zera só após uma `CRASH_WINDOW` de pé): o Python solta as sessões pela passagem
  normal (`leave_python`, de volta a `pending`; falhou = não sobe), depois a porta, e só então o
  filho nasce; não subiu, nova desistência. Falha determinística só tenta de novo com o binário
  mudado. Nunca duas portas nem dois donos; a saída dos SSE é do sinal real, não do `should_exit`. O segredo interno nunca entra no `os.environ`. Formato de
  `ChatEvent`, ids de evento e contrato interno
  mudam nos dois lados no mesmo commit. Mexeu no contrato interno (rotas `/internal`, canal
  privado do estado, variáveis do filho): suba `RUST_SERVER_PROTOCOL` (Python) e `INTERNAL_PROTOCOL`
  (Rust) juntos. O `versao` do snapshot do `hangar-cano` acompanha o `VERSAO` do `cano.py`.
  Medidas e motivo em [plataforma.md](docs/decisoes/plataforma.md#hangar-server-a-porta-pública-em-rust-o-python-atrás).
  Índice, paridade e medidas de custos em [plataforma.md](docs/decisoes/plataforma.md#custos-e-uso-no-hangar-server).
- **As escritas do dono em sessão Claude e Codex, com e sem terminal, são do Rust.** `/input`
  (`/clear` incluído: o ator do terminal o conduz), `/steer`, `/interrupt`, `/keys`, `/select`,
  `/select/submit`, `/term-input`, `/answer`, o descarte da fila, o painel da fila (`PATCH` e
  `DELETE queue/<id>`, este com `?only_waiting`, `POST queue/reorder` e `POST queue/<id>/steer`) e
  os controles do Codex são decididos no `hangar-server` (`session_write/table.rs`). O `/input` e o
  `/api/broadcast` para nome sem entrada no Rust também são dele (`session_write/unowned.rs`): Codex
  rodando direto no tmux é digitado com prova, só com o campo vazio e pronto (senão 409
  `codex_composer_busy` ou `codex_menu_open`, sem fila); shell ou programa desconhecido, 400 na
  hora; nome inexistente, 404. `model-effort` fora do Claude é 409 `erro_model_effort_so_claude`.
  As outras rotas sem entrada no Rust e as de pane de sessão sem terminal seguem ao Python. Entrada doente é 503
  `runtime_unhealthy`, corpo fora de forma é 422 `erro_corpo_invalido`, nunca repasse. Sessão sem
  terminal religando ao mesmo cano vivo não é entrada doente: a escrita vai ao ator e espera na
  fila até religar. Cada sessão
  tem uma porta de entrada no Rust: o Python a fecha antes de congelar ou transferir a sessão
  (renomear, trocar de conta) e a reabre depois; fechada pela troca de conversa, recusa na hora
  (409), fechada por congelamento curto, a escrita espera. A saúde diz o que o Rust atende em
  `owns` (`table::owned_modes`). No convidado (8766) o Python autentica e repassa
  num sentido, corpo, método e query intactos: `input`/`answer`/`select`/`select/submit` e as rotas
  `queue/*` do Claude a `/__hangar_server/claude/<nome>/<rota>`, tudo do Codex do Rust (`queue/*`
  incluídas) a
  `/__hangar_server/codex/<nome>/<rota>` (a mesma ponte recebe o texto de broadcast para Codex
  com terminal); Pi/omp/Kimi fecham pelos atores Rust; 421 = o Rust não atende e o Python segue, transporte que falha =
  5xx `<provedor>_bridge_failed`. Texto que nasce no Python para o Claude segue em `_send_one`, coberto
  só pelo `freeze`. Contrato e motivo em
  [plataforma.md](docs/decisoes/plataforma.md#escritas-do-claude-no-hangar-server) e
  [fronteira](docs/decisoes/plataforma.md#perguntas-permissões-e-ciclo-de-vida-no-rust-fronteira-com-o-python).
- **Pergunta, permissão e menu exibidos levam `interaction_id`, e a resposta devolve o do cartão
  em que a pessoa agiu.** `/select`, `/select/submit` e `/answer` sem ID recebem 409
  `erro_interacao_sem_identidade`; ID de pergunta trocada ou de outra vida, 409
  `erro_interacao_desatualizada`; vínculo ilegível, 503 `erro_interacao_indisponivel`. Nunca
  associar a resposta à pergunta atual: cliente antigo sem ID é recusado de propósito. O ID vem do
  que foi publicado (`StateEvent`, `AskQuestion`, linha da lista), nunca buscado depois do clique.
  Codex com terminal não informa quem resolveu: a resposta escrita sai 409
  `erro_sem_confirmacao_resposta` e o cartão fecha no `serverRequest/resolved`, sem "permitido" ou
  "negado por você".
- **Abrir, reabrir, retomar e restaurar sessão é do Rust, por um lançador só.** Toda origem (telas,
  voz, `hangar-send`, MCP, `orq`, outro servidor, `claude`/`codex`/`pi`/`omp`/`kimi` digitados no
  terminal) manda um `SessionOpenRequest`; `resolve_open()` decide conta, modo, modelo, plugin e
  com/sem terminal (os padrões que as telas preenchiam moram aqui) e grava o `SessionSpec` da sessão
  em `~/.hangar/session-specs/<chave>.json`. Modelo, esforço e permissão omitidos vêm do padrão
  salvo no servidor (`~/.hangar/session-defaults.json`), que vence a criadora; `inherit: true` traz
  a permissão dela; convidado não recebe. Reabrir é `reopen_session(spec ⊕ SessionChange)`;
  retomar do Arquivo e `/resume` passam pelas mesmas guardas da criação. Com o servidor fora, o
  comando de terminal `hangar-server open` abre igual e o servidor adota depois; `-p`, pipe,
  `--resume` explícito, subcomando e flag desconhecida seguem com o CLI cru. Depois de reboot ou
  queda do tmux o Rust restaura as sessões com terminal pelo spec (`~/.hangar/session-restore.json`,
  comando no `new-session`, nunca digitado no shell); o tmux-resurrect pula as sessões do Hangar.
  Sessão que bate o limite de uso troca de conta sozinha pela reabertura (só Claude). Login recusado
  segue o mesmo vigia: com login no disco reabre uma vez na mesma conta; recusou de novo ou o login
  sumiu, vai à conta logada de mais folga; sem nenhuma, avisa `limit_switch_login_lost`. No modo
  `python`, abrir e reabrir respondem 503 com código, sem lançador Python. Exceções que ainda passam
  pelo Python: texto que nasce nele (broadcast que entra pelas portas do Python) acorda Claude/Codex sem terminal pedindo
  ao Rust, e a transferência Claude→Codex. Decisões e medições em
  [plataforma.md](docs/decisoes/plataforma.md#abertura-de-sessão-única-no-rust).
- **Trocar o nome da sessão é do Rust (`session_rename/`), com desfazer.** Tudo que recusa vem antes
  do primeiro efeito; a troca segura a reserva do nome novo e as portas dos dois nomes; a sessão viva
  fecha sem matar e reabre no nome novo na mesma geração; falha no meio desfaz e diz o que não voltou.
  O Python só repassa (convidado com conta). Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#trocar-o-nome-da-sessão-é-do-rust-23102026).
- **Nova conversa do Claude sem terminal usa uma reserva pré-aquecida** (`crates/hangar-server/src/prewarm/`):
  com a tela aberta o cliente pede `PUT /api/sessions/prewarm` e o create leva `prewarm`. A reserva
  nasce pelo mesmo lançador, sem sidecar até a adoção, com chave, id e nome definitivos; adota só
  com impressão, nome, conta, `config_marca` e pasta iguais, senão descarta e abre frio. Falha não
  vai à tela. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#reserva-pré-aquecida-da-nova-conversa-do-claude-22102026).
- **Sessão oculta e parada além do prazo adormece; ler não acorda, escrever acorda onde estava.**
  A vigia `session_sleep/` só pré-seleciona; o `sleep_*` do `RuntimeRegistry` reconfere sob a porta
  fechada e desiste calado. A marca `asleep` mora no sidecar sem terminal e o spec segue com a
  superfície de antes: acorda com terminal se tinha, também na subida do Rust. Um acordar por vez
  (`waking` + `rewake`), texto pela fila durável. O Codex com terminal acorda com a permissão menor
  entre rollout e sidecar e só adormece sem ninguém ligado depois da última rodada. Claude sem
  terminal segue no estacionamento de 65 min. O cliente só manda `sleep` a servidor cujo `GET` o
  trouxe. Evidência e limites em
  [plataforma.md](docs/decisoes/plataforma.md#sessão-oculta-adormece-e-acorda-onde-estava-03112026).
- **Perguntas, permissões e ciclo de vida da sessão aberta são do Rust.** Rust: plugin do Claude (`/api/plugin/*`, `plugin::Store`), fatos e entrega, modos de
  permissão (sondagem, troca, memória), Codex e Claude com terminal adotados (o Claude pela
  `CP_SESSION_KEY` do pane, que o vínculo do Python também usa), pane morto, fechamento, `/clear`,
  troca de conversa e sidecar, e toda reabertura pelo spec (`claude_reopen.rs`, `codex_relaunch`):
  terminal↔sem terminal, `/recarregar`, `/conta` com a conta ChatGPT fixa, o modelo e o Fast dela, e
  o bypass; o modo vem do spec e do observado, nunca do rodapé lido na hora. A transferência
  Claude→Codex (`/conta` com `credential_id`) segue no Python. Todo `DELETE` do dono fecha no Rust,
  pelo nome quando não há entrada (`runtime/close_name.rs`); processo que sobrevive é 500
  `session_kill_unconfirmed`, com fila, sidecar e convite intactos. Python: autorização das portas dele com o repasse acima e, depois do `DELETE`
  concluído no Rust, só a limpeza externa (`share_store`/`sync_tunnel`, `ThenLink`, atalhos,
  caches), sem chamar o Rust de volta. Responsabilidade migrada nunca volta ao Python em
  `rust`/`pending`: Rust indisponível é erro visível. Modo de permissão gravado desconhecido é
  recusado; ausente é o padrão da abertura.
- **Tempo da sessão parada vem da conversa, não do mtime do transcript.** Com `idle`, `last_activity`
  sai do `ReplyCache` (último evento da conversa); o mtime anda sozinho e impediria ocultar e
  adormecer. O mtime segue como chave dos caches. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#tempo-da-sessão-parada-vem-da-conversa-não-do-mtime-05112026).
- **A lista do dono é do Rust; o estado vem dos atores em processo.** `GET /api/sessions` e
  `/api/sessions/events` do dono saem do `ListHub` (`list/hub.rs`): um produtor por servidor,
  ligado enquanto houver lista aberta, tique de 1,5 s, JSON só quando a assinatura muda; cada
  conexão só lê o publicado. Escrita nas pastas de estado das contas (marcador, registro nativo,
  pergunta aberta) reclassifica só a sessão afetada sobre a última rodada boa, juntada em 150 ms;
  observador que falha vai ao diário e o tique continua valendo. Pi/omp/Kimi usam o estado
  dos atores com identidade da vida e conversa; transferências, `orq`,
  acesso de convidado, atalhos e problema do Claude sem terminal a ponte lê do disco
  (`list/local_*.rs`); sessão sem terminal e Codex com terminal adotado, do
  `RuntimeRegistry`; pergunta segurada e presença do app no Claude, do `plugin::Store`. Falha
  nunca vira lista vazia nem a do Python: 503 com código no `GET`, `list_error` com código no SSE,
  `problema` interno na linha quando só os fatos caíram (a saída o tira e o registra no diário).
  Convidado e outros métodos seguem ao Python.
  Medidas em [plataforma.md](docs/decisoes/plataforma.md#lista-do-dono-no-hangar-server).
- **Os grupos são só do Rust; o Python não tem lógica de grupo.** Sidecars, fusão, contrato,
  texto do protocolo e a varredura de membro morto (ausência por tempo, lista sem fatos não varre)
  moram em `groups/`. O Python só lê o arquivo de grupo e, nas portas dele (`orq`), repassa num sentido pela ponte `groups_bridge`
  (`/__hangar_server/groups`). No modo `python` toda operação de grupo responde 503
  `groups_need_rust_server`; fechar sessão em grupo ainda fecha, com esse aviso. Motivo em
  [plataforma.md](docs/decisoes/plataforma.md#grupos-o-rust-grava).
- **Grupo entre máquinas do dono: uma dona, um `gid`, fila em disco.** Parear com
  `servidor::sessao` junta as máquinas do `peers.json` num grupo só; a máquina dona guarda o
  registro (`.hangar-pair/groups/<gid>.json`) e o contrato, e entrada, saída, troca de nome, aviso
  e contrato passam por ela. Saída e troca de nome vão por `groups/outbox.json` e esperam a
  máquina voltar; aviso de grupo não espera (`pulados`). Rede, 503 `groups_need_rust_server`,
  409 `busy`, 429, 401 e 408 tentam de novo; outro 4xx com código é recusa. Membro de fora só
  alcança a máquina do par; par 1:1 antigo vira grupo sozinho. Sem rede sob o lock de grupo.
  Evidência em [plataforma.md](docs/decisoes/plataforma.md#grupo-entre-máquinas-do-dono-uma-dona-um-gid-e-fila-em-disco-20102026).
- **Sessão filha ligada: `link` na abertura, vínculo no arquivo de grupo da filha.** Só o dono liga
  (`erro_link_convidado`), só local (`erro_link_remoto`), nunca em grupo `orq`. O vínculo é
  `linked_by` no sidecar da filha e vale só com o pai entre os `peers`; o pai sair o apaga. Ligar
  não injeta protocolo de grupo, nem no `SessionStart`. `interrupt` (MCP) só alcança filha ligada a
  quem chama. A resposta final de turno pedido pelo pai volta sozinha como `[de: <filha>]` (só
  Claude e Codex), por uma assinatura interna da lista que não conta como app aberto; sem esperar
  bloqueante. Os clientes calculam a linha "N subagentes" da lista. Motivo em
  [plataforma.md](docs/decisoes/plataforma.md#sessão-filha-ligada-a-quem-a-abriu).
- **A porta privada de loopback do Rust mora no mesmo filho**, anunciada na saúde somente como
  endereço; o segredo vem do Supervisor em memória após conferir o protocolo. Uma captura canônica
  por rodada, sem grade auxiliar: com o Rust de pé, quem captura o pane de Claude com terminal para o
  estado ao vivo é o `Monitor` (abaixo; modo de permissão, entrega e captura avulsa do push também
  são do Rust), e a ponte Python do observador (`terminal_observer` →
  `/__hangar_server/terminal`) ficou sem consumidor, porque o `StateMonitor`/`PreviewBroker` de
  Claude só roda no modo `python`, com a ponte desligada. Pi, omp e Kimi compartilham a captura
  e o estado temporal no ator Rust; Codex conserva estado e prévia nativos. `/clear` ou troca do filho
  descartam leituras antigas; sidecar Claude vazio continua sendo uma resposta; erro de captura
  vira `problema` registrado no diário e no log (não chega à tela), nunca leitura Python.
  Evidência em [plataforma.md](docs/decisoes/plataforma.md#observação-terminal-rust-erro-visível-sem-captura-python).
- **Estado ao vivo de Claude com terminal é do `Monitor` do Rust no modo `rust`/`pending`, em
  qualquer porta.** Um por hub (`side.rs`), nascido com o primeiro assinante: o `/events` do dono
  ou o canal privado `/__hangar_server/state/{name}/events`, que o Python lê para quem entrou por
  ele (login de convidado). Ele publica `state`, `preview`, `ask_question` e `suggest` pelo retrato do hub, com o
  `interaction_id` da pergunta ou permissão em aberto, e drena a fila direto no `RuntimeRegistry`
  (`LiveSources::deliverable`); o Python não sobe `StateMonitor` nem `PreviewBroker` dessas
  sessões. Captura em processo
  (`-C`; psmux avulso no Windows), fatos do plugin lidos do `plugin::Store` do Rust; leitura que
  não vem é `problema=state_facts_unavailable`, nunca estado inventado, e esse código vai ao diário
  e ao log, não à tela (portão de saída, regra "Aviso na tela só para ação que falhou"). O `Sources` não tem método com corpo
  padrão: fonte que esquece um não compila. A lista lê o último `state` do `Monitor` vivo
  (`state/published.rs`) e não captura o pane dessa sessão. No modo `python` tudo roda como antes.
  Codex (`info.headless`), Claude (provider `ClaudeHeadless`) sem terminal e Codex com terminal
  cujo ator é do Rust (`codex_terminal_owned` em `side.rs`) têm no lugar do `Monitor` o feed do runtime
  (`state/runtime_feed.rs`): o ator escreve vista, erro e prévia num canal em processo
  (`RuntimeRegistry::live`), fora do `/runtime/events`, e o feed publica os seis eventos (os quatro
  mais `pensamento`/`ferramenta`) coalescidos em 150 ms; o Python não os produz nem os repassa. No
  Claude a sugestão sai do `plugin::Store`, e a sessão parada mostra o estado
  estacionado (linha de status, modo de permissão, `dead` sem sidecar nem troca), tudo no Rust. Só
  o modo `python` volta a produzi-los. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#estado-do-claude-sem-terminal-no-feed-do-runtime).
- **A fila durável ao vivo do chat é do hub do Rust no modo `rust`/`pending`, em qualquer porta.**
  Um produtor por hub e por geração (`queue_feed.rs`) lê a projeção da fila e publica
  `message`/`queue_confirmed` pelo retrato; o login de convidado lê o mesmo hub no Rust. A
  baixa da fila do Codex sem terminal é pedida pelo hub ao ator, em processo. O `follow` do Python
  fica com a reserva de Claude/Codex no modo `python`. Contrato e diferenças em
  [plataforma.md](docs/decisoes/plataforma.md#fila-durável-ao-vivo-no-hub-do-rust).
- **Com o Rust de pé, o Python não lê o arquivo de estado da fila (`runtime/<chave>.json`).** No
  Windows a leitura no meio da troca do Rust falha e faz a troca falhar. O que precisava dele é do
  Rust (`patch_meta`/`clear_cano` do Claude, `native_message`, recibos na caixa do Rust). O sidecar do
  Claude tem dois escritores, sob a trava `<nome>.lock` nos dois lados; leitura de sidecar no Python
  passa por `atomico.read`, que no Windows compartilha a exclusão. Evidência em
  [plataforma.md](docs/decisoes/plataforma.md#o-python-fora-do-arquivo-de-estado-da-fila).

## tmux + Claude Code truecolor

Preserve `COLORTERM=truecolor` e `CLAUDE_CODE_TMUX_TRUECOLOR=1` antes de iniciar o Claude;
`settings.json` não basta. Ao alterar wrappers/tmux ou diagnosticar cores, consulte
[`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md) e
[`docs/tmux.conf.example`](docs/tmux.conf.example).

## Agent skills

### Issue tracker

Markdown versionado, não GitHub Issues: spec em `docs/superpowers/specs/`, tickets como Tasks de um
plano em `docs/superpowers/plans/` — que é de onde `crates/hangar-server/src/list/plan.rs` lê a barra
de progresso e de onde `skills/orquestrar` recorta a Task do executor. O formato de `### Task N:`
e `- [ ] **Step N: …**` é casado por regex e não é livre. Ver
[`docs/agents/issue-tracker.md`](docs/agents/issue-tracker.md).

### Triage labels

Os cinco papéis canônicos (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`,
`wontfix`), escritos como linha `Status:` no corpo da Task — não há sistema de labels aqui. Ver
[`docs/agents/triage-labels.md`](docs/agents/triage-labels.md).

### Domain docs

Contexto único, mas **sem** `CONTEXT.md` e **sem** `docs/adr/`: o vocabulário e as decisões medidas
vivem neste `CLAUDE.md`, e decisão nova entra aqui, no mesmo formato. Inclui o glossário dos quatro
termos que se confundem (servidor · peer · conta · motor). Ver
[`docs/agents/domain.md`](docs/agents/domain.md).
