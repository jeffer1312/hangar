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
- `state.py` — classifies live state from `tmux capture-pane`: `working` / `idle` / `awaiting_input` / `dead`.
- `terminal_input.py` + `tmux.py` — input via `tmux send-keys` (prompt / option select via `(n-1)×Down`+`Enter` / `Esc`).
- `adapters/codex/` — um app-server WebSocket de loopback por sessão Codex; o backend
  consome eventos JSON-RPC enquanto a TUI `codex --remote` da mesma thread roda no tmux.
  O app-server é do PANE, não do backend. Sem terminal (`sem_terminal.py`), ele roda em stdio
  como filho do mesmo cano do Claude sem terminal, e o backend abre a thread e responde as
  aprovações por cartão. Decisões e armadilhas:
  [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md).
- `difusor.py` — **uma fonte por chave, não uma por conexão**: monitor de estado,
  acumulador de estatísticas e git da listagem são compartilhados entre os SSE abertos.
  Por quê e o que quebrou antes: [`docs/decisoes/plataforma.md`](docs/decisoes/plataforma.md).
- `codex_voice.py` + `CodexVoice.svelte` — voz do Codex no web, **beta e opt-in por
  servidor** (`codex_voice_beta`, nasce `false`). Desligada, o botão não monta e o backend
  recusa o WebSocket. Contrato completo:
  [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md).
- `adapters/kimi/` + `hooks/kimi_state_hook.py` + `kimi_hook_installer.py` — Kimi Code runs in the
  same tmux-native shape as Pi: TUI in the pane, chat from
  `~/.kimi-code/sessions/<wd>/<session_id>/agents/main/wire.jsonl`, state pushed by hooks in
  `~/.kimi-code/config.toml` (no pane scraping for state). The pane↔session link is the hook's
  ticket (`~/.claude/.hangar-kimi/<pane>.json`) — the CLI has no caller-chosen session-id.
- `sse.py` — merges the above into the SSE stream. `api.py` — FastAPI routes. `auth.py` — bearer token / `cp_token` cookie.
- Also: `pqueue.py` (durable input queue), `preview.py` (live in-flight block), `askquestion.py`
  (native AskUserQuestion stepper), `uploads.py`, `git_ops.py`, `commands.py`, `workflows.py`,
  `model_picker.py`, `config.py`, `fs.py`, `hook_installer.py`.

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
    for the list (only the per-session SSE does), so a killed session's row simply disappears.
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

./scripts/test-wrappers.sh                 # claude-engine (bash/zsh/fish) against a fake `claude`, no tmux
./scripts/test-statusline.sh               # statusline.js contract (engine sessions suppress cost), needs node
node scripts/test-pi-hangar-state.mjs          # hangar-state.ts: fork de subagente do Pi não rouba o pane
```

**A verificação da tarefa é o uso real**: abrir a tela, clicar, rodar o fluxo. **Teste
automatizado (vitest, pytest, check) só roda quando o usuário pedir** — ele pede no fim, antes do
push; nem entre edições, nem ao terminar a tarefa. Pedido o focado: num comando só, os testes dos
arquivos tocados (`cd backend && uv run pytest tests/test_x.py tests/test_y.py`,
`cd frontend && npx vitest run src/x.test.ts src/y.test.ts`); falhou → repita só o que falhou.
Pedido o completo: `npm run check` na raiz, `vitest` e `pytest` inteiros. O `pre-push` roda sozinho os testes
ligados aos arquivos que vão subir e recusa o push se falharem; a suíte inteira é do `ci.yml`. `npm run build` só para servir o `dist` local. Ao reportar, diga o que foi conferido no uso real e que os testes automatizados não rodaram.

Claude sessions with a terminal must run as `claude --session-id <uuid>` **inside tmux** —
`scripts/install-claude-wrapper.sh` sets this up. Headless Claude/Codex sessions are created by the
app or `hangar-send --new ... --headless` and are discovered from their durable sidecars.
The same installer also wraps interactive `codex`: it calls the local backend through `scripts/hangar-codex`,
creates a managed Codex app-server/TUI pair, and attaches the caller to that tmux session. Codex
subcommands/advanced flags remain raw; `command codex` is the explicit bypass.

## Sessões-irmãs (hangar-send) + pareamento

Sessões usam `scripts/hangar-send` para recados, pareamento e criação; consulte `--help` antes
dessas operações. Recados e pareamento 1:1 alcançam outros servidores por `servidor::sessao`;
`--group` é local. A sessão nasce no modelo/esforço/permissão pedidos, validados por
`app/model_args.py`. Pareamento = vínculo simétrico (`app/pair.py`, sidecars em
`<config>/.hangar-pair/`) + prompt de protocolo injetado nas duas sessões; a UI
mostra chip 🤝 (Composer), badges nas listas, PairSheet (conversa do par + contrato compartilhado
`<a>__<b>.md` + split view desktop).

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
Escopo: `--group` é local; recados, pareamento 1:1 e `--list` alcançam OUTROS servidores via
endereço `servidor::sessao`: `backend/peers.json` (id → base_url+token, gitignored;
ver `peers.json.example`) + `CP_SERVER_ID` no `backend/.env`. Peer com `"enabled": false` sai da
VARREDURA (painel e `--list`) mas segue endereçável por `servidor::sessao` — é pra máquina que
você sabe que está desligada, senão cada poll paga o timeout de 4s esperando ela (id desta máquina, endereço de
resposta do `[de: id::sessao]`). Recados usam o script; o pareamento remoto passa pelo backend
local e por `/pair-remote` no destino.

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
- **Two views: mobile & desktop (820px).** `Sidebar` (desktop) e `SessionList` (mobile) são
  arquivos separados: template e CSS mudam nos DOIS e se verifica nos DOIS. Lógica da lista vai
  no `lib/sessionListModel.svelte.ts`, e a agregação SSE no `lib/sessionsStore.svelte.ts` — uma
  por servidor, nunca uma por card.
- **Todo texto de interface vem de `m.<chave>()`** (Paraglide). `pt.json` e `en.json` no mesmo
  commit. Dado do servidor não vira chave. A trava `i18nGuard.test.ts` só desce. Rótulo de
  stub/fixture de teste que vive em árvore varrida pela trava é identificador (`abrir-term`),
  nunca frase.
- **Markdown NUNCA aparece cru.** Todo `.md` exibido passa por `renderMarkdown`. Um `<pre>` com
  `**` e `##` à mostra é bug, não estilo.
