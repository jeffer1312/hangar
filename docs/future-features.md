# Future features (backlog — requested 2026-06-25)

Next things to design + build after the current redesign phases. Mobile-first. The backend
drives the live claude via tmux send-keys + reads the JSONL transcript + capture-pane.

## 00. Board: canvas livre (✅ ENTREGUE v1 — 2026-07-17)

> **ENTREGUE v1 (2026-07-17).** `screens/Canvas.svelte` na rota `#/canvas`, convivendo com `#/board`
> como um terceiro modo desktop (não substituiu o quadro). Decisões tomadas:
> - **Convive** com `#/board` — o board continua sendo o modo agrupado por estado.
> - **Drag pelo handle + resize nativo** (CSS resize corner capturado por `ResizeObserver`).
> - **Layout persistido** em `localStorage` sob `cp_canvas_layout`, keyed `serverId::name`.
> - **Posição inicial em colunas por servidor** via `lib/canvasLayout.ts` (`placeNew`, 6 testes) —
>   atende o pedido "separasse por servidores pra não ficar tudo jogado".
> - **Pareados nascem adjacentes, mas NÃO movem juntos** (decisão conservadora: agrupamento visual
>   no nascimento, sem arrasto solidário).
> - **Sem pan/zoom** — scroll nativo do container.
> - **`BoardCard` ganhou a prop `fill`** (o card preenche o tile dimensionável em vez do tamanho fixo do board).
> - **Mobile cai na `SessionList`** (canvas é desktop-only). Overlay do chat = rota `#/canvas/<serverId>/<nome>`.
> - **Smoke ao vivo passou:** drag/resize/reload persistem, overlay abre no servidor certo com 2
>   servidores, Esc restaura, mobile intocado.
>
> **Ficou de fora (v2):** mover-junto de pareados (arrasto solidário do grupo 🤝); auto-arrumar
> contínuo (reflow quando entra/sai sessão); poda de entradas mortas do `cp_canvas_layout` (sessões
> que sumiram deixam a posição órfã no localStorage). O texto original do pedido segue abaixo.

**O pedido, nas palavras dele:** *"queria poder ajustar a altura e a largura de cada card
individualmente"*, e — sobre os cards de um grupo de pareamento — *"os que estão em pareamento
deveriam ficar diferentes e agrupados"*.

**A decisão:** foram apresentadas 3 opções (altura-por-card + largura-por-coluna; canvas livre;
altura + span de 2 colunas). Ele escolheu **canvas livre**, ciente do trade-off explicado:

> Largura individual por card **quebra o conceito de coluna** — num kanban a coluna define a
> largura; se cada card tem a sua, não há mais alinhamento vertical, e o que existe é um canvas.

Então isto **não é um ajuste do quadro atual — é uma tela nova**. O que se ganha: liberdade total
de posição e tamanho. O que se perde: o agrupamento automático por estado, que é o eixo do quadro
de hoje (nada se reorganiza sozinho quando uma sessão muda de estado). Decidir se o canvas
**substitui** `#/board` ou convive como um terceiro modo é parte do design — pergunte antes.

**O que existe hoje pra reusar** (`#/board`, entregue 2026-07-16):
- `screens/Board.svelte` — agregação SSE (1 stream por *servidor*, nunca por card), colunas por
  estado, Maps içados de `drafts`/`pending`/`sendError` (o card remonta ao trocar de coluna).
- `components/BoardCard.svelte` — o card é um mini-chat: cauda via `GET /history?limit=N` no mount,
  input que envia cross-server, botões de opção, recibo de erro. **Reusável quase inteiro** — o que
  muda é quem posiciona/dimensiona o card, não o que ele mostra.
- O overlay do chat completo é a rota `#/board/<serverId>/<nome>`.

**Restrições que NÃO podem ser afrouxadas** (custaram bugs reais nesta feature):
- **Nunca um SSE por card** — o navegador limita ~6 por host. Estado ao vivo vem do stream agregado.
- **O servidor ativo tem que ser apontado ANTES do render** (síncrono, no `hashchange`), senão o
  Chat monta e busca no servidor errado — ver `applyRouteServer` em `App.svelte` e o commit `5288ea4`.
- **Espiar um card não muda em que servidor você está** (`lib/peek.ts` + `peek.test.ts`). Essa regra
  já foi apagada uma vez por um refactor; o teste é o que a protege.

**Pontas soltas do quadro atual, a decidir junto** (podem morrer com o canvas, ou não):
- **Agrupar os pareados** (o outro pedido do mesmo dia): um grupo 🤝 é uma unidade lógica (N sessões
  na mesma tarefa) e deveria ser um bloco, não N cards soltos. A **lista já faz isso** — o cluster
  colapsável do commit `6497b8f` (`SessionList`/`Sidebar`); o quadro não herdou. Num canvas isso
  vira "cards do mesmo grupo nascem juntos / movem juntos"?
