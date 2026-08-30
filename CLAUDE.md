# Hangar

Drive a live Claude Code session (running in a `tmux` session on your machine) from your phone over
LAN/VPN, as a mobile chat. Single-user, LAN/VPN-only by design. Backend: Python 3.14 + FastAPI
(`backend/`). Frontend: Svelte 5 PWA (`frontend/`).

- **Architecture + full API table + run guide:** [`README.md`](README.md).
- **End-user / setup guide** (pairing, Tailscale, install as PWA, every feature): [`docs/USAGE.md`](docs/USAGE.md).
- Other docs in `docs/`: design brief, onboarding/network, polish backlog, tmux setup, future features.

## Architecture at a glance

The app never scrapes the terminal for chat content — it reads Claude Code's **JSONL transcript** and
only peeks at the tmux pane for live **state**. Backend pieces (`backend/app/`):

- `registry.py` — SessionRegistry: tmux list/new/kill ↔ maps Claude sessions to JSONL and Codex
  sessions to their durable thread/rollout sidecar.
- `transcript.py` — tails `~/.claude/projects/<cwd>/<uuid>.jsonl` (the chat content).
- `state.py` — classifies live state from `tmux capture-pane`: `working` / `idle` / `awaiting_input` / `dead`.
- `terminal_input.py` + `tmux.py` — input via `tmux send-keys` (prompt / option select via `(n-1)×Down`+`Enter` / `Esc`).
- `adapters/codex/` — one loopback WebSocket app-server per Codex session; the backend consumes
  structured JSON-RPC events while a `codex --remote` TUI for the same thread runs inside tmux.
  **O app-server é do PANE, não do backend** (`scripts/hangar-codex-tui`, o lançador único que o
  backend e o terminal chamam igual): ele escolhe a porta, sobe o servidor em segundo plano, roda a
  TUI em primeiro plano — nunca `exec`, que é o que o deixaria sem quem matar o servidor na saída —
  e grava `endpoint`+`app_pid` no sidecar junto de thread/rollout/cwd. O backend só se **liga**
  nele (`AppServerClient.connect`), conferindo o pid antes: porta de loopback é reciclada, e
  conectar só pelo endereço pode cair num processo alheio. Pid morto é sessão morta, não sessão a
  reconectar. Por isso criar sessão Codex passou a ser o caminho normal de criação (`registry.create`
  com `provider="codex"`, transcript vazio como Pi/Kimi) — não há mais `create_codex`. Duas armadilhas
  que sobram: o `codex` está no `_EXEC_PROVIDER` porque entre o pane nascer e o sidecar existir o pane
  cairia no default `claude` e seria casado com o transcript do Claude do mesmo diretório; e nessa
  janela `info.jsonl` é `None`, então tudo que deriva chave do transcript (`session_key`) tem que
  desviar — `session_key(None)` levanta `TypeError` e derrubaria a lista inteira, de todas as sessões.
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
Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+.
Frontend uses **npm** (has `package-lock.json`).

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

Sessions must run as `claude --session-id <uuid>` **inside tmux** — `scripts/install-claude-wrapper.sh`
sets this up. A `claude` without an id, or outside tmux, is invisible to the app or flagged ⚠ no id.
The same installer also wraps interactive `codex`: it calls the local backend through `scripts/hangar-codex`,
creates a managed Codex app-server/TUI pair, and attaches the caller to that tmux session. Codex
subcommands/advanced flags remain raw; `command codex` is the explicit bypass.

## Sessões-irmãs (hangar-send) + pareamento

Sessões Claude da MESMA máquina se falam via `scripts/hangar-send` (`--list`, `<sessao> "msg"`,
`--pair <sessao> "tarefa"`, `--unpair`, `--new <nome> [cwd] [--engine <motor>] [--provider ...]
[--conta <nome>] [--model <id>] [--effort <nivel>] [--permissao <modo>]` — a sessão já NASCE no
modelo/esforço/permissão pedidos, validados pelo backend via `app/model_args.py`) — tudo sobre a
API local do backend (`/input`, `/pair`, fila durável). Pareamento = vínculo simétrico (`app/pair.py`,
sidecars em `<config>/.hangar-pair/`) + prompt de protocolo injetado nas duas sessões; a UI
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
`orquestrar` — esta sessão vira líder de um grupo multi-repo (cria/pareia sessões via
hangar-send, escreve o contrato do grupo, distribui escopo, monitora e consolida).
`orquestrar` — conduz UM trabalho da ideia ao push: research, spec/plano com
o usuário, e daí em diante autônomo — um executor, um revisor independente de outra família
por commit, portão entre as Tasks, e uma sessão fresca revisando a branch no fim. O revisor
entrega **correção fechada** (causa reproduzida, arquivo/símbolo, inventário de callers,
passos numerados, comportamento final, prova), não diagnóstico; a próxima Task só abre com
`APROVA`. Task que mexe em pixel carrega uma **barra** (uma tela nomeada, que dá pra abrir):
o executor compara o print dela com a barra numa escolha **cega** feita por subagente fresco,
teto de 2 rodadas, e o revisor refaz a comparação. Um escritor por árvore, e o padrão é
**serial** — lote paralelo com uma worktree por Task é exceção declarada no plano
(`references/paralelo-worktree.md`), com merge mecânico do árbitro e verificação completa
depois de cada merge. `SKILL.md` é roteador: cada sessão lê só a página do seu papel em
`references/`.

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
Escopo: pareamento e `--group` só dentro da mesma máquina. Recado 1:1 e `--list` alcançam OUTROS
servidores via endereço `servidor::sessao`: `backend/peers.json` (id → base_url+token, gitignored;
ver `peers.json.example`) + `CP_SERVER_ID` no `backend/.env`. Peer com `"enabled": false` sai da
VARREDURA (painel e `--list`) mas segue endereçável por `servidor::sessao` — é pra máquina que
você sabe que está desligada, senão cada poll paga o timeout de 4s esperando ela (id desta máquina, endereço de
resposta do `[de: id::sessao]`). Só o hangar-send muda — o backend nem sabe da feature.

## SSE event model

The frontend `EventSource` (`screens/Chat.svelte`) listens for:

- `message` — transcript events: `user_msg` / `assistant_msg` / `tool_use` / `tool_result`.
- `state` — live state + status line (model / context / cost / rate badges).
- `preview` — live in-flight assistant text (full-replace; dropped when the real block commits).
- `ask_question` — opens the native AskUserQuestion sheet.
- `ping` — liveness heartbeat; resets a 25s watchdog that reconnects on half-open connections.
- `reset` — transcript swapped (e.g. `/clear`) → wipe and reload history. **Também** quando o
  *provider* da sessão muda debaixo de um stream já aberto: uma sessão Pi/Kimi recém-criada leva
  ~15s até a extensão publicar o bilhete do pane, e nesse intervalo o registry a classifica como
  `claude` e resolve um caminho no layout do Claude, que nunca vai existir (medido 21/08/2026:
  `sse: abriu name=hangar provider=claude jsonl=a05ee4a8-….jsonl` às 16:01:14, com o `.jsonl` do Pi
  nascendo às 16:01:31 em `~/.pi/agent/sessions/`). Rebindar só o arquivo não bastava — o adapter
  (parser do transcript, monitor de estado, fonte da prévia) era escolhido **uma vez**, na abertura,
  então o tailer lia o arquivo certo com o parser errado e o chat ficava mudo até sair e voltar.
  Hoje o `jsonl_watcher` vigia `provider` junto do `jsonl` e emite `__reprovider__`, que refaz as
  quatro tarefas dependentes de adapter. Duas regras: a troca de provider **não** espera os 2 polls
  de confirmação que a troca de arquivo exige (ela não oscila — é a sessão terminando de se
  identificar), e o `drain` da fila passou a resolver o adapter na hora, porque drenar pela TUI
  errada digita teclas que aquela TUI não espera.

## Conventions & gotchas (read before touching UI / backend lifecycle)

- **O nome antigo (`claude-pocket`) só existe em ponte de compatibilidade** (rename de 25/08/2026).
  O código conhece **um** nome: as pastas de dados são `<config>/.hangar-*`, o cofre do sync é
  `~/.hangar/`, os comandos são `hangar-send`/`hangar-engine`/`hangar-codex`/`hangar-conta` e
  `hangar-panel-*`, a instância do quickshell é `hangar`, e o logger é `hangar.*`. Escreva com o
  nome novo; nunca leia o antigo em código novo. O que resta do velho é, todo ele, migração:
  - `backend/app/migracao_sidecars.py`, chamado na **subida** do backend (`main.py`), renomeia
    `.claude-pocket-*` → `.hangar-*` em todo perfil `~/.claude*` e deixa **link** no caminho antigo.
    O link é o que impede a máquina de se partir no meio da atualização: hook, extensão do Pi e o
    publicador de statusline do Kimi (`~/.kimi-code/statusline.js`, que nem mora neste repo) podem
    estar vivos e desatualizados, escrevendo no nome velho — e caem na pasta nova. Startup, e não
    installer, porque atualizar é `git pull` + reiniciar o serviço; rodar `install-*.sh` não é
    garantido. Ele **nunca funde** duas pastas: destino já existente para naquele item, com aviso.
  - `.json` SOLTOS (`apelidos`, `conn`, `models`, `runner`, `opencode`) leem os dois caminhos, novo
    primeiro (`migracao_sidecars.caminho_de_leitura`), porque no Windows link de ARQUIVO exige
    privilégio — para pasta há junção (`mklink /J`), para arquivo não há equivalente. **Escrita
    sempre no nome novo.**
  - `<cwd>/.hangar-uploads/` é a única pasta que mora no projeto, fora do alcance da migração da
    subida: `uploads._base()` a migra na primeira leitura daquele cwd, senão todo anexo antigo
    citado por caminho absoluto no histórico viraria 404.
  - **Marcador de bloco gerenciado** (rc do shell, `~/.tmux.conf`, `keybinds.lua`, o bloco
    "Sessões-irmãs" do `~/.claude/CLAUDE.md`) virou `hangar`, e cada installer **arranca o bloco do
    marcador antigo antes** de escrever o novo — sem isso o arquivo do usuário fica com os dois, um
    deles ensinando o comando velho e que nenhum installer atualiza mais.
  - `cp-send` e companhia continuam existindo como **symlink permanente** pro mesmo script (rc
    antigo e sessão Claude já aberta chamam o nome velho); as variáveis `CP_*` e o cookie
    `cp_token` **não mudam** — quebrariam o `.env` de instalação alheia. A documentação ensina só o
    nome novo.