- **Tela ou lista nova trata os quatro estados: carregando, vazio, erro e sucesso.**
- **Config e opção moram em MODAL, não em painel docado.** Só o `DesktopSessionContext` fica
  docado. Tela de config usa **container query**, nunca media query — quem aperta a linha é a
  largura do painel.
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
- **Dedup de fila/pending é delicado** — bolhas `pending`/`queued-` reconciliam por texto
  normalizado contra o transcript real.
- **Vite HMR pode servir componente VAZIO** (stub <1KB, sem erro no console): a tela some sem
  nada quebrar. Remédio: reiniciar o serviço do front. Verificação de front SEMPRE inclui abrir
  a tela e confirmar que montou.
- **Fechar uma sessão e criar outra com o MESMO nome**: a chave do `Chat` e a marca de exclusão
  não podem ser só o nome — sem a época de recriação, a conversa da morta fica montada; sem o
  `jsonl` na marca, a nova nasce escondida para sempre.
- **Terminal real no rodapé e no celular**: um PTY por WebSocket, backend não interpreta nada.
  Um painel por sessão; xterm com fundo `rgba(0,0,0,0)`, nunca `'transparent'`. Com o painel
  aberto, quem conta linha de pane responde 409 — e o app tem que MOSTRAR esse texto.
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

### Harnesses — Claude, Codex, Pi, omp, Kimi → [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md)

**Antes de mexer em** `backend/app/adapters/` (claude_headless, codex, kimi, omp, pi), `codex_*.py`,
`engines.py`, `model_picker.py`, `permission_mode.py`, `skill_bridge.py`, `omp_dirs.py`, `loop.py`,
`terminal_input.py`, `state.py`, statusline/prévia, hooks de estado, ou em sessão sem terminal:
**leia a seção "Regras vigentes" de `docs/decisoes/harnesses.md`**. São 40 regras, e cada uma já
custou um bug calado.

### Windows → [`docs/decisoes/windows.md`](docs/decisoes/windows.md)

**Antes de mexer em** `install.ps1`, `scripts/*.ps1`, `procinfo.py`, `atomico.py`, psmux, encoding de
arquivo gerado, `subprocess`/código de retorno, ou teste que simula `os.name`: **leia "Regras
vigentes" de `docs/decisoes/windows.md`**.

### Instalação, atualização e serviços → [`docs/decisoes/instalacao.md`](docs/decisoes/instalacao.md)

- **Reiniciar o backend**: sem `--reload`; mate `-9` o pid da porta e suba destacado. No Linux é
  `systemctl --user restart`.

**Antes de mexer em** `install.sh`/`install.ps1`, `scripts/install-*`, `atualizar.py`,
`atualizacoes.py`, `docs/atualizacoes/`, `VERSION`, `frontend/dist` servido pelo backend ou na
criação de sessão sob escopo do systemd: **leia "Regras vigentes" de `docs/decisoes/instalacao.md`**.