- **Organizar por servidor** — ele levantou (*"separasse por servidores? pra não ficar tudo jogado"*)
  antes de escolher o canvas. Num canvas, isso provavelmente vira posição inicial/auto-arrumar.

**Dica de implementação (só uma):** persistir posição+tamanho por `serverId::name` no localStorage,
no mesmo padrão dos `drafts` do Board. E cuidado com o que já mordeu: card é flex item — em qualquer
container flex, `flex-shrink: 0` (ver `23e137d`, os cards nasceram espremidos e vazios com 13 sessões).

## 0. Git manager (EM CONSTRUÇÃO — iniciado 2026-07-03)

Objetivo: transformar a GitSheet num **gerenciador de git** de verdade, aberto pelo **menu de
contexto** de uma sessão (botão direito) — SEM precisar abrir a conversa. O repo vem da própria
sessão (`name` + `serverId` já estão no menu). Referência visual do usuário: commit graph estilo
GitLens (histórico com a árvore, autor, branches/tags).

- **Fase 1 — DESKTOP FEITO (commitado 2026-07-03):** menu de contexto da Sidebar → item **Git** abre a
  GitSheet no repo da sessão via `selectServer(serverId)` (restaura o server ativo no fechar), sem entrar
  no chat; o botão **log** da GitSheet lista os commits da branch (backend: action `log` no `git_ops` + no
  `Literal` de `api.py`, corrigido e reiniciado). Log inline bugado removido.
  **FALTA O MOBILE:** a `SessionList` não tem menu de contexto de sessão — o `SessionCard` só faz
  long-press→renomear. Pra retomar: criar um menu (long-press ou botão ⋯) no `SessionCard` com o item
  **Git** e renderizar a GitSheet a partir dele (mesmo padrão do Sidebar: apontar o server + restaurar).
- **Fase 2:** grafo visual dos commits (as linhas da árvore + branches/tags), estilo GitLens.
- **Fase 3:** trocar branch e demais ações (stash, pull…) integradas no mesmo gerenciador.

**Melhorias de UX pedidas (feedback 2026-07-03, olhando a GitSheet aberta):**
- **log pouco visível ("não vê nada"):** hoje o botão `log` só joga os commits no `<pre>` de output
  (rodapé, cortado). Dar uma **view dedicada** que ocupa a sheet (igual o visualizador de diff), com a
  lista de commits legível (hash + msg + autor + data).
- **arquivos alterados ruins de ler:** os nomes longos (ex. `docs/superpowers/plans/2026-…`) truncam feio.
  Melhorar: **basename em destaque + path/dir menor** (ou wrap), pra bater o olho e entender.
- **renderizar código/diff com syntax highlighting** estilo editor do VS Code — **só VISUALIZAR, não
  editar**. Ex.: Shiki / highlight.js / Prism inline (self-contained, sem CDN). Aplicar no visualizador de
  diff e, se fizer sentido, no log.
- **log = UMA LINHA POR COMMIT (estilo TortoiseGit):** hoje o texto faz wrap e fica ilegível. Cada commit
  numa linha única (hash + msg + autor + data), truncada com ellipsis se longa; sem wrap. Idealmente scroll
  horizontal ou tap pra expandir um commit.
- **sheet redimensionável:** deixar o usuário **aumentar o tamanho** da sheet lateral (arrastar a borda /
  handle, ou um botão de largura) — a de 420px fica apertada pra ler log/diff.

Fazer nas **2 views** (Sidebar desktop + menu equivalente no mobile) — ver o gotcha das 2 views no CLAUDE.md.

## 1. See running agents (subagents + workflows)
A way to view, from the phone, what's executing inside the live claude session: the
running **Agent(...)** subagents and **Workflow** runs (mirrors what the terminal shows —
`Agent(...) Running…`, `+N tool uses`, `ctrl+b to background`). 
- Source: the JSONL transcript already records subagent activity (tool_use entries / agent
  spawns); workflow progress + subagent transcripts live under the session's
  `.../subagents/workflows/<runId>/` (journal.jsonl, agent-*.jsonl). Parse those for a live
  "Agents" panel (name, phase, state, tokens, elapsed). 
- UI: a panel/sheet listing active agents/workflows with live status; tap to see detail.
- **TodoWrite task list** (same panel): Claude Code renders a "N tasks (M done)" block with
  ✔/◻ rows; surface it in the app too. Cleanest source = the **TodoWrite tool_use entries in
  the transcript** (structured: subject + status), not the pane. Render a tasks panel with
  progress. Batched here with agents/workflows (same "ambient activity" surface).
