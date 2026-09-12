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
- `adapters/codex/` — um app-server WebSocket de loopback por sessão Codex; o backend
  consome eventos JSON-RPC enquanto a TUI `codex --remote` da mesma thread roda no tmux.
  O app-server é do PANE, não do backend. Decisões e armadilhas:
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

Sessions must run as `claude --session-id <uuid>` **inside tmux** — `scripts/install-claude-wrapper.sh`
sets this up. A `claude` without an id, or outside tmux, is invisible to the app or flagged ⚠ no id.
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
quando o usuário pedir o fluxo ou o kick-off mandar invocá-lo com `Role:`. Após o planejamento
aprovado, executor e revisor independente trabalham com portão entre Tasks e revisão final da
branch. Push depende de autorização do usuário. Um escritor por árvore, execução serial por
padrão; cada sessão lê só a referência do seu papel. Contrato visual, exceções de paralelismo e
demais etapas ficam na skill.

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

Decisão nova entra assim: a regra aqui, a medição em `docs/decisoes/<assunto>.md`. Mecanismo que
deixou de existir vai para [`docs/decisoes/superado.md`](docs/decisoes/superado.md) — fica
registrado, fora do caminho de leitura, para não competir com o que vale hoje.

### Frontend — telas, CSS, i18n → [`docs/decisoes/frontend.md`](docs/decisoes/frontend.md)

- **Duas interfaces, não uma: front web (`frontend/`, Svelte) e app nativo (`mobile/`, Expo).**
  Lógica (API, formatação, parser, tipos) entra em `packages/core` e serve as duas. Tela é por
  interface, escrita duas vezes. Verificação é `npm run check` **na raiz**, que cobre as três.
- **Two views: mobile & desktop (820px).** `Sidebar` (desktop) e `SessionList` (mobile) são
  arquivos separados: template e CSS mudam nos DOIS e se verifica nos DOIS. Lógica da lista vai
  no `lib/sessionListModel.svelte.ts`, e a agregação SSE no `lib/sessionsStore.svelte.ts` — uma
  por servidor, nunca uma por card.
- **Todo texto de interface vem de `m.<chave>()`** (Paraglide). `pt.json` e `en.json` no mesmo
  commit. Dado do servidor não vira chave. A trava `i18nGuard.test.ts` só desce.
- **Markdown NUNCA aparece cru.** Todo `.md` exibido passa por `renderMarkdown`. Um `<pre>` com
  `**` e `##` à mostra é bug, não estilo.
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

### Harnesses — Claude, Codex, Pi, omp, Kimi → [`docs/decisoes/harnesses.md`](docs/decisoes/harnesses.md)

- **`omp` é um FORK do Pi** — mesmo JSONL, mesmas extensões, e as diferenças pequenas já custaram
  bugs calados (binário próprio, raiz `~/.omp/agent`, sem `--session-id`, outros nomes de evento,
  subagente no mesmo processo). Raiz do agente omp tem UMA resposta: `app/omp_dirs.agent_dir()`.
- **A lista de modelos NUNCA é constante.** Conta Anthropic lê o picker ao vivo (cache de 1h,
  porque ler dirige o terminal); sessão de motor usa `/v1/models` do provedor. `/model <id>`
  grava default global — reponha o valor anterior.
- **Antes de digitar no composer do Pi, PERGUNTE a ele** (`getEditorText`): a tela não distingue
  aviso de extensão de rascunho da pessoa, e comparar duas capturas não resolve.
- **Statusline e prévia vêm de sidecar do agente, não do pane.** O pane corta na largura da
  janela. `""` é resposta ("nada em voo"), `None` é ausência. Sessão Pi já aberta só publica
  depois de `/reload`.
- **O `wire.jsonl` do Kimi não é bem-comportado**: nem toda escrita é turno (`config.update` com
  a sessão parada), e o main fica mudo quando delega. Quem decide é a fronteira de turno, não o
  mtime. `tool.result` não tem `uuid` — id é `res:<toolCallId>`.
- **Integração nativa do Codex: o Codex converte, o backend decide quando, o lançador só avisa.**
  Dois gatilhos, e só: abertura de sessão Codex e o botão Reconciliar. Nunca gravar confiança
  para autoaprovar hooks. Fonte inválida nunca significa remoção.