### Plataforma — nomes, pareamento, planos, ditado → [`docs/decisoes/plataforma.md`](docs/decisoes/plataforma.md)

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
  Varredura de sessão morta confirma ausência por TEMPO, nunca por número de polls. Aviso do app
  sai como `[painel: <rótulo com espaço>]`, nunca `[de: …]`: o modelo responde a quem assina.
- **Plan progress lê o `.md` do plano**, sem arquivo de estado: blocos cercados são removidos
  preservando offsets, e a decoração roda dentro do `to_thread` do git.
- **Ditado: a transcrição não é o problema, o que vem depois é.** Vocabulário vai para a Whisper
  (consertar depois é impossível por construção); regra de prompt só funciona com exemplo de
  entrada e saída; raciocínio no modelo de limpeza piora e não é calibragem. Quem manda no estilo
  é a pill que a pessoa leu antes de falar, não a config.
- **Transcrição, organização do texto e leitura são capacidades separadas.** A tela não leva nome
  de provedor no rótulo da transcrição; endpoint próprio de áudio nunca compartilha sua chave com
  o LLM padrão, e controles de voz só aparecem dentro do provedor que os oferece.
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
  abre ou volta a ficar visível: a queda dele é a suspensão do aparelho, não a máquina.
  Falha com o app em segundo plano não conta (nem marca nem grava prazo): tenta a cada 30 s
  fixos e reconecta todos na volta.
- **Revisão de código:** neste repositório, revisão local e as verificações do projeto.
- **MCP `hangar` (`/mcp`): identidade do chamador vai no cabeçalho e o backend resolve.** Chave
  vence pane, pane vence nome, pane ambíguo não resolve, nada resolvido é erro (nunca `cli`). O
  bearer é conferido ANTES do sub-app (mount passa por fora do `Depends`) e nunca entra no
  ambiente do pane: Claude via `headersHelper`, Codex via `http_headers`. Tool mapeia 1:1 num
  endpoint que já existe; o CLI continua como fallback e resolve identidade sozinho.
- **Arquivo citado na conversa é LEGÍVEL e EDITÁVEL; a citação é o consentimento.** Fora da raiz
  da sessão a política de caminho é `_resolver_citado()` (aparece no transcript), não a raiz — e
  é a mesma para o `GET` e para o `POST` de `/file/text`. A mecânica de ler e gravar é a do
  `filetree` (`read_at`/`write_at`): digest da leitura, tmp+rename, `.git` fora por componente
  do realpath. Escrita nova fora da raiz entra por aqui, nunca afrouxando o `/files/write`.
- **HTML servido como arquivo executa isolado e sem o token na URL do documento interno.**
  Arquivos citados e uploads usam `file_response`; SVG/XML mantêm o MIME com scripts bloqueados.
- **Configuração compartilhada leva o conteúdo, e o destino resolve caminho e programa.** Caminho
  vira marcador `⟦HOME⟧`/`⟦CLAUDE⟧`/`⟦CODEX⟧`/`⟦HANGAR⟧` (nunca `{HOME}`); quem envia vence;
  hooks e skills do Hangar, MCP `hangar`, credenciais e o login do `.claude.json` são sempre do
  destino. Regras e motivo em [plataforma.md](docs/decisoes/plataforma.md#configuração-compartilhada-leva-o-conteúdo-o-destino-resolve-caminho-e-programa).
- **Logs pertencem ao Hangar, não à conta.** Use `log_paths.base()`; diário exportável registra
  etapas, códigos e origem da falha. Texto de conversa, credenciais e saídas brutas ficam fora
  dele. O shell Electron também escreve lá (`privado/shell.log`): lançado pelo `.desktop`, o
  console dele vai pro `/dev/null`. Detalhes e compatibilidade em [plataforma.md](docs/decisoes/plataforma.md#diário-de-uso-causa-e-contexto-no-arquivo-exportado).

## tmux + Claude Code truecolor

Preserve `COLORTERM=truecolor` e `CLAUDE_CODE_TMUX_TRUECOLOR=1` antes de iniciar o Claude;
`settings.json` não basta. Ao alterar wrappers/tmux ou diagnosticar cores, consulte
[`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md) e
[`docs/tmux.conf.example`](docs/tmux.conf.example).

## Agent skills

### Issue tracker

Markdown versionado, não GitHub Issues: spec em `docs/superpowers/specs/`, tickets como Tasks de um
plano em `docs/superpowers/plans/` — que é de onde `backend/app/planprog.py` lê a barra de progresso
e de onde `skills/orquestrar` recorta a Task do executor. O formato de `### Task N:`
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