- **Two views: mobile & desktop (820px breakpoint).** `App.svelte` switches on
  `matchMedia('(min-width: 820px)')`: desktop → `DesktopShell` (which uses `Sidebar.svelte`), mobile →
  `SessionList.svelte`. Lots of UI has a per-view path (the session list is the clearest — `Sidebar` vs
  `SessionList`; sheets also re-dock as a right-side panel via `@media (min-width: 820px)`). Whenever you
  touch the front, make the change in BOTH views and verify BOTH — they drift apart easily (e.g. the
  session-list ordering ended up alphabetical only in `SessionList`, not `Sidebar`).
  - **The multi-server SSE aggregation lives in ONE place now** — `lib/sessions.ts` (pure dedup/order/
    classify helpers, unit-tested) + `lib/sessionsStore.svelte.ts` (a refcounted singleton: one
    `openSessionsStream` per *server* for the whole app, `retain`/`release` by consumer). `Sidebar`,
    `SessionList`, `Board` and `Canvas` all subscribe to it — the old `slots`/`recompute`/`connect` trio
    copied in three files is gone (top item of the polish-backlog structural debt, resolved 2026-07-17).
    The **two-views drift warning still stands**, though: `Sidebar` and `SessionList` are still separate
    files, so template/CSS changes to the list must be made and verified in BOTH. See
    [`docs/polish-backlog.md`](docs/polish-backlog.md#structural-debt-in-the-session-list-2026-07-16) —
    unifying the two list views is the remaining "bigger fish", deliberately not done yet.

- **i18n: todo texto de interface vem de `m.<chave>()`** (Paraglide, `frontend/src/paraglide/` gerado;
  `pt.json` + `en.json` em `frontend/messages/`). A trava em `src/lib/i18nGuard.test.ts` falha o teste
  quando um arquivo passa do seu número na linha de base `frontend/i18n-baseline.json` — e a linha de
  base **só desce**: arquivo novo tem limite zero, e texto novo em arquivo existente quebra o CI.
  Falso positivo do extrator (heurística ~89%) vai pro `i18n-allow.json`, nunca pra linha de base.
  Chave nova primeiro procura no `pt.json` (reuso antes de duplicar), e `pt.json`/`en.json` andam
  juntos no mesmo commit — chave que falta num deles aparece como ID cru na tela sem erro nenhum.
  Dado do servidor (nome de sessão, caminho, mensagem do agente, saída de comando) **não** vira chave.
  O idioma segue o sistema por padrão e troca em Configurações → Geral (a tela recarrega — as
  mensagens são funções compiladas, não valores reativos); o seletor guarda em `PARAGLIDE_LOCALE`.
  Texto que o **backend** manda pra tela (erros, descrições de built-ins) chega como chave/código e é
  traduzido no front — exceção: frontmatter de skills e conteúdo de chat são dados, não interface.
- **iOS black-rectangle repaint.** Glass on NavBar/Composer lives in a `::before` leaf with a near-opaque
  solid bg and **no** `backdrop-filter` / `transform` / `translateZ` on WebKit — those promote a layer that
  renders pure black during momentum scroll. Don't reintroduce them. Liquid-glass blur is Chromium-only
  (`html[data-liquid]`).
- **Transparência é padrão do app, não enfeite de uma tela.** O app tem papel de parede
  (`html[data-bg="image"]`) e um slider **Transparência** que move `--cp-panel-alpha`
  (`lib/background.ts`, `aplicarScrim`). Todo painel de vidro — `BottomSheet`, `ModalDialog`,
  `Sidebar`, `DesktopSessionContext` — já anda com esse slider via `--glass-panel`. Quem quebra é
  **superfície DENTRO do painel**: um `background: var(--bg-elevated)` ou `var(--bg-base)` cru não
  acompanha o véu, e o controle vira retângulo chapado boiando sobre a foto enquanto o painel atrás
  dele é translúcido. Ao escrever CSS de qualquer componente, nesta ordem:
  1. **`transparent`** — o certo por padrão. Quem carrega o material é o contêiner; a textarea do
     `Composer.svelte` é o precedente (`background: transparent` por cima do vidro).
  2. Precisa mesmo de superfície própria (campo de texto, chip, menu flutuante, bloco de saída)?
     Use os tokens de `app.css`: **`--surface-raised`** (chip, botão pequeno, menu) e
     **`--surface-inset`** (campo de texto, área de entrada). Sem papel de parede eles são
     exatamente `--bg-elevated`/`--bg-base`; com papel de parede entram no mesmo véu sozinhos.
  3. `--bg-elevated`/`--bg-base` crus só para **realce de estado** (`:hover`, `.sel`, linha atual),
     que é tinta por cima da linha, não superfície.
  Quanto as caixas ficam mais opacas que o painel **não é constante no CSS**: é o slider *Solidez
  das caixas* (Aparência → Fundo, ao lado de Transparência), que escreve `--cp-surface-alpha`
  (`lib/background.ts`). O ponto certo depende da foto de quem usa — se um valor desses te parecer
  errado no código, o lugar dele é um controle, não um número fixo.
  Verificação: ligue um papel de parede e olhe a tela. Qualquer retângulo que não deixe a foto
  atravessar, enquanto o painel em volta deixa, é bug — não estilo.
- **Config e opção moram em MODAL, não em painel docado.** Decisão de desenho de 2026-07-30, vale
  daqui pra frente. No desktop, a **única** coisa que fica docada à direita do chat é o
  `DesktopSessionContext` — o painel de contexto **daquela sessão** (estado, plano, grupo, repo).
  Ele é dado do que está aberto na tela, então acompanha a conversa. Todo o resto — Aparência,
  Configurações do servidor, Motores, Git, e o que for adicionado — abre como **modal centrado**:
  `BottomSheet` com `wide={isDesktop} centered={isDesktop}` (o mesmo par que o próprio
  `SettingsModal.svelte` usa; no celular continua folha subindo de baixo).
  O porquê é medido, não gosto: no dock de ~530px, rótulo + descrição à esquerda e um segmentado à
  direita brigam pela linha, e como o rótulo tem `min-width: 0` ele cede tudo — a descrição quebrava
  em **uma palavra por linha**. Tela de configuração é rótulo-e-controle repetido dezenas de vezes;
  ela precisa de largura, e largura é o que o dock não tem.
  Ao mexer em qualquer tela dessas, use **container query** (`container-type: inline-size` no
  wrapper + `@container`), nunca media query: quem aperta a linha é a largura do PAINEL, não a da
  janela — num monitor de 1440px o dock tem 530px e uma media query de 560px nunca dispara ali.
  **Config e opção num modal único — implementado (2026-08-16).** A direção acordada de juntar as
  configs num só modal (antes marcada "ainda não implementada") existe: `SettingsModal.svelte` abre
  todas as telas num `BottomSheet` de navegação por seções (Aplicativo · Servidor) com onze linhas —
  Geral, Aparência, Ditado, Sobre, Acesso, Contas, Servidores, Notificações, Anexos, Avançado,
  Motores. Quem for adicionar aba: registra no `LINHAS` do `SettingsModal.svelte` e no
  `lib/configRoute.ts` (`TelaConfig`/`TELAS_DE_SERVIDOR`), com chave de idioma nos dois
  `messages/*.json` no mesmo commit. O `lib/gitTabs.ts` + `GitTabs.svelte` continuam sendo o
  precedente de navegação por abas DENTRO de uma tela (incluindo nível por aba no celular).
- **The message list is windowed.** `MessageList.svelte` mounts only the last `WINDOW=120` events; scroll-to-top
  reveals older pages (in-memory, no backend call). Don't render the whole transcript at once.
- **Queue/pending dedup.** Messages sent while Claude is `working` echo as `pending` / `queued-` bubbles and
  reconcile against the real transcript by normalized text/line. Touch `Chat.svelte` dedup carefully.
- **The phone app renders `AskUserQuestion` natively.** The `ask_question` SSE event opens the
  `AskQuestionSheet` stepper; since the pending payload isn't in the jsonl, a PreToolUse hook
  (`askq_capture.py`, installed idempotently by `hook_installer.py`) captures it into a sidecar. Verified
  live: use AskUserQuestion freely, it shows as the stepper. Numbered plain text is only a fallback for a
  session where that capture hook isn't installed. Raw TUI option pickers (not the tool) surface separately
  via `OptionButtons` (selection sent by `terminal_input.py`), so free composer text does not answer a picker.
- **Restarting the backend.** No `--reload` (it holds SSE + watchfiles). `pkill -f app.main` can match your
  own shell; SIGTERM can hang on an open SSE connection. Kill `-9` the pid bound to the port and relaunch
  detached (`setsid`).
- **Vite HMR servindo componente VAZIO (stub de ~800 bytes).** Medido 2× em 2026-08-04 (vite 8.1.0 +
  vite-plugin-svelte 7.1.2 + svelte 5.56.4): depois de editar um `.svelte` com a página aberta, o dev
  server passa a servir o módulo transformado como um stub sem template (`function X(...) { ...; return
  $.pop(...) }` e mais nada) — o componente monta ZERO nós, SEM erro no console e SEM overlay do Vite.
  Sintoma na UI: a tela/componente some (ex: o chat inteiro vira papel de parede; cliques em cards não
  fazem nada porque a rota nunca monta). O `svelte-check` passa — o arquivo está bom, quem corrompeu é o
  cache de transform do dev server, e ele vale pra TODOS os clientes (não é por-browser). Diagnóstico:
  `fetch('/src/<modulo>')` na página — stub tem <1KB e não contém o markup. Remédio: `systemctl --user
  restart hangar-frontend.service` + reload ignorando cache. Verificação pós-edição de front
  SEMPRE inclui abrir a tela afetada e conferir que ela montou (não só o `check`/`vitest`).
- **Markdown NUNCA aparece cru.** Todo conteúdo `.md` exibido no app passa por `lib/markdown.ts`
  (`renderMarkdown`) com tipografia própria — contrato do par (`PairSheet`), prompt/transcript de
  subagente (`ActivitySheet`), plano, README, qualquer arquivo lido do disco. Um `<pre>` com
  `**Tarefa:**` e `##` à mostra é sempre bug, não estilo. Vale também pro que vem de fora do
  transcript: se o texto é markdown, renderize.

- **CSS animations.** Shared tokens/keyframes live in `app.css` (`--ease-out`, `--spring`, …); a global
  `prefers-reduced-motion` rule neutralizes loops, so new keyframes don't each need their own guard.
- **Loop runner** (`app/loop.py` + `components/LoopSheet.svelte`): loop autônomo por sessão —
  goal → sessão trabalha → idle dispara tick (`_on_hook_transition`, dentro do `_work`, só com
  `sent == 0`) → roda `check_cmd` (exit 0 = `done`) ou procura `LOOP_DONE` (→ `done_claimed`,
  que SÓ fecha com confirmação humana via `/loop/resolve`) → senão re-prompta com a cauda do erro.
  Sidecar em `.hangar-loop/<nome>.json` (sobrevive `/clear`); guardrails: max_iters,
  branch≠main, kill-switch `automations_enabled`, anti-estagnação (mesma cauda 2×). Loop ativo
  **suprime o chain** da sessão. Campos `loop_status/loop_iter/loop_max` fluem no `/api/sessions`
  e no `sig` do SSE (badge 🔁 nas 2 views). Spec/decisões: docs/superpowers/specs/2026-07-22-*.md.
- **Model engines** (`app/engines.py` + `app/engine_probe.py` + `components/settings/EnginesSettings.svelte`):
  a session can run on a non-Anthropic provider — only env vars change inside that session's process,
  `~/.claude` (skills, hooks, transcript) stays the SAME. Single source of truth at
  `~/.claude/engines.json` (0600). Four invariants: (1) `engines.py` is **stdlib-only** — an
  `app.config` import there would pull in pydantic and break `scripts/hangar-engine`, which the shell
  calls with the system `python3`; (2) it's `ANTHROPIC_AUTH_TOKEN`, **never** `ANTHROPIC_API_KEY`
  (that one writes `customApiKeyResponses` into the global `~/.claude.json`); (3) the env is applied
  by `hangar-engine --exec <engine> -- claude …` (`os.execvpe` inside the pane) and **never** via
  `tmux -e`, because the key would land in `/proc/<pid>/cmdline`, world-readable — tmux doesn't
  inherit the caller's env, so there's no "just export it" path; (4) the context-window var is
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS` — `CLAUDE_CODE_AUTO_COMPACT_WINDOW` measured inert on both
  providers tested, and without the right var Claude Code still compacts at ~167k on a 500k model.
  A live session's engine is read back from `/proc/<pid>/environ` (`CP_ENGINE`), same trick as
  `CLAUDE_CONFIG_DIR` — it's what keeps both resumes (`registry.resume` and the Archive one in
  `api.py`) from silently switching engines mid-conversation; Archive resume, unlike a live resume,
  has no process left to read, so it always re-asks. Models and context window come from
  `GET {base_url}/v1/models` — no static catalog, because the value varies by the user's
  subscription tier. The statusline only hides `💵`/cost-sidecar writes on an engine session — the
  effort chip (`(high✦)`) is untouched, it's not faked.
- **Modelo de uma sessão Claude Code: a lista NUNCA é constante** (`app/model_picker.py` +
  `terminal_input.list_model_options` / `set_engine_model` + `app/default_model.py` +
  `components/ClaudeModelPopover.svelte` + `components/ClaudeEffortPopover.svelte`). Duas fontes, escolhidas pelo que a sessão é — medido em
  31/07/2026, claude 2.1.220:
  - **Conta Anthropic** → as linhas do próprio picker do `/model`, lidas ao vivo (abre, parseia,
    Esc). A lista `['default','opus','sonnet','haiku']` chumbada no front envelheceu: o picker real
    tem 5 linhas com o **Fable** entre Opus e Sonnet, então o app escondia um modelo e ainda dava a
    Sonnet/Haiku o número de linha errado. `MODEL_NUMBERS` sobrou só como fallback pra linha rolada
    pra fora da viewport. Cache de **1 hora** por config dir porque ler a lista **dirige o
    terminal** e isso deixa rastro: `❯ /model` + `⎿ Kept model as …` (o Esc de saída) ficam no
    scrollback do tmux pra sempre. Não polui o chat do app — entra no jsonl como `type: system`,
    que o `transcript.py` ignora —, mas quem estiver com aquele terminal aberto vê, e cinco
    leituras empilhadas ali já pareceram bug. Esperar o **rodapé** (`Esc to cancel`), não só o
    título, antes de parsear: no instante em que o título aparece as linhas ainda estão sendo
    pintadas e a leitura devolvia 4 modelos, sem o Haiku. E nunca mandar o 2º Enter sem antes
    reler: se o picker já abriu, esse Enter **confirma como default** a linha sob o cursor — num
    caminho que era pra ser só leitura.
  - **Sessão de motor** → o `/v1/models` do provedor (o mesmo `engine_probe` da tela de Motores).
    Ali o picker é inútil: lista os 4 aliases, **todos apontando pro mesmo `ANTHROPIC_MODEL`**
    (`Custom Opus model`, `Custom Fable model`, …) — e `gateway_model_discovery: true` não muda
    isso. A troca vai por `/model <id>`, que aceita id arbitrário.
  Três armadilhas medidas: (1) `/model <id>` grava o id como **default GLOBAL** no
  `settings.json` ("saved as your default for new sessions") — uma sessão nova da conta Anthropic
  nasceria pedindo `kimi-for-coding`; `default_model.restore_quando_aterrissar` repõe o valor
  anterior, e **espera a escrita chegar**, porque o arquivo só muda ~0.8s depois do Enter (repor
  antes é um no-op e o vazamento aterrissa em seguida); (2) a linha `⎿ Set model to …` da troca
  ANTERIOR continua na tela, então a confirmação só vale se **mencionar o id pedido** — sem isso a
  primeira leitura devolvia a resposta da troca passada como se fosse desta; (3) o guard de "posso
  digitar agora?" usa **duas capturas**, não uma: um pane parado não distingue spinner vivo de
  marcador de turno concluído (está na docstring do `state.classify`), e uma captura só recusava,
  com "está trabalhando", uma sessão que tinha acabado de terminar.
- **Pi model + thinking level** (`app/pi_models.py` + `scripts/pi/hangar-state.ts` + `components/PiModelPopover.svelte` + `components/PiEffortPopover.svelte`):
  the third mechanism, next to Claude's TUI picker and Codex's app-server, and it does **not** scrape
  the pane. Measured on pi 0.82.1: `/model` is a fuzzy-**search** list of ~300 entries (footer
  `(1/301)`, 10 rows visible) — not enumerable from the pane and not navigable by counting `Down`;
  and there is no `/thinking` command (it lives inside `/settings` → "Thinking level", a submenu).
  So the Pi extension we already ship publishes a catalog sidecar
  (`<config>/.hangar-pi/models/<jsonl-stem>.json`, same key as the state marker) and registers
  `/cp-model <provider> <id>` + `/cp-think <level>`, which the backend types with `send-keys` and Pi
  applies through `pi.setModel()` / `pi.setThinkingLevel()`. Two invariants: (1) the thinking levels
  are **per model** (glm-5.2 → off/low/medium/high/xhigh; k3 → low/high/max), so they come from the
  session, never from a constant — the static `LEVELS` tuple only rejects garbage before typing;
  (2) Pi **clamps** the level to what the model supports (`agent-session.js:1277`), so the endpoint
  re-reads the sidecar and returns what *stuck*, not what was asked (asking `max` on glm-5.2 lands on
  `xhigh`). Missing sidecar → 409 telling the user to re-run `install-claude-wrapper.sh`, never an
  empty list that reads as "no models".
- **Before typing into the Pi's composer, ASK — the screen cannot tell a notice from a draft**
  (`terminal_input._composer_ocupado_pi` + `pi_inbox.perguntar` + `responderPergunta` in
  `scripts/pi/hangar-state.ts`). Pi prints extension notices (`console.error`) **inside the composer
  band**, with the same ANSI as typed text; measured 22-23/08/2026, `cursor_flag` is 0 either way.
  So the anti-paste guard counted our own `[hangar-state] linha do hangar conectada` as a draft and
  every `/cp-model`/`/cp-think` came back **409 with the composer empty**. Recognizing each phrase by
  regex is whack-a-mole (`/reload` draws a fourth one no regex of ours knows), and the "compare two
  captures — a notice is static, a draft changes" upgrade the code itself proposed **was measured and
  does not hold**: a *parked* draft is static too, and the parked draft is exactly what the guard
  exists for. What answers is the Pi: `ctx.ui.getEditorText()` returns `""` with a notice on the band
  and the exact text with a draft. So the `pi_inbox` line, until then delivery-only, took a second
  verb — `{id, pedir}` out, `{id, resposta}` back. Four rules: questions live in a **separate**
  futures dict from deliveries (a delivery resolves `(ok, erro)` and a question resolves a value);
  `""` is an **answer** and `None` is absence (→ fall back to scraping, so an old extension behaves
  exactly as before); the question does **not** take `linha.lock` (that lock orders *writes*, and a
  read must not queue behind a 3s ACK); and it uses `pi_inbox.linha_de(name, pane_id)`, never the raw
  pane. Note `/reload` drops and re-raises the line — a command fired inside that ~5s window falls to
  plan B and can still 409.
- **Ditado: a transcrição não é o problema, o que vem depois é** (`app/transcribe.py` +
  `app/narrar.py:limpar_ditado`). Duas etapas, dois modelos: a Whisper (`whisper-large-v3-turbo`)
  ouve, e um LLM limpa. Tudo aqui foi **medido em 14/08/2026** — 5 ditados reais × 3 execuções ×
  4 modelos —, e as três coisas que mudaram valem como regra, não como preferência:
  - **O modelo da limpeza importa mais do que parece, e o critério não é tamanho — é obediência.**
    O `llama-3.3-70b-versatile`, o padrão até aqui, inventava pasta em caminho ditado
    ("backend barra app barra narrar ponto py" → `backend/barra/app/barra/narrar.py`, 3/3) e mantinha
    **as duas versões** quando a pessoa se corrigia falando. Padrão agora é `openai/gpt-oss-120b`
    (Groq, ~1,2s). O melhor dos quatro foi o `deepseek-v4-flash`, mas ele **raciocina**: 6,4s de
    mediana e **3 de 15 chamadas estourando o timeout de 8s** da limpeza — o ditado voltava cru. Com
    `reasoning_effort: "none"` ele cai pra 1,8s e acerta tudo. Daí o campo `llm_reasoning_effort`, que
    é **opcional de propósito**: vazio = a chave some do payload, porque mandá-la a um provedor que
    não a conhece é um 400 que derruba a limpeza inteira.
  - **Regra de prompt só funciona com exemplo de entrada e saída.** A regra "aplique as correções que
    a pessoa falou" era a razão de ser da limpeza e falhava 0/3 em dois modelos: eles *pontuavam* a
    correção ("A primeira é o custo do carretel. Não, desculpa. A primeira vai ser…") em vez de apagar
    a versão errada. Trocar o verbo por **APAGUE** e colar um par entrada/saída levou a 3/3. Mesmo
    padrão na regra 4 (`barra` → `/`, `traço traço` → `--`): sem o par, o `gpt-oss-120b` deixava a
    frase literal 3/3. Toda regra nova aqui **nasce com exemplo**, e com um contra-exemplo quando ela
    pode generalizar demais ("o ponto principal", "a barra de rolagem" não podem virar pontuação).
  - **Vocabulário vai pra Whisper, não pro LLM.** O `prompt` da API é enviesamento de decodificação,
    e é onde `hangar-send` para de sair "CP send". Consertar depois é impossível por construção: a
    limpeza tem ordem explícita de **preservar** nome próprio como veio, então o que a Whisper errou
    chega errado no fim. `VOCAB_BASE` (termos do app, valem pra todo mundo) + `ditado_vocabulario`
    (o que é de uma pessoa só), truncados em `_VOCAB_MAX` porque a API corta em ~224 tokens **calada**.
    `language=pt` fixo pelo mesmo motivo: sem ele, frase curta cheia de jargão inglês voltava em inglês.
  - Cuidado de cota: o prompt novo tem ~940 tokens por chamada (era ~400). No plano gratuito da Groq
    (8000 tokens/minuto) isso não incomoda um ditado por vez, mas **estoura em teste automatizado** —
    um 429 lá é cota, não qualidade; separe os dois antes de culpar o modelo.
  - **Três estilos, escolhidos na pill ao lado do microfone** (`ESTILOS_DITADO`, `ditado_estilo`,
    `components/DitadoEstiloPopover.svelte`): `limpar` (só tira hesitação e pontua), `prosa`
    (reorganiza e corta repetição — o padrão) e `briefing` (vira documento com seções). A pill fica na
    barra do composer, ao lado do microfone, e abre o MESMO popover do esforço — não um modal: é
    decisão do tamanho de escolher o esforço, e cobrir a tela pra isso é desproporcional. Existem
    porque a mesma limpeza não serve pros dois usos: ditar "abre o narrar.py" e ditar um pedido de
    dois minutos. Quem lê o estilo é o backend, então o atalho Ctrl+Espaço já grava no estilo
    escolhido sem saber que ele existe. **`briefing` é rebaixado pra `prosa` abaixo de
    `_MIN_PALAVRAS_BRIEFING`** — sem isso ele punha um `**Objetivo**` em cima de uma linha só.
  - **A trava de honestidade mudou de forma porque a antiga proibia o que o usuário pediu.** Contar
    palavra nova crua (o guarda antigo) rejeitava qualquer estruturação: `Objetivo:`, `-`, e até
    escrever "tô" como "estou" contavam como conteúdo inventado — 8 "palavras novas" num ditado
    real, com 100% do conteúdo preservado. Agora são duas medidas, e as duas foram calibradas
    contra os mesmos casos: **cobertura** (quanto do conteúdo da pessoa sobreviveu; pega o modelo
    que resumiu ou respondeu) e **`_conteudo_novo`** (palavra de conteúdo que ela não falou). Medido: defeito 4 palavras novas, limpeza honesta 0, prosa real 1 → teto 2. **Cobertura
    sozinha não separa** (defeito 79%, prosa legítima 75%), e é por isso que as duas coexistem.
  - **O `briefing` NÃO paga a trava de invenção** (`_Travas.cobra_invencao`), e isso é decisão do
    usuário, não descuido: "no briefing minhas palavras vão mudar; se eu estiver em prosa, aí
    beleza, não mudar minhas palavras". `limpar` e `prosa` não reescrevem — um pontua, o outro
    reordena —, então ali palavra nova é palavra que a pessoa não disse. O briefing reescreve por
    definição, e cobrar dele é recusar o serviço pedido: medido, um briefing bom com 98% de
    cobertura foi rejeitado por 4 "invenções" que eram conjugação. Ele segue protegido pelo teto de
    tamanho, pelo piso de cobertura e pela recusa de saída vazia.
  - **Comparação é por RADICAL** (`_radical`), não pela palavra inteira. `clicava`/`clico`/`clicar`
    caem no mesmo balde. Sem isso a trava punia conjugação — a mesma classe de erro que
    `_CONTRACOES` resolveu pra `tô`/`estou` e que voltou por outra porta. **Mas a vogal final só cai
    com prova de verbo no próprio texto** (`_raizes_de_verbo`, alimentado pelos DOIS textos): cortá-la
    sempre juntava `posto`/`posta` e `conta`/`conto` no mesmo radical — o par que o comentário do piso
    usava como exemplo do que não podia acontecer —, e aí trocar "a conta do cliente" por "o conto do
    cliente" passava com 0 palavra nova e 100% de cobertura, calado, justo em `limpar` e `prosa`.
    Sufixo de verbo (`ava`, `ando`, `ar`, …) e derivação (`mente`, `dade`, …) cortam sempre; plural
    (`s`) também. Contra-exemplo travado em `test_troca_de_genero_ainda_e_palavra_nova`.
  - `_CONTRACOES` iguala fala reduzida à forma escrita (`tô`→`estou`, `pra`→`para`) **antes** de
    qualquer comparação. Sem isso a limpeza melhora o texto e é punida por isso.
  - **Raciocínio piora e não é questão de calibragem.** Testado com os dois ditados reais: com
    `reasoning_effort` ligado, 4 de 9 execuções estouraram 25s e a única prosa que voltou levou
    14,9s, contra 2,3–3,2s desligado. Num teste anterior o modelo pensando ainda comeu o "não o
    redis" de "usa o postgres não o redis", lendo negação como autocorreção. Pensar sobre um texto
    vira interpretar o texto, e aqui interpretar é o defeito.
  - **Quem manda no estilo é a PILL, não a config** (`?estilo=` no `/transcribe` →
    `narrar._estilo_efetivo(cru, pedido)`). O estilo mora no servidor, mas o app o lê **uma vez por
    carga de página** (`lib/ditadoEstilo.svelte.ts`): uma troca feita noutra aba ou noutro aparelho
    nunca chegava na tela aberta, e em 21/08/2026 a pill dizia "Só limpar" enquanto o servidor
    guardava `briefing` — o ditado voltou estruturado sem ninguém ter pedido. O front manda junto o
    rótulo que a pessoa **leu antes de falar**, e ele vence; ausente ou desconhecido, a config
    decide como sempre. Duas amarras: o front só manda quando o store já leu o servidor
    (`ditadoEstilo.pronto` — mandar o padrão chutado seria o app sobrescrevendo a escolha dela com
    palpite), e o popover **revalida** ao abrir, pra a lista parar de exibir valor de horas atrás.
  - **O teto de tempo é rede contra pendurar, e ele mora nas DUAS pontas.** O do navegador era
    120s (`lib/api.ts`) enquanto o backend podia gastar 120s de Whisper **mais** a limpeza: a
    requisição era abortada com o trabalho em curso e a pessoa perdia o ditado inteiro por causa do
    relógio do cliente. Hoje: 300s no cliente, e 60/90/120s por estilo no servidor (subidos em
    21/08/2026 — o `muse-spark-1.2-contributor-free` do OpenCode Zen levou 16,4s pra limpar UMA
    frase, contra ~1,2s do `gpt-oss-120b` na Groq). Quem estoura ainda **não perde o áudio**: o
    `Composer` guarda o `File` da tentativa que falhou e oferece "Transcrever de novo" ao lado do
    erro — o áudio já existe, mandar a pessoa repetir dois minutos de fala é que era o defeito.
  - **O provedor da limpeza é trocável pela tela, e não só a Groq** (Configurações → Servidor →
    Avançado: Endpoint / Chave / Modelo / Raciocínio do LLM → `llm_*` em
    `~/.claude/runtime-config.json`). `_provedor()` só lê `llm_api_key` quando há `llm_base_url`
    próprio; endpoint vazio = Groq com a `groq_api_key`. E o **briefing tem provedor próprio**
    (`llm_briefing_*`, `_provedor("briefing")`), porque os dois usos não pedem o mesmo modelo:
    limpar e prosa querem rapidez — a pessoa está olhando o campo esperando o texto —, o briefing
    quer quem estrutura melhor e pode demorar. Medido aqui: Groq/`gpt-oss-120b` 2,1s no limpar,
    OpenCode Zen/`muse-spark-1.2-contributor-free` 9,0s no briefing. O perfil sai do **estilo**,
    nunca de um flag à parte, e endpoint de briefing vazio cai no provedor de sempre. Provedor
    lento muda o que as travas veem:
    medido no muse-spark, um ditado com autocorreção longa cai pra 62% de cobertura e o estilo
    `limpar` (piso 0,80) devolve o cru com aviso — o modelo apagou a versão corrigida, que é a
    regra funcionando, mas o piso de `limpar` não foi calibrado nele.
- **Statusline por sidecar, não pelo pane** (`app/statusline.py` + `scripts/omniroute-statusline.js`
  + `scripts/pi/rich-status-line.ts` + `~/.kimi-code/statusline.js`): a linha que o app mostra
  (modelo, contexto, ⚡5h/📅7d, custo)
  **não** sai do transcript — quem a calcula é o agente, e o app só via o texto **já renderizado no
  terminal**, cortado na largura da janela. Medido 2026-07-30 num pane de 99 colunas: o Pi chama
  `truncateToWidth` e a linha morre em `cache…` (somem contexto, cota e custo); o Claude quebra em
  várias linhas, mas quando a quebra cai em cima do par de contexto ele vira `💬 769k/238 770k…`.
  Nos dois casos o painel dizia "medição indisponível" **por causa do tamanho do terminal**.
  Contrato: quem RENDERIZA publica a linha inteira (sem ANSI) em
  `<config>/.hangar-status/<stem>.json` = `{"line", "ts"}` — mesma chave dos outros
  marcadores (o stem do `.jsonl`) — e `statusline.read()` a prefere ao pane, caindo nele quando não
  há sidecar (sessão sem instrumentação **nunca** pode ficar sem linha nenhuma). Três detalhes que
  já custaram bug: (1) o tmp do `tmp+rename` leva o **pid**, porque o script do Claude roda a cada
  render e duas invocações da mesma sessão se sobrepõem (nome fixo → `rename` promovendo bytes
  entrelaçados, o mesmo furo que `hangar_panel_common.py` já corrigiu); (2) `read()` exige **dict** —
  JSON válido do tipo errado (`null`, lista) não levanta `ValueError` e o `.get()` derrubava a
  resolução de estado de TODAS as sessões em `list_with_state`; (3) o publicador do Pi vive na
  extensão porque a linha completa só existe dentro do processo dele — logo, **sessão Pi já aberta
  só passa a publicar depois de `/reload`** (o Pi carrega extensão na largada), enquanto o lado
  Claude vale na hora, por ser script executado a cada render. O publicador do **Kimi Code**
  (`~/.kimi-code/statusline.js`, fora do repo porque o `tui.toml` aponta pra lá) segue o lado
  Claude: script a cada render, sidecar em `~/.claude/.hangar-status/<sessionId>.json` —
  a chave é o `sessionId` do stdin, o mesmo que `session_key()` extrai do `wire.jsonl`. A linha
  dele replica os marcadores do Claude (`🤖 K3 (high✦)`, `📁 dir [branch*]`, `⚡5h`, `📅7d`,
  `🕐 HH:MM ⏱`) com duas diferenças de formato: o contexto vem como par **rotulado e sozinho**
  (`💬 ctx 480k/1M` — o stdin do Kimi não traz in/out do turno, então a regra dos "≥2 pares" do
  parser/`sse._status_sig` tem exceção pro rótulo `ctx`, a mesma do Pi) e **não há 💵** (Kimi é
  assinatura de valor fixo, mesmo motivo do Claude em motor). O ⏱ dele é a idade do
  `wire.jsonl` (birthtime), não duração de API como no Claude.
- **O `wire.jsonl` do Kimi não é um transcript bem-comportado** — duas armadilhas medidas em
  14/08/2026, as duas em produção, na mesma sessão:
  - **Nem toda escrita é turno.** O hook grava `idle` no `Stop` e `state.corrige_ocioso_kimi`
    promovia pra `working` sempre que o arquivo fosse mais novo que o marcador (é o que cobre o
    prompt ENFILEIRADO na TUI, que não dispara hook nenhum). Só que o Kimi grava `config.update` —
    o system prompt inteiro, ~90KB — com a sessão parada: turno fechou 08:28, o `config.update` caiu
    08:40 e a sessão ficou "em execução" com o pane no prompt. Agora o mtime é só o **portão barato**
    (um `stat` por poll) e quem decide é `_kimi_turno_aberto`, que lê o **fim** do arquivo até a
    primeira fronteira de turno: `turn.ended`/`turn.cancel` = parada, `turn.prompt`/`turn.steer` =
    andando (levantado sobre todos os wires da máquina: não há outro `turn.*`). O regex é só filtro
    barato — quem decide é o `type` de TOPO da linha, via json, senão uma msg CITANDO
    `"type":"turn.ended"` vira fronteira.
  - **O main fica MUDO quando delega.** Subagente (tool `Agent`/`AgentSwarm`) roda no mesmo
    processo mas escreve no wire DELE (`<sessão>/agents/agent-N/wire.jsonl`); o
    `agents/main/wire.jsonl` não recebe uma linha enquanto isso. E quando um subagente termina, o
    hook `Stop` dispara com o `session_id` da SESSÃO — marcando `idle` no meio do turno do main.
    Foi essa dupla que fez a mesma sessão aparecer "pronta" com o terminal mostrando
    `Running 2 agents`, três vezes. Por isso o mtime não decide nada: quem decide é a fronteira de
    turno do main, e prova de vida (no caminho degradado) é o mtime mais novo entre TODOS os
    `agents/*/wire.jsonl`. Quem for mexer em estado do Kimi: **o wire do main não é a sessão**.
  - **`tool.result` não tem `uuid`** (só `parentUuid` e `toolCallId`), e o parser mandava `id=""`.
    O front deduplica evento **por id** (`Chat.svelte`, `idIndex`), então os 205 resultados de uma
    sessão real disputavam o MESMO slot: cada um apagava o anterior. Dois estragos ao mesmo tempo —
    todo card de ferramenta preso em "Executando…", e o card do **AskUserQuestion reabrindo depois
    de respondido** (o front deriva "respondida" da presença do `tool_result`; quando a ferramenta
    seguinte tomava o slot, a pergunta voltava a parecer pendente). Id agora é `res:<toolCallId>`.
    O teste antigo não pegou porque fabricava um `uuid` que o Kimi nunca manda: **ao escrever teste
    de parser, copie o shape do wire real**, não o que a doc sugere.
- **Furar a fila do Kimi (steer)** (`terminal_input.steer_now` + `POST /api/sessions/{name}/steer` +
  o chip `⏳ N na fila · mandar agora` no `Composer`): msg enviada com a sessão trabalhando fica na
  fila da TUI do Kimi ("↑ to edit · ctrl-s to steer immediately"); o `ctrl-s` a injeta no turno em
  curso — vira `turn.steer` no wire, no MESMO turnId, com o `context.append_message` de user de
  sempre (por isso o dedup da fila durável não muda nada). Medido: o ctrl-s promove a fila
  **inteira** de uma vez (duas msgs entraram como um bloco só), e com a sessão parada é no-op. É
  tecla avulsa, não parâmetro do envio: a decisão "essa não espera" vem DEPOIS de já ter mandado. O
  número do chip conta as bolhas translúcidas — eco local (`pending`) **mais** os eventos
  `queued-` da fila durável; só o eco local dava 0 (ele some em ~1s, quando o `queued-` chega) e o
  chip nunca nascia. 409 fora do Kimi.
- **Prévia ao vivo: sidecar do agente primeiro, pane depois** (`preview.read_sidecar` +
  `scripts/pi/hangar-state.ts`): mesmo contrato da statusline, agora pro texto **em voo**. A extensão do
  Pi recebe o bloco do assistente token a token (`message_update`) e publica o **último bloco de
  texto** em `<config>/.hangar-preview/<stem>.json` = `{"text", "ts"}`; `PreviewBroker._loop`
  o prefere e só cai no `capture-pane` quando não há sidecar. É o que tira a prévia do Pi da
  adivinhação: todo o `extract_assistant_text` (verbo de ferramenta, caixa do composer, spinner,
  painel de Todos) existe só pra separar prosa de desenho de TUI, e um quadro do spinner em `*`
  ASCII — fora de `SPINNER_GLYPHS` — já fez a prévia engolir a linha de status **e o painel de
  tarefas inteiro** (03/08/2026). Quatro coisas que o desenho decide de propósito: (1) `""` é
  **resposta** ("não há nada em voo"), `None` é ausência (cai no pane) — tratar os dois igual traria
  de volta o bloco já commitado como bolha duplicada; (2) publica o **último** bloco, não a soma —
  mandando a soma, `sse.preview_is_committed` vê o commitado como prefixo da prévia e engole tudo;
  (3) a extensão coalesce em 150ms e `unref()` o timer, porque `message_update` dispara por token e
  um timer pendente não pode segurar o processo do Pi vivo; (4) teto de idade de 10min, pro caso da
  extensão morrer no meio do turno — aí o pane volta a mandar em vez de congelar a última frase.
  Vale o mesmo aviso da statusline: **sessão Pi já aberta só publica depois de `/reload`**. O
  **Claude Code também publica** desde 17/08/2026: `hooks/preview_hook.py` (instalado pelo
  `hook_installer.ensure_preview_hook_installed` no startup) escuta o evento `MessageDisplay`
  (Claude Code ≥ 2.1.152 — deltas INCREMENTAIS do texto em exibição, medido: 5 parágrafos = 6
  eventos com `index` crescente e `final` no último, markdown cru) e grava o mesmo sidecar; o
  `Stop` zera. O acúmulo entre eventos vive no próprio sidecar (`message_id` gravado junto), e
  texto com `agent_id` (subagente) nunca é publicado. Sessão Claude já aberta não relê hooks →
  segue no pane até reiniciar; a raspagem inteira do `extract_assistant_text` vira plano B, não
  código morto. Codex nunca raspou pane (app-server).
- **Process info lives in `app/procinfo.py` — the only OS-bound layer.** Nine functions
  (`_proc_children_map`, `_descendant_pids`, `_open_jsonl`, `_cmdline`, `_config_dir_of`,
  `_proc_start_time`, `_engine_of`, + the two `_proc_*_path` test seams) hold **every** `/proc`
  read in the backend; `registry.py` imports them and no longer knows what OS it's on. Four rules:
  (1) the implementation is chosen **once, at import, by capability** (`Path("/proc").is_dir()`),
  never by OS name — "is it unix?" says YES for macOS, which has no `/proc` and would silently read
  nothing; (2) **Linux does not move to psutil** — `open_files()` is orders of magnitude slower than
  listing `/proc/<pid>/fd` and these run per poll, per session; (3) both implementations live in
  **one module**, not three — with `procinfo.py` importing from a `procinfo_proc.py`, a monkeypatch on
  `procinfo._proc_stat_path` wouldn't reach the caller inside it and the test would pass by *accident*
  reading real `/proc`; (4) `psutil` is a **platform-conditional** dependency
  (`sys_platform != 'linux'`), so a Linux install doesn't download it — but it's an unconditional
  *dev* dependency, because `tests/test_procinfo.py` forces `_TEM_PROC = False` on Linux to exercise
  the Windows/macOS path against real processes. Without that, code that only runs off-Linux would
  never be tested by anyone developing on Linux.
- **Windows runs on psmux, not tmux** (`marlocarlo.psmux` — native ConPTY multiplexer that publishes a
  `tmux` alias, so `tmux.py` calls it unchanged). Measured on psmux 3.3.7: `new-session -e`, exact
  `=NAME:` targets, `-F` formats incl. `#{?alternate_on,...}`, `capture-pane -S` with Unicode intact,
  named keys and the option picker all work. **`paste-buffer` works too** — what cannot carry a
  newline are the **buffers**: `set-buffer` truncates at the first one and `load-buffer` escapes it
  with nothing ever unescaping it back, so multi-line arrives cut either way (measured on this same
  version; an earlier note here claimed `paste-buffer` itself was missing, and that was wrong).
  That is why Windows sends multi-line **through the clipboard** — `Set-Clipboard` over stdin plus
  one `M-v` (`tmux.paste_via_clipboard`), under a module-wide lock, because the clipboard belongs to
  the machine and not to the session. The `paste_text` fallback stays for everything else and
  branches on the **return code**, not on the OS: a multiplexer that lacks a command says so, and on
  Linux the fast path returns 0 and never reaches plan B. Plan B is one `send-keys -l` per line
  with `C-j` between; a `\n` *inside* the argument makes psmux swallow everything after it, and `\r`
  as a separator glues the lines together (both measured) — and it is precisely the path that was
  measured delivering 309 of 600 lines while returning success, which is why the clipboard exists.
  Probe: `scripts/test-psmux.py` (+ `.ps1`).
  Install: `install.ps1`. Not there on Windows: systemd services and the `codex` shell wrapper. The
  `claude` one **is** there — `install.ps1` step 5/8 dot-sources `scripts/shell/claude.ps1` and
  `claude-conta.ps1` from the PowerShell profile — so a `claude` typed in PowerShell is trackable
  like on Linux; one typed in another shell (Git Bash) is not, and app-created sessions are always
  fine.
- **Where psmux and tmux disagree about IDENTITY and TARGETS** (measured on psmux 3.3.7, 22/08/2026).
  These four are not cosmetic: three of them were live bugs, and the pattern is the same — a command
  the tmux docs say **fails** or **addresses one thing** quietly does something else.
  - **`%N` addresses nothing.** psmux numbers panes per SESSION (tmux, per server), so two sessions
    each have `%1`. `send-keys -t %1` did not reach either of them: it landed in the **client's
    current session** — i.e. the app can type a phone prompt, Enter included, into someone else's
    conversation. `agentpane.resolve_target` only returns a `%N` when the session has 2+ panes, which
    is why it stayed latent. The address that works is `=<session>:<window_index>.<pane_index>`;
    `tmux.alvo_de_pane` builds it, and on POSIX returns the `%N` unchanged. `pane_id` is still the
    **identity** (Pi ticket, agentpane cache) — what changed is what serves as an **address**.
  - **`kill-session -t "=<name>"` does not kill.** The `=` (exact match) is honored by `has-session`,
    `display`, `send-keys`, `new-window` and `split-window` — and NOT by `kill-session`, which waits
    **5s** and returns rc=1 with the session still alive. `tmux.alvo_de_kill` is the single place
    that knows this (production and tests share it); test teardowns that missed it left **65** orphan
    servers on this machine and made `test_termsock` fail the NEXT case with "duplicate session".
  - **Killing the last session does not end the SERVER, and `list-sessions` cannot tell you.** psmux
    keeps a pre-warmed `tmux server -s __warm__ -L <socket>` process alive per socket, forever, each
    holding a shell and a console. On an emptied socket `list-sessions` answers rc=0 with **empty
    output** — byte-identical to a socket that never existed — so there is no question to ask the
    multiplexer; the process table is the only answer. This is a *test* leak with a machine-sized
    bill: 70 orphans here on 22/08/2026, ~12,7 GB of working set, and the Claude session running the
    suite died with the VM at its memory ceiling (`0xc00000fd`). Not one test ever went red. The
    cleanup is `kill-server` **on the own `-L` socket** (rc=0, 0,1s, idempotent even on a virgin
    socket) — never bare, which would take down the user's default tmux server. `tests/tmux_teste.py`
    is the single place that knows it (`novo_socket`/`matar_servidor`, which refuses an empty socket),
    and a session-scoped conftest fixture fails the suite if any registered socket still has a live
    process. On Linux the same defect is harmless (the server exits with the last session; a 0-byte
    socket file stays), so the fix is the same command with no OS branch.
  - **`rename-session` to an occupied name overwrites instead of failing** (rc=0). The session that
    was there does not die: it becomes unreachable, with the name pointing at the other one, and both
    processes keep running. `registry.rename` depends on the refusal to fall back to killing the old
    hidden shell, so `tmux.rename_session` now checks first — non-POSIX branch only.
  - **`list-clients` ignores `-F` and invents the tty.** Any format string comes back as the default
    line, every client shows as `/dev/pts/0` (even clients of different sessions), and
    `detach-client -t <tty>` is parsed as a session name. A line from `list-clients` therefore does
    **not** prove a client is attached — what proves it is the `[activity=...]` suffix and the
    `(attached)` flag in `list-sessions`. Worse than useless, in fact: with the session provably
    empty (`#{session_attached}` = 0) `list-clients -t "={name}"` still returns rc=0 and **one
    line** for a client that does not exist, so `assert list-clients == ""` is not a regression
    check there — it is a question that command cannot answer. `#{session_attached}` answers it on
    both multiplexers. `detach-client` is unusable for a different reason: with the exact target it
    answers `no session '={name}'` (rc=1) — the `=` is **not** honored by it, same family as
    `kill-session` above — and without the `=` it drops **every** client of that session, the
    user's own native `attach` included. This is why the terminal panel's Windows teardown is
    **killing our own `tmux attach` process**: measured, it releases just that client, a client of
    another session stays attached, and the session keeps running.
- **Where psmux and tmux disagree about CONFIGURATION — and how to tell a real setting from one it
  merely stored** (measured on psmux 3.3.7, 22/08/2026, on a throwaway `-L` socket). The section
  above is about commands that address the wrong thing; this one is about `bind`/`set` **accepting
  everything**. Same family as the `terminal-features` it once ignored in silence, except here
  reading the value back does not catch it either.
  - **`set -g <anything> <value>` returns 0 and the invented option comes back from `show -g`.**
    So "it accepted it, and I read it back" proves nothing about a psmux option — the read is just
    your own string. What proves a setting is real is it appearing in the **default `show -g`
    listing** (58 entries here) *before* anyone sets it. Use that as the test.
  - **No mouse key name can be bound.** `bind -T root WheelUpPane …` returns rc=0 and is **silently
    discarded** — `list-keys -T root` never shows it, in the plain form or in the nested `if -F`
    one. It is not the table and not `list-keys`: `NPage`, `Home`, `F5`, `C-a`, `M-v` and a prefix
    `bind X` all store fine, while `WheelUpPane`, `WheelDownPane`, `WheelUpStatus`, `MouseDown1Pane`
    and `MouseDrag1Pane` behave exactly like a `TeclaQueNaoExiste`. The consequence: the wheel
    recipe from the user's Linux `~/.tmux.conf` (`bind -T root WheelUpPane` + nested `if -F`)
    **cannot be ported**, and a future attempt will get rc=0 the whole way and look like it worked.
  - **`#{mouse_any_flag}` does not exist**; `#{alternate_on}` and `#{pane_in_mode}` do. Measured
    against an app holding the alternate screen and asking for mouse (`?1049h` + `?1000h` +
    `?1006h`): `alternate_on` went `0` → `1`, `pane_in_mode` answered `0`, and `mouse_any_flag` came
    back **empty** — same as an invented variable — even while the app was requesting mouse. So the
    "app that asks for mouse" branch of that recipe has no condition to test, either.
  - **The wheel-into-copy-mode behaviour is OUR option, not a psmux law.** psmux carries three
    settings tmux has no equivalent for — `scroll-enter-copy-mode` (default `on`), `mouse-selection`
    and `pwsh-mouse-selection` — and `docs/tmux.conf.windows.example` already writes
    `set -g scroll-enter-copy-mode on`. That line is what sends the wheel into copy mode; the lever
    is one word, and it is in our own managed block.
  - **Synthetic wheel events could not be delivered** (two attempts, both dead ends worth not
    repeating): writing the SGR sequence (`ESC [ < 64 ; x ; y M`) into a ConPTY's input measures the
    **ConPTY**, which translates input before the client sees it; and a `MOUSE_WHEELED`
    `INPUT_RECORD` posted with `WriteConsoleInput` into a console the client inherited never reached
    it either. Neither triggered copy mode even with the option `on`, which is the behaviour a real
    wheel produces every day — so the result is about the injection, not about psmux. Wheel
    behaviour here is verified by a human scrolling, not by a probe.
- **The pane's environment comes from the SERVER on tmux and from the CALLER on psmux — which is
  why `CLAUDE_CONFIG_DIR` cannot be exported unconditionally** (measured on psmux 3.3.7,
  22/08/2026). tmux gives a new session the env of whoever started the *server*, so `new_session`
  sent `-e CLAUDE_CONFIG_DIR=<value>` **always**, the default included: without it, a server started
  by a `claude-conta contaA` silently births every later session in contaA. psmux has neither half
  of that — the pane inherits the **caller's** env (`ZZ=x tmux new-session …` → the pane sees
  `ZZ=x`) and nothing crosses from one session to the next (a `-e` on session A is invisible to a
  later B). And exporting the default there is not free: for Claude Code, `CLAUDE_CONFIG_DIR` set —
  **even pointing at `~/.claude` itself** — means "read `.claude.json` from INSIDE that folder", a
  file it then creates empty (measured here: `~/.claude.json` 52236 bytes, the real one, against
  `~/.claude/.claude.json` 1259). So **every session created by the app on Windows landed on the
  welcome screen** ("Select login method", theme picker) with the credential intact, reading the
  wrong `settings.json` on the way (that is where the fullscreen TUI went). `tmux._e_config_dir` is
  the one place that decides: on POSIX always (the argument list is byte-identical to before); on
  psmux only when the value **differs** from `~/.claude`, or when the backend itself declares the
  variable — the pane would inherit that one anyway, so omitting would not erase it. Same rule in
  the Windows shell wrapper (`scripts/shell/claude.ps1`); the POSIX wrappers are untouched. The
  fallback everywhere else already reads absence as `~/.claude` (hooks, sidecars, `projects/`), so
  nothing else moves.
- **Windows-only trap in the installer: encoding is per interpreter, and ASCII is never the answer.**
  Every launcher `install.ps1` writes carries a PATH inside it (checkout, python, `%LOCALAPPDATA%`
  log — the user's profile name). Measured by executing each file with an accented path, console
  codepage 850: `.cmd` needs the console's OEM codepage (ANSI, UTF-8 and ASCII all fail; UTF-8 plus
  `chcp 65001` also works but changes the caller's console), `.vbs` needs UTF-16LE **with** BOM (ANSI
  works, OEM does not), `.sh` needs UTF-8 without BOM. `Escrever-Lancador` encodes per type; ASCII
  content still comes out byte-identical. And the same file's two other rules stand: the `.env` and
  `settings.json` must NOT have a BOM (`JSON.parse` in Node throws on it), while the PowerShell
  **profile** must — it is the only encoding both 5.1 and 7 can read when the path has an accent
  (5.1 alone writes ANSI, 7 alone writes UTF-8-no-BOM, and each fails to read the other's).
- **Two Windows traps that a `subprocess` and a `tmp+rename` hide from you** (measured 22/08/2026 on
  this VM, python 3.14, locale cp1252, console codepage 850). Both are cases where the failure does
  **not** land where you would look for it.
  - **`Path.replace` IS `os.replace`** — `pathlib` calls `os.replace(self, target)` — so
    `tmp.replace(alvo)` carries the exact WinError 5 that `atomico.substituir` exists to survive.
    The first sweep converted only the `os.replace(tmp, alvo)` spelling and left 20 sites written the
    other way, the sidecars with a concurrent reader by design among them (durable queue, the state
    marker read by a hook in another process, the price cache). The guard is now on the **shape**:
    `tests/test_atomico_call_sites.py` walks the AST of `app/` for `<x>.replace(<one positional
    arg>)`, a signature only the file rename has (`str.replace` takes two, `datetime.replace` takes
    keywords). Testing this needs the fake `os.replace` patched on the **`os` module**, not on
    `atomico.os` — same object, and only that reaches the `pathlib` spelling, so the case fails
    against the old code on Linux too.
  - **A strict decode failure in `subprocess` dies in a reader THREAD.** With `capture_output` there
    are two pipes, so Windows reads them in threads: `encoding="utf-8"` without `errors=` on a byte
    that is not UTF-8 prints `Exception in thread` to stderr, `run()` raises **nothing** and
    `stdout` comes back **None** — the caller blows up later, far from the cause (on Linux the same
    code raises `UnicodeDecodeError` from `run()`). This is why `errors="replace"` stays in
    `conta_estado`/`pi_catalog`; what changed is that its output stops being stamped as good — a
    field carrying U+FFFD is dropped (`conta_estado`, so the account keeps working) or refused
    (`pi_catalog`, where the `id` is later TYPED into the TUI). Note also that `encoding="utf-8"`
    **alone** already fixes the cp1252 mojibake; `errors=` covers a different failure.
- **`monkeypatch.setattr(os, "name", …)` in a test takes `pathlib` with it — and it does NOT blow up
  where you patched.** This is how `test_script_ao_lado_do_projeto_nao_e_acusado_de_inexistente`
  shipped green on Windows and could not even start on Linux (fixed in `b4d97790`): forcing
  `os.name = "nt"` to exercise a Windows branch made the test's own helper raise
  `UnsupportedOperation: cannot instantiate 'WindowsPath' on your system` before it asserted
  anything. Measured on 3.14 (win) and confirmed by the Linux run, the mechanism is worth knowing
  because none of it is where you would look:
  - The guard is a subclass `__new__` installed **at import time** by the REAL `os.name`
    (`class PosixPath: if os.name == 'nt': def __new__… raise`). Patching the attribute later never
    moves it, so the raising class is fixed for the whole process.
  - `Path(...)` itself **does not raise** — `Path.__new__` calls `object.__new__(cls)` and skips
    that guard, while still picking the class from the PATCHED `os.name`. So you get a `PosixPath`
    on Windows (or a `WindowsPath` on Linux) and nothing complains yet.
  - The blow-up lands on the first operation that RE-instantiates: `/`, `.parent`, `.with_suffix`
    (all go through `type(self)(...)`). And it is not uniform — measured, `PosixPath("a").is_file()`
    on Windows answers `False` instead of raising, so the wrong-class path can also just lie.
  So: in a test that patches `os.name`, use **`os.path`** (`join`/`isfile`), which is chosen at
  import and does not change class under you. Patching `os.name` is still the right way to exercise
  a `if os.name == "nt"` branch on both systems — it is the `pathlib` in the test's own scaffolding
  that has to go. Sibling cases only escaped by mocking `shutil.which` with a constant lambda.
- **`stop_command` on Windows: the return code cannot be read as failure, and the shell won't tell
  you in a language you can parse** (`projects.py`, measured against the real `cmd.exe`).
  `taskkill /F /IM x` with no such process answers **128**, the Windows sibling of the `pkill`
  returning 1 that made this code ignore `rc` in the first place; a POSIX `stop_command` (common
  when the project came from a Linux box) answers **1** with "not recognized". Charging by `rc`
  turns every stop of an already-stopped project into an error on screen, and the two stderr
  messages come translated into the Windows UI language. What separates them without depending on
  either is whether the command **exists** — so the check runs only **after** a non-zero rc, on the
  first token of the line, with `cwd` added to the search (`cmd.exe` looks at the current directory
  before PATH) and cmd builtins skipped. Not-found → a `ProjectError` naming the command and warning
  about the orphan; found and failed → silence, with rc and the stderr tail in the log. The stderr
  never reaches the screen: it comes in the console's OEM codepage, not the locale's.
- **Quem serve a interface é o BACKEND, e o `frontend/dist` chega pronto do CI.** Duas mudanças de
  25-29/08/2026 que andam juntas, e as duas têm o mesmo antônimo: uma máquina de quem usa não
  compila nem serve nada além do necessário.
  - O backend monta o `frontend/dist` na raiz (`api.py`, `_UIStatic`), então o `vite preview` num
    serviço à parte era um SEGUNDO servidor para o mesmo arquivo. Instalação nova não registra mais
    esse serviço — no Linux o `services-setup.sh` já decidia assim; o `install.ps1` passou a seguir.
    Quem **já** tem o serviço fica com ele: trocar a porta muda a ORIGEM, e origem nova é
    `localStorage` vazio (`cp_servers` com os tokens, tema, layout do canvas). Ninguém perde
    configuração por causa de um `git pull`. O que decide é a existência da unit/tarefa, nunca uma
    pergunta nova.
  - `CP_FRONT_PORT` deixou de ter `5173` cravado como default (`config.porta_do_front`): vazio = a
    porta do próprio backend. Com o serviço do front fora, o QR e o painel de alcance apontavam para
    uma porta onde ninguém escuta — foi o `Rede local … não respondeu` do painel. Quem mantém o
    preview tem o `5173` **gravado** pelos instaladores, e é isso que preserva a origem dele.
    O firewall também segue essa decisão: a 5173 só é liberada quando há serviço de front.
  - O `ci.yml` publica `frontend-dist.tar.gz` + `frontend-dist.sha` na release fixa `dist-latest` a
    cada push na main, e os instaladores baixam de lá **só** quando o `.sha` bate com o `HEAD` e o
    `frontend/` não está editado. Não bateu (CI ainda compilando), sem rede, ou tar quebrado → build
    local, como sempre foi. `tar.gz` e não zip porque o Windows 10+ traz `tar.exe` — um comando só
    nos dois instaladores. O `npm ci` **continua** para quem mantém o preview, que precisa do
    `node_modules`.
- **Session creation's systemd-scope probe.** Creating a session wraps `tmux` in
  `systemd-run --user --scope` so the tmux server doesn't inherit the backend's cgroup, but the wrap
  is now gated on a probe: a systemd user manager that refuses transient scopes was making **every**
  session creation fail (app and terminal both). Failing the probe, sessions are created without the
  scope and the backend logs a warning (commit `23da052`).
- **Atualizar pelo app** (`app/atualizar.py` + `app/atualizacoes.py` + `docs/atualizacoes/`): o
  botão faz tudo sozinho — decisão do usuário em 25/08/2026 —, `reset --hard` e reinstalador
  incluídos, porque quem usa não administra nada e não deve precisar saber que passos existem.
  Quatro coisas que o desenho decide de propósito:
  - **Roda destacado do backend** (`setsid` / `DETACHED_PROCESS`), e o progresso mora em
    `<config>/.hangar-update/estado.json`. A atualização reinicia o backend: dentro do processo ela
    se mataria no meio, e a máquina ficaria com código novo no disco e processo velho no ar — o
    estado que `install.ps1:1242` já registra como o pior. O arquivo é também o que deixa a tela
    dizer "atualizando…" enquanto o servidor volta, em vez de "desconectado".
  - **Automático não é irreversível.** `resguardar()` roda antes de qualquer coisa destrutiva: o
    que estava no disco vai pra `resgate/<data-hora>` + stash, e a função **confere a ref** antes
    de devolver. Falhou o resgate, a atualização para com o disco intacto. O único `reset --hard`
    que não passa por ali é o rollback, cujo alvo é um commit da própria máquina de minutos antes.
  - **O registro é do que JÁ RODOU aqui** (`aplicados.json`), não do intervalo de commits. O
    intervalo fura em instalação nova e em quem reclonou ou resetou. Instalação do zero marca tudo
    como aplicado (os dois installers), senão a primeira atualização roda a história inteira.
  - **Passo só entra no registro depois da PROVA passar** — comando com exit 0 e efeito ausente é
    a falha que o campo `prova` existe pra pegar. Passo novo: um arquivo em `docs/atualizacoes/`
    (formato no README de lá), no mesmo commit que o exige. Os não destrutivos também rodam na
    **subida do backend** (`main.py`), pelo motivo do `migracao_sidecars`: atualizar aqui é
    `git pull` + reiniciar, e ninguém garante que o botão foi usado.
  - **Quem reinicia o serviço é diferente em cada sistema, e no Windows já é o installer.** No
    Linux é `systemctl --user restart`; no Windows o `install.ps1 -Update` — chamado na etapa
    anterior — já derruba a instância velha (`Pare-Servico`) e chama `Start-ScheduledTask`, e esse
    bloco NÃO é pulado no modo `-Update` (o que ele pula é firewall/Tailscale e o hook). Há ainda
    a tarefa `hangar-vigia`, que sobe a tarefa de novo se a porta não estiver escutando. Por isso
    `_reiniciar` não faz nada no ramo Windows: marcar "falta reiniciar" ali fazia a tela pedir um
    passo que já tinha sido dado. Medido em 25/08/2026 naquela máquina: três tarefas
    (`hangar-backend`, `hangar-frontend`, `hangar-vigia`), backend como cadeia de três processos,
    e todas em `Ready` mesmo com o servidor vivo — o `.vbs` não espera.
  - **O installer matava a própria atualização, e a proteção é por COMANDO, não por linhagem.** O
    `Pare-Servico` derruba a "instância anterior" casando o caminho do checkout mais `uv|python`, e
    o motor roda como `<repo>\backend\.venv\Scripts\python.exe -m app.atualizar` — casa nos dois.
    Ou seja, o instalador chamado PELA atualização matava quem o invocou, no meio dela (medido
    25/08/2026: lock e processo morreram no minuto do "instância anterior derrubada"). A proteção
    por linhagem já existia e não bastou; hoje há exclusão explícita de quem tem `app.atualizar` na
    linha de comando. No Linux quem cobre isso é o escopo transiente do systemd, que lá não existe.

- **Plan progress** (`app/planprog.py` + `registry._decorate_plan` + `PlanBar`/`PlanPanel.svelte`):
  the source of truth is the plan's own `.md` under `docs/superpowers/plans/` — no separate state
  file, `parse_plan` re-reads it and re-counts `- [x] **Step …**` on every discovery. Fenced blocks
  (` ``` `/`~~~`) are stripped **before** the regex runs, **preserving byte offsets** (chars → spaces,
  `\n` kept) — plans show example steps inside code fences, and without the strip a freshly-written
  plan is born "3/47 done" (measured on this very plan: 53 matched vs 48 real). The decoration runs
  **inside** the git `to_thread` (`registry.py:851`), never in the coroutine — same precedent as the
  2026-07-23 incident. `_list_sig` (`sse.py`) carries `plan_name` alongside `plan_done`/`plan_total`:
  switching from plan A to plan B that happens to also read `9/17` wouldn't re-emit the list and the
  chip would stick on the wrong plan — the same bug class as `engine`. `plan_tasks` (one `(done,total)`
  per Task) rides the payload too, because the segmented bar can't be derived from
  `task_idx`/`task_total` alone — it would lie whenever an earlier Task still had a pending step.
  `_plans_dir` climbs up to 6 levels looking for `docs/superpowers/plans/` but **stops at the first
  `.git`** — without that, a worktree with no plans of its own would climb into the main checkout and
  show someone else's plan ("no bar" is a limitation, "wrong bar" is a bug). Executing a superpowers
  plan: mark `- [ ]` → `- [x]` at the end of each Step — that's what feeds this feature.
- **Real terminal in the desktop footer** (`app/termsock.py` + `components/TerminalPanel.svelte`,
  plus `tmux.new_hidden_shell` and the native-terminal launcher in `api.py`): one PTY per WebSocket
  running `tmux attach`, consumed by xterm.js. The backend interprets **nothing** here — no ANSI, no
  state, no scraping; it's a pipe, same choice as `adapters/codex`. Seven invariants, all measured
  on this machine (tmux 3.7b) while replacing the old `capture-pane` mirror:
  - **A tmux target needs the colon, and it fails DIFFERENTLY per command.** `={name}` is exact
    session match; `={name}:` is exact session, active window. Without the `:`, `list-panes -s -t =0`
    for a numeric name with no such session returns rc=0 **and the panes of the attached session**
    (a numeric name reads as a *window index*), `display -p '#{window_width}'` comes back **empty**,
    and `set-option -t "=alvo"` answers **"no such session" with the session alive**. `has-session`
    is the deliberate exception (it resolves sessions only, never a pane/window). Rule: every
    pane/window/option target carries `=` **and** `:`; the same operation never gets two spellings
    (the native-terminal `attach` was aligned to the termsock one in the final review).
  - **`attach` targets the SESSION, never the pane.** `attach -t %N` moves the active window/pane
    for **every** client attached to that session — opening the browser panel would drag the owner's
    native `tmux attach` to the agent's pane. `_pane_target` (send-keys/capture-pane) is the opposite
    case on purpose.
  - **One panel per session** (`termsock._ativos`, keyed by name). Two clients with
    `window-size=latest` fight over the size on every frame; the second connection tears the first
    down and the first one's socket is **closed** (a silently frozen terminal is worse than a
    visible disconnect).
  - **xterm's theme takes `rgba(0, 0, 0, 0)`, never the string `'transparent'`** — xterm 6.0.0's
    color parser only matches hex/`rgb()`/`rgba()`, the keyword throws inside `ThemeService`, which
    **swallows** it and falls back to opaque `#000000` over the panel's `--surface-inset`. Nothing
    in the console; you just lose the wallpaper behind a black rectangle.
  - **The hidden shell is a tmux user option (`@cp_hidden`), not a name convention.** The `+` tab
    creates a SEPARATE session `term-<name>` so the panel and the user's native terminal stop
    fighting over which window is in front; it is filtered out of the three views by the **mark**,
    read straight from tmux (`is_hidden`), because "missing from `registry.list()`" also happens to
    a real Codex session of the same name. The mark rides the shared `list-panes -a -F` as a 6th
    field, and that parse is **defensive** (5 fields or more): a multiplexer that doesn't
    interpolate a user option must cost you the *mark*, never the whole session list, which feeds
    the three views, `list_with_state`, `_pane_info` and `_cwd_has_siblings`. Only the psmux probe
    (`scripts/test-psmux.py`, section 4b) can tell you the command is *refused*, which no parse
    survives — keep it in sync with the format.
  - **`term-<name>` is keyed by NAME, so the name has to be kept in sync by hand.** Two different
    paths, don't mix them up:
    - *Orphan from another repo* — the shell outlives an agent session killed outside the app, and a
      later session that reuses the name would reattach the OLD repo's shell under the new label: a
      command typed in the wrong directory. `new_hidden_shell` compares `#{session_path}` (the birth
      directory — measured that a `cd` inside the pane moves `pane_current_path` and **not** this
      one) and kills+recreates on divergence. If that kill **fails**, it returns `None` (→ 500)
      instead of handing back the old-directory shell.
    - *Rename* — `registry.rename` **renames** `term-<old>` → `term-<new>`. A rename touches neither
      the cwd nor what is running in the pane, so there is no wrong-directory risk here; killing
      would silently take down whatever was running in the Shell tab (a `npm run dev`) with nothing
      but a `_log.debug`. The kill is only the **fallback** for when `term-<new>` is already taken —
      leaving the old one alive brings back the orphan this exists to prevent. Both paths gate on
      the `@cp_hidden` mark (`is_hidden`, exact `={name}:` target), so a third party's `term-<name>`
      is never renamed or killed.
  - **POSIX-only imports (`pty`, `fcntl`, `termios`) live INSIDE the functions.** `termsock` is
    imported by the 409 guard that also runs on Windows; a top-level `import fcntl` there is a
    `ModuleNotFoundError` that breaks a feature which works today. The rule is symmetric now that
    there are two engines: `asyncio.windows_utils` (which pulls `_winapi`/`msvcrt`) is imported
    inside `_pipe_handle` for the same reason, and `app/conpty.py` guards its `ctypes.wintypes`
    import on `sys.platform` — `wintypes` does not import at all on Linux. A gate that turns the
    *panel* off never protects an import — or a format string — on a shared path.
  - **The panel runs on Windows too, and it is TWO ENGINES, not one** (`app/conpty.py` +
    `termsock._motor_windows`, 22/08/2026). `terminal_panel` in `/api/config` is
    `termsock.painel_disponivel()` — a **capability** ("can a panel open here?"), never
    `os.name == "posix"`, which is what it used to say. POSIX is `pty.fork()` + `add_reader`;
    Windows is a ConPTY via ctypes whose pipes are fed to the Proactor's
    `connect_read_pipe`/`connect_write_pipe` — no thread, no queue, because
    `pause_reading()`/`resume_reading()` on `_ProactorReadPipeTransport` is a one-for-one
    replacement for `remove_reader`/`add_reader`. The shared front door (auth, Origin, session
    exists, cols/rows clamp) stays in ONE place; only the engine forks. Four things measured that
    bite whoever touches this:
    (1) **`STARTF_USESTDHANDLES` with all three handles NULL is mandatory** — without it
    `CreateProcess` propagates the *parent's* std handles, so in a service (stdout → log file) the
    child writes to the log and the pseudoconsole renders a **blank screen**, while `mode con`
    inside the child already reports the right size. Clearing `HANDLE_FLAG_INHERIT` does **not**
    fix it. Microsoft's own sample omits the flag and "works" only because its parent is a console
    app whose std handles are already console handles;
    (2) the ConPTY **input** pipe needs `duplex=True` — `_ProactorWritePipeTransport` fires a
    16-byte `ReadFile` on the write end just to detect closure, and `GENERIC_WRITE` alone returns
    WinError 5;
    (3) kill the child **before** `ClosePseudoConsole` (it can hang, microsoft/terminal#17716) —
    which is also the only safe teardown psmux allows;
    (4) there is **no size-restore step** on Windows, deliberately: psmux's window size follows
    whichever client is attached (the next client at 80x24 makes it 80x23 by itself), and
    `resize-window`/`setw window-size latest` both return rc=0 and do nothing there.
    `pywinpty` was tried and **removed**: it ships its own `conpty.dll`/`OpenConsole.exe` instead
    of using the system ConPTY, so it exposes no handle for asyncio; it also returns `str` from
    `read()` and opens a **listening** socket per session.
  While the panel is attached the window is at ITS size (~120x20), so anything that counts lines in
  the pane (option picker, AskUserQuestion stepper, `model_picker`) would read a truncated screen:
  `/select`, `/answer` and friends answer **409**, and the phone UI must **show that text** — the
  refusal explains the way out ("close the panel"), and a `catch` that only logs turns a tap into
  nothing at all.
- **O mesmo terminal no CELULAR** (`components/TerminalMobile.svelte`, aberto pelo botão Terminal do
  `Chat` quando `desktop` é falso): mesmo PTY, mesmo socket, mesma montagem do xterm — o que é
  compartilhado mora em `lib/xterm.ts` (`novoTerminal`/`temaDe`, onde vivem o fundo
  `rgba(0, 0, 0, 0)` e a fonte lida por `getComputedStyle`), e não em uma segunda cópia. O
  `TerminalMirror` (capture-pane a cada 450ms, texto cru) **continua existindo**: é o caminho quando
  `somente_leitura.terminal_panel` é falso — hoje isso não é mais "Windows", que ganhou motor de
  ConPTY em 22/08/2026, e sim qualquer máquina sem motor nenhum —, e o Chat escolhe pela config do
  servidor — otimista em `true` enquanto ela não chega, senão o primeiro toque cairia no espelho por
  causa de um fetch em voo. Três decisões medidas em 21/08/2026:
  - **A entrada são BYTES CRUS, não os nomes de tecla do `/term-input`.** Do outro lado está o
    `tmux attach`, que parseia a entrada como um terminal de verdade e reemite pro programa no modo
    que ELE espera (inclusive cursor-keys em modo aplicação): `\x1b[A` é exatamente o que a seta
    física manda. Texto e Enter saem no MESMO `send` (`valor + '\r'`) — dois envios abrem janela pra
    a TUI processar a linha antes do texto inteiro chegar.
  - **A fonte é o controle de COLUNAS, não só de legibilidade**, porque o tmux redimensiona a janela
    pro tamanho deste cliente enquanto ele estiver anexado (medido: 68x53 com o celular aberto,
    200x50 de volta ao fechar). Por isso trocar a fonte **não** remonta o terminal: muda
    `options.fontSize`, refaz o `fit()` e manda `resize` — remontar fecharia o socket e repintaria a
    TUI a cada toque em A+.
  - **O `Origin` do WebSocket precisa ser declarado quando o front vem de OUTRA máquina**
    (`CP_TERM_ORIGINS`, csv). O PWA carregado da VPS manda a Origin da VPS, que não é mesma-origem,
    não é a `public_url` e não está no `peers.json` — o handshake voltava **403** e a tela dizia só
    "desconectado". Vazio (o default) **não** pode virar "aceita qualquer um": o handshake também
    autentica pelo cookie `cp_token`, então origem arbitrária seria qualquer site abrindo um
    terminal na máquina.

## tmux + Claude Code truecolor

Inside tmux, Claude Code caps color depth to 256 and renders theme colors wrong (teal / pink / washed-out)
while rendering correctly outside tmux. Fix: `COLORTERM=truecolor` + `CLAUDE_CODE_TMUX_TRUECOLOR=1` in the
environment before `claude` starts (settings.json env is unreliable here). The installer covers this: the
`claude` wrapper sets both on every path, the backend passes them via `tmux new-session -e`, and the managed
`~/.tmux.conf` block (with `default-terminal "xterm-256color"`, not `tmux-256color`) sets them for hand-made
sessions. Only a `command claude` outside the wrapper still needs the exports in the shell rc. Full
explanation, reference config, and verify steps: [`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md)
and [`docs/tmux.conf.example`](docs/tmux.conf.example).

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