- **Contas Codex adicionais têm `CODEX_HOME` próprio**; a identidade é `credential_id=codex:<home>`,
  nunca a chave. Sem migração, rotação ou troca automática por cota.
- **Instruções nativas do Codex entram por `AGENTS.override.md`** apontando para o `CLAUDE.md`.
  Override pessoal nunca é sobrescrito; onde existe `AGENTS.md` de verdade, ele deixa de ser lido.
- **A ponte de skills é a ÚNICA dona das pastas de ponte**, é stdlib-only, e só mexe em symlink
  cujo alvo está numa fonte conhecida. Config alheia é conferida, nunca editada.
- **Motor de modelo: `engines.py` é stdlib-only**, é `ANTHROPIC_AUTH_TOKEN` (nunca `_API_KEY`),
  o env entra por `execvpe` dentro do pane (nunca `tmux -e`, que expõe a chave no `cmdline`), e a
  janela é `CLAUDE_CODE_MAX_CONTEXT_TOKENS`.
- **Modo de permissão troca COM a sessão trabalhando** — é tecla, não texto. O guard de "está
  trabalhando" existe para o `/model`, que é texto.
- **Runtime por conta não vira atalho** (`telemetry/`, `feedback/`, `image-cache/`, caches por
  config dir) — senão cada reconciliação acha "deriva" e gaveta de novo.
- **O diálogo de confiança do Claude Code derruba três coisas**: a chave do pre-trust usa barra
  normal no Windows, `is_overlay` tem que ignorar as linhas em branco do fim do pane, e o Enter
  às cegas cai em "No, exit".
- **Loop runner**: `LOOP_DONE` só fecha com confirmação humana; guardrails são max_iters,
  branch≠main e kill-switch. Loop ativo suprime o chain.
- **Plugin e marketplace do Codex usam os comandos nativos do CLI.** Nome do plugin + origem
  confirmada identificam um alias (o mesmo pacote tem nome diferente em cada manifesto);
  associar só pelo nome deixa o plugin antigo executando. Hook de arquivo em subpasta preserva
  o caminho relativo inteiro — homônimo na raiz não o substitui.
- **Falha do CLI do Codex deixa diagnóstico privado**, nunca stdout/stderr cru no log do
  serviço: a saída pode transcrever tokens.
- **Quem segura a abertura de uma sessão Codex é a TUI parada num widget**, não a
  sincronização. Sem thread não há sidecar, e o app fica esperando para sempre — o cartão de
  seletor pré-thread existe para isso.
- **Pergunta assíncrona do Codex chega como `agentMessage` com `delivery: "async"`**, não como
  pedido JSON-RPC. Cada pergunta é independente; o eco da resposta local não responde outra de
  título igual.
- **O aviso de espera vem de `model/safetyBuffering/updated`**, não de temporizador local — o
  turno continua trabalhando.
- **A preferência da barra do Claude Code não autoriza sobrescrever `statusLine`**: desligada,
  o instalador preserva o que está lá.

### Windows → [`docs/decisoes/windows.md`](docs/decisoes/windows.md)

- **Windows roda psmux, não tmux** — e ele aceita comando que não executa. O que a doc do tmux
  diz que falha, aqui às vezes "funciona" errado: `%N` endereça a sessão errada, `kill-session`
  com `=` não mata, `rename-session` sobrescreve em vez de recusar, `list-clients` inventa tty,
  `set -g <qualquer coisa>` volta do `show -g`. Endereço é `=<sessão>:<janela>.<pane>`.
- **Multi-linha vai pelo CLIPBOARD**, porque os buffers do psmux cortam no primeiro `\n`. O
  fallback ramifica pelo **código de retorno**, nunca pelo nome do sistema.
- **O ambiente do pane vem do SERVIDOR no tmux e de QUEM CHAMA no psmux** — por isso
  `CLAUDE_CONFIG_DIR` não pode ser exportado incondicionalmente ali.
- **`Path.replace` É `os.replace`** e carrega o mesmo WinError 5; toda troca atômica passa por
  `atomico.substituir` (guarda de AST em `test_atomico_call_sites.py`).
- **Falha de decode em `subprocess` morre numa thread**: `run()` não levanta e `stdout` volta
  `None`. Use `errors="replace"` e não carimbe como bom o que tem U+FFFD.
