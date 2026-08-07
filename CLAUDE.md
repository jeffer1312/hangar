# claude-cockpit

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
```

Sessions must run as `claude --session-id <uuid>` **inside tmux** — `scripts/install-claude-wrapper.sh`
sets this up. A `claude` without an id, or outside tmux, is invisible to the app or flagged ⚠ no id.
The same installer also wraps interactive `codex`: it calls the local backend through `scripts/cp-codex`,
creates a managed Codex app-server/TUI pair, and attaches the caller to that tmux session. Codex
subcommands/advanced flags remain raw; `command codex` is the explicit bypass.

## Sessões-irmãs (cp-send) + pareamento

Sessões Claude da MESMA máquina se falam via `scripts/cp-send` (`--list`, `<sessao> "msg"`,
`--pair <sessao> "tarefa"`, `--unpair`, `--new <nome> [cwd] [--engine <motor>]`) — tudo sobre a API
local do backend (`/input`, `/pair`, fila durável). Pareamento = vínculo simétrico (`app/pair.py`,
sidecars em `<config>/.claude-pocket-pair/`) + prompt de protocolo injetado nas duas sessões; a UI
mostra chip 🤝 (Composer), badges nas listas, PairSheet (conversa do par + contrato compartilhado
`<a>__<b>.md` + split view desktop).

**Par noutro modelo:** `--engine <motor>` faz a sessão nova nascer num motor de
`~/.claude/engines.json` (ver "Model engines" nas convenções). Vale pra parear uma sessão Claude com
uma Kimi/GPT no mesmo trabalho: o par continua no MESMO `~/.claude` — skills, hooks, contrato
compartilhado, PairSheet, tudo igual —, só o motor difere, e o consumo vai pra conta do provedor.
O flag só repassa `engine` pro `POST /api/sessions`, então motor inexistente volta `400 motor
invalido` e a sessão **não** nasce (nunca uma sessão que parece estar no motor e não está). O texto
do protocolo que as sessões leem vive no heredoc de `scripts/install-cp-send.sh` — editar o
`~/.claude/CLAUDE.md` direto é perdido no próximo sync.

Skills do repo em `skills/` (symlinkadas em `~/.claude/skills/` pelo installer):
`orquestrar` — esta sessão vira líder de um grupo multi-repo (cria/pareia sessões via
cp-send, escreve o contrato do grupo, distribui escopo, monitora e consolida).

**Instalar/atualizar numa máquina** (após `git pull`):

```bash
./scripts/install-cp-send.sh          # symlink ~/.local/bin/cp-send + skills/* + bloco "Sessões-irmãs" no ~/.claude/CLAUDE.md (idempotente)
./scripts/install-claude-wrapper.sh   # symlink ~/.local/bin/cp-engine + wrapper claude-engine — sem isto,
                                       # motor configurado pelo celular abre um pane que morre na hora
                                       # (tmux new-session ainda retorna 0, o app reporta sucesso calado)
systemctl --user restart claude-cockpit-backend.service   # API de pareamento/preview
npm --prefix frontend run build                          # só se o front for servido estático (vite dev pega via HMR)
```

Sessões Claude já abertas não releem o CLAUDE.md global — só as novas conhecem o cp-send.
Escopo: pareamento e `--group` só dentro da mesma máquina. Recado 1:1 e `--list` alcançam OUTROS
servidores via endereço `servidor::sessao`: `backend/peers.json` (id → base_url+token, gitignored;
ver `peers.json.example`) + `CP_SERVER_ID` no `backend/.env`. Peer com `"enabled": false` sai da
VARREDURA (painel e `--list`) mas segue endereçável por `servidor::sessao` — é pra máquina que
você sabe que está desligada, senão cada poll paga o timeout de 4s esperando ela (id desta máquina, endereço de
resposta do `[de: id::sessao]`). Só o cp-send muda — o backend nem sabe da feature.

## SSE event model

The frontend `EventSource` (`screens/Chat.svelte`) listens for:

- `message` — transcript events: `user_msg` / `assistant_msg` / `tool_use` / `tool_result`.
- `state` — live state + status line (model / context / cost / rate badges).
- `preview` — live in-flight assistant text (full-replace; dropped when the real block commits).
- `ask_question` — opens the native AskUserQuestion sheet.
- `ping` — liveness heartbeat; resets a 25s watchdog that reconnects on half-open connections.
- `reset` — transcript swapped (e.g. `/clear`) → wipe and reload history.

## Conventions & gotchas (read before touching UI / backend lifecycle)

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
  `BottomSheet` com `wide={isDesktop} centered={isDesktop}` (o mesmo par que `EnginesSheet.svelte` e
  `Git.svelte` já usam; no celular continua folha subindo de baixo).
  O porquê é medido, não gosto: no dock de ~530px, rótulo + descrição à esquerda e um segmentado à
  direita brigam pela linha, e como o rótulo tem `min-width: 0` ele cede tudo — a descrição quebrava
  em **uma palavra por linha**. Tela de configuração é rótulo-e-controle repetido dezenas de vezes;
  ela precisa de largura, e largura é o que o dock não tem.
  Ao mexer em qualquer tela dessas, use **container query** (`container-type: inline-size` no
  wrapper + `@container`), nunca media query: quem aperta a linha é a largura do PAINEL, não a da
  janela — num monitor de 1440px o dock tem 530px e uma media query de 560px nunca dispara ali.
  **Direção acordada com o usuário, ainda não implementada:** juntar todas as configs num modal
  único de "Configurações" com abas (Aparência · Geral · o que vier), em vez de uma folha por
  assunto. Quem for fazer: o `lib/gitTabs.ts` + `GitTabs.svelte` já são o precedente de navegação
  por abas dentro de um modal, incluindo nível por aba no celular.
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
  restart claude-cockpit-frontend.service` + reload ignorando cache. Verificação pós-edição de front
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
  Sidecar em `.claude-pocket-loop/<nome>.json` (sobrevive `/clear`); guardrails: max_iters,
  branch≠main, kill-switch `automations_enabled`, anti-estagnação (mesma cauda 2×). Loop ativo
  **suprime o chain** da sessão. Campos `loop_status/loop_iter/loop_max` fluem no `/api/sessions`
  e no `sig` do SSE (badge 🔁 nas 2 views). Spec/decisões: docs/superpowers/specs/2026-07-22-*.md.