- Open question: how much is reliably parseable from the transcript vs the workflow files;
  whether to show tool-use stream inline in the chat as collapsible cards.

## 2. Attachments — send + view images (audio later)
- **Send images** — ✅ DONE (2026-06-25): `POST /api/sessions/{name}/upload` (raw bytes) saves to
  `<cwd>/.claude-pocket-uploads/`; the composer has a 📎 picker + paste-into-textarea; on send it
  uploads and sends `"<caption>\n📎 imagem: <path>"` and the assistant reads the path. Lazy
  (upload-on-send, no orphan if cancelled).
- **CLEANUP / retention (deferred — needed):** uploaded images pile up in
  `.claude-pocket-uploads/` forever. Add a retention sweep — on backend startup (and/or periodic),
  delete files older than N days (e.g. 7d) or keep the last N. Simple, no extra endpoint. (User
  explicitly flagged this; deferred but must happen.)
- **View images** that appear in the chat: the transcript can carry image content blocks
  (user-attached or tool results); render them as inline image bubbles (currently only text).
  Also: persist/serve the uploaded images so they survive reload (v1 only shows the path marker).
- **Audio** — deferred (the user will tackle later): voice input/output.
- General **attachments** (files) — same upload-to-cwd + reference pattern as images.

## 3. Surface interactive prompts (AskUserQuestion / menus) in the app
The app already renders Claude Code's native selection menus: `state.py classify()` detects
`❯ N.` cursor + numbered options (`_CURSOR_RE`/`_OPTION_RE`) → `awaiting_input` → the app's
`OptionButtons`. So the capability EXISTS.
- Gap: the assistant's `AskUserQuestion` tool widget didn't show on the phone. Most likely it
  was just the **401** (app got no events). Possibly the classifier doesn't match its exact
  render (multi-select? different markers? the question/preview layout?).
- Plan: once auth is fixed, trigger an `AskUserQuestion` in a session the app is viewing and
  check if `OptionButtons` appear. If not, capture the pane and extend `classify()`/the option
  parser to recognize the widget (and carry multi-select + per-option descriptions).
- Why it matters: the user drives sessions from the phone; interactive prompts must be
  answerable there (see memory `claude-pocket-app-interaction`).

## 4. Pending fixes (batch — this session)
- **Cost chip → top row of the composer** (a thin row above the textarea), so the model pill
  gets more room in the control-left. Composer-only change.
- **401 self-heal:** today an invalid/rotated token leaves the app wedged — `isAuthenticated()`
  only checks that a token EXISTS, not that it's valid, so the app keeps 401ing and shows
  `undefined` session. On a 401 from the API, clear creds (`clearCredentials`) and bounce to
  Login so the user can re-pair (in-app QR scanner). Critical for REMOTE use (user can't clear
  site data from the phone easily). Lives in `lib/api.ts` (fetch wrapper) + the router.
- **Keyboard / top-bar bug (iOS):** when the keyboard opens, the NavBar (top bar) disappears
  and an accessory bar with a check shows above the keyboard. NEEDS a screenshot to pin down
  (user is remote → blocked until image-upload exists or user is at the PC). Likely the
  document-lock/`100dvh`/visualViewport-transform pushing the NavBar out of the visual viewport,
  plus iOS's native input-accessory bar.

> **Priority note:** image **upload** (item 2) is now higher priority — the user collaborates
> from the phone (away from the PC) and currently has NO way to send screenshots for debugging
> (e.g. the keyboard bug above). It unblocks the whole remote feedback loop.

## 5. Git / branch control from the phone (requested — not built)
The app only **shows** the current branch (read-only chip in the composer, from the statusline)
and a `is_git` badge in the project picker. No way to act on git from the phone.
- **Branch switch (lazy MVP):** make the branch chip tappable → sheet lists local branches →
  switch. Backend `git -C <cwd>` via **argv list** (never a shell string); validate the target
  against the listed branches (trust-boundary input — rejects injection + typos). No
  `checkout -b`/arbitrary git in v1.
- **Arbitrary git commands:** already possible by typing them to the live claude session
  (send-keys). The new value is the branch-switch *control*, not a generic git terminal — a
  run-any-command endpoint is an RCE-class footgun even on LAN; skip it.
- Open: also surface quick actions (status/pull) as fixed buttons in the same sheet?

## Notes
- These build on the existing infra: SSE stream, transcript parser, send-keys input, the
  HTTPS/secure-context (Tailscale), and the redesigned composer (a natural home for an
  attach button next to the slash/model controls).
- Prior backlog (UI polish, separate-statusline-from-badge, etc.) is largely addressed by
  the redesign; see docs/polish-backlog.md + docs/ui-redesign-proposal.md.