- **`monkeypatch.setattr(os, "name", …)` leva o `pathlib` junto** e estoura longe de onde você
  aplicou. Em teste que faz isso, use `os.path`.
- **Código de retorno no Windows não se lê como falha** (`taskkill` sem processo devolve 128).
  Separe "comando não existe" de "comando falhou"; stderr vem na codepage do console.
- **Encoding é por interpretador**: `.cmd` em OEM, `.vbs` em UTF-16LE com BOM, `.sh` em UTF-8 sem
  BOM, `.env`/`settings.json` sem BOM, perfil do PowerShell com BOM.
- **O instalador NUNCA roda elevado** — admin é UAC pontual. Instalar elevado deixa as tarefas
  com dono Administradores e quebra todo `-Update` seguinte.
- **`ln -sf` do Git Bash COPIA e devolve 0**; confira com `test -L` depois. Script sem extensão é
  invisível para o PowerShell, e a falha é muda.
- **O navegador embutido precisa da sessão gráfica ATIVA**: com a janela ocluída o teclado entrega
  e o mouse não. View escondido precisa de `setDeviceMetricsOverride` para ter viewport e print.
- **Recado repetido no Windows não é o par insistindo** — é o oráculo de entrega: o argv entre
  Python e psmux come uma contrabarra quando o argumento vai entre aspas, a comparação falha e o
  reconcile redigita. Olhe `REQUEUE` no log antes de responder.

### Instalação, atualização e serviços → [`docs/decisoes/instalacao.md`](docs/decisoes/instalacao.md)

- **Quem serve a interface é o BACKEND; o `frontend/dist` chega pronto do CI.** Gate que pergunta
  "o front mudou?" tem que conhecer TODAS as árvores de onde o front é compilado (`frontend` **e**
  `packages`) — senão serve tela velha ou apaga edição local.
- **Instalador com portão de prova por etapa**: essencial que falha para na hora; extra opcional
  que falha entra na lista e o fim nunca diz "Pronto". Nada é dado como feito sem prova.
- **Instalador guiado: duas perguntas, o resto é padrão.** Sem terminal, tudo é NÃO. O log nunca
  carrega o token.
- **Atualizar pelo app faz tudo sozinho, mas nada é irreversível**: resgate antes de qualquer
  passo destrutivo, com a ref conferida. Passo só entra no registro depois da prova passar, e o
  registro é do que JÁ RODOU aqui — não do intervalo de commits.
- **Reiniciar o backend**: sem `--reload`; mate `-9` o pid da porta e suba destacado. No Linux é
  `systemctl --user restart`.
- **Criar sessão embrulha o tmux em escopo transiente do systemd, sob sonda** — um gerenciador que
  recusa escopo transiente derrubava toda criação de sessão.

### Plataforma — nomes, pareamento, planos, ditado → [`docs/decisoes/plataforma.md`](docs/decisoes/plataforma.md)

- **O nome antigo (`claude-pocket`) só existe em ponte de compatibilidade.** Escreva com o nome
  novo; nunca leia o antigo em código novo. O que resta é migração, e ela nunca funde duas pastas.
- **Comentário explica o PORQUÊ, e é curto.** Medição, versão de CLI e data envelhecem: num
  comentário viram afirmação falsa que ninguém revisa. Elas moram em `docs/decisoes/` e na
  mensagem de commit, datadas por construção.
- **Grupo: protocolo reinjetado no `SessionStart`, saída por UMA esteira, anti-loop no backend.**
  Varredura de sessão morta confirma ausência por TEMPO, nunca por número de polls.
- **Plan progress lê o `.md` do plano**, sem arquivo de estado: blocos cercados são removidos
  preservando offsets, e a decoração roda dentro do `to_thread` do git.
- **Ditado: a transcrição não é o problema, o que vem depois é.** Vocabulário vai para a Whisper
  (consertar depois é impossível por construção); regra de prompt só funciona com exemplo de
  entrada e saída; raciocínio no modelo de limpeza piora e não é calibragem. Quem manda no estilo
  é a pill que a pessoa leu antes de falar, não a config.
- **Revisão de código:** neste repositório, revisão local e as verificações do projeto.

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