- **Model engines** (`app/engines.py` + `app/engine_probe.py` + `components/EnginesSheet.svelte`):
  a session can run on a non-Anthropic provider — only env vars change inside that session's process,
  `~/.claude` (skills, hooks, transcript) stays the SAME. Single source of truth at
  `~/.claude/engines.json` (0600). Four invariants: (1) `engines.py` is **stdlib-only** — an
  `app.config` import there would pull in pydantic and break `scripts/cp-engine`, which the shell
  calls with the system `python3`; (2) it's `ANTHROPIC_AUTH_TOKEN`, **never** `ANTHROPIC_API_KEY`
  (that one writes `customApiKeyResponses` into the global `~/.claude.json`); (3) the env is applied
  by `cp-engine --exec <engine> -- claude …` (`os.execvpe` inside the pane) and **never** via
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
  `components/ModelEffortSheet.svelte`). Duas fontes, escolhidas pelo que a sessão é — medido em
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
- **Pi model + thinking level** (`app/pi_models.py` + `scripts/pi/cp-state.ts` + `components/PiModelSheet.svelte`):
  the third mechanism, next to Claude's TUI picker and Codex's app-server, and it does **not** scrape
  the pane. Measured on pi 0.82.1: `/model` is a fuzzy-**search** list of ~300 entries (footer
  `(1/301)`, 10 rows visible) — not enumerable from the pane and not navigable by counting `Down`;
  and there is no `/thinking` command (it lives inside `/settings` → "Thinking level", a submenu).
  So the Pi extension we already ship publishes a catalog sidecar
  (`<config>/.claude-pocket-pi/models/<jsonl-stem>.json`, same key as the state marker) and registers
  `/cp-model <provider> <id>` + `/cp-think <level>`, which the backend types with `send-keys` and Pi
  applies through `pi.setModel()` / `pi.setThinkingLevel()`. Two invariants: (1) the thinking levels
  are **per model** (glm-5.2 → off/low/medium/high/xhigh; k3 → low/high/max), so they come from the
  session, never from a constant — the static `LEVELS` tuple only rejects garbage before typing;
  (2) Pi **clamps** the level to what the model supports (`agent-session.js:1277`), so the endpoint
  re-reads the sidecar and returns what *stuck*, not what was asked (asking `max` on glm-5.2 lands on
  `xhigh`). Missing sidecar → 409 telling the user to re-run `install-claude-wrapper.sh`, never an
  empty list that reads as "no models".
- **Statusline por sidecar, não pelo pane** (`app/statusline.py` + `scripts/omniroute-statusline.js`
  + `scripts/pi/rich-status-line.ts`): a linha que o app mostra (modelo, contexto, ⚡5h/📅7d, custo)
  **não** sai do transcript — quem a calcula é o agente, e o app só via o texto **já renderizado no
  terminal**, cortado na largura da janela. Medido 2026-07-30 num pane de 99 colunas: o Pi chama
  `truncateToWidth` e a linha morre em `cache…` (somem contexto, cota e custo); o Claude quebra em
  várias linhas, mas quando a quebra cai em cima do par de contexto ele vira `💬 769k/238 770k…`.
  Nos dois casos o painel dizia "medição indisponível" **por causa do tamanho do terminal**.
  Contrato: quem RENDERIZA publica a linha inteira (sem ANSI) em
  `<config>/.claude-pocket-status/<stem>.json` = `{"line", "ts"}` — mesma chave dos outros
  marcadores (o stem do `.jsonl`) — e `statusline.read()` a prefere ao pane, caindo nele quando não
  há sidecar (sessão sem instrumentação **nunca** pode ficar sem linha nenhuma). Três detalhes que
  já custaram bug: (1) o tmp do `tmp+rename` leva o **pid**, porque o script do Claude roda a cada
  render e duas invocações da mesma sessão se sobrepõem (nome fixo → `rename` promovendo bytes
  entrelaçados, o mesmo furo que `cp_panel_common.py` já corrigiu); (2) `read()` exige **dict** —
  JSON válido do tipo errado (`null`, lista) não levanta `ValueError` e o `.get()` derrubava a
  resolução de estado de TODAS as sessões em `list_with_state`; (3) o publicador do Pi vive na
  extensão porque a linha completa só existe dentro do processo dele — logo, **sessão Pi já aberta
  só passa a publicar depois de `/reload`** (o Pi carrega extensão na largada), enquanto o lado
  Claude vale na hora, por ser script executado a cada render.
- **Prévia ao vivo: sidecar do agente primeiro, pane depois** (`preview.read_sidecar` +
  `scripts/pi/cp-state.ts`): mesmo contrato da statusline, agora pro texto **em voo**. A extensão do
  Pi recebe o bloco do assistente token a token (`message_update`) e publica o **último bloco de
  texto** em `<config>/.claude-pocket-preview/<stem>.json` = `{"text", "ts"}`; `PreviewBroker._loop`
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
  Vale o mesmo aviso da statusline: **sessão Pi já aberta só publica depois de `/reload`**. Claude
  Code não tem essa API de extensão e segue no pane; Codex nunca raspou pane (app-server).
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
  named keys and the option picker all work. **`paste-buffer` does not** — hence the `paste_text`
  fallback, which branches on the **return code**, not on the OS: a multiplexer that lacks it says so,
  and on Linux the fast path returns 0 and never reaches plan B. Plan B is one `send-keys -l` per line
  with `C-j` between; a `\n` *inside* the argument makes psmux swallow everything after it, and `\r`
  as a separator glues the lines together (both measured). Probe: `scripts/test-psmux.py` (+ `.ps1`).
  Install: `install.ps1`. Not there on Windows: systemd services and the `claude`/`codex` shell
  wrappers, so a session you open in the terminal is invisible to the app — app-created ones are fine.
- **Session creation's systemd-scope probe.** Creating a session wraps `tmux` in
  `systemd-run --user --scope` so the tmux server doesn't inherit the backend's cgroup, but the wrap
  is now gated on a probe: a systemd user manager that refuses transient scopes was making **every**
  session creation fail (app and terminal both). Failing the probe, sessions are created without the
  scope and the backend logs a warning (commit `23da052`).
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
  - **`term-<name>` is keyed by NAME and nothing renames it.** It outlives an agent session killed
    outside the app, so `new_hidden_shell` compares `#{session_path}` (the birth directory — measured
    that a `cd` inside the pane moves `pane_current_path` and **not** this one) and recreates on
    divergence, and `rename()` kills it: reattaching a shell from the previous repo under the new
    session's label is a command typed in the wrong directory.
  - **POSIX-only imports (`pty`, `fcntl`, `termios`) live INSIDE the functions.** `termsock` is
    imported by the 409 guard that also runs on Windows; a top-level `import fcntl` there is a
    `ModuleNotFoundError` that breaks a feature which works today. The panel itself is gated by
    `config.somente_leitura.terminal_panel` (`os.name == "posix"`), but a gate that turns the
    *panel* off never protects an import — or a format string — on a shared path.
  While the panel is attached the window is at ITS size (~120x20), so anything that counts lines in
  the pane (option picker, AskUserQuestion stepper, `model_picker`) would read a truncated screen:
  `/select`, `/answer` and friends answer **409**, and the phone UI must **show that text** — the
  refusal explains the way out ("close the panel"), and a `catch` that only logs turns a tap into
  nothing at all.

## tmux + Claude Code truecolor

Inside tmux, Claude Code caps color depth to 256 and renders theme colors wrong (teal / pink / washed-out)
while rendering correctly outside tmux. Fix: `COLORTERM=truecolor` + `CLAUDE_CODE_TMUX_TRUECOLOR=1` in the
environment before `claude` starts (settings.json env is unreliable here). The installer covers this: the
`claude` wrapper sets both on every path, the backend passes them via `tmux new-session -e`, and the managed
`~/.tmux.conf` block (with `default-terminal "xterm-256color"`, not `tmux-256color`) sets them for hand-made
sessions. Only a `command claude` outside the wrapper still needs the exports in the shell rc. Full
explanation, reference config, and verify steps: [`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md)
and [`docs/tmux.conf.example`](docs/tmux.conf.example).
