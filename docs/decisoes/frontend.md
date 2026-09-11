# Frontend — telas, CSS, i18n, terminal embutido

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Planejamento no chat (07/09/2026)

`SessionModeControl` é o controle compartilhado de
  Claude e Codex. Ele ocupa a linha inferior do compositor; quando os controles e seus rótulos
  não cabem, passa para a primeira linha existente, sem acrescentar uma faixa vertical. A medida
  considera o texto cortado pelo flex do celular, para não esconder o modelo só para acomodar o modo.
  Shift+Tab percorre todos os modos disponíveis no Claude, como Alt+Shift+P; no Codex,
  alterna Normal e Planejar. O monitor reaproveita a captura do pane para publicar o modo e lembrar
  o último modo fora do planejamento por identidade de sessão. Sondas e trocas controladas
  não deixam seus modos intermediários contaminarem essa memória.
  Pedidos `item/tool/requestUserInput` do Codex têm identidade própria: JSON-RPC distingue
  pedido de resposta pela presença de `method`, mesmo quando os IDs coincidem. O cliente
  guarda pendências independentemente do SSE; `serverRequest/resolved` e o fim do turno
  retiram a pergunta de todos os clientes. Fechar o formulário não responde ao servidor.
  A confirmação de um `proposed_plan` é uma ação local da TUI, não um pedido JSON-RPC:
  implementar muda para Normal e envia o pedido de implementação. Tags em linhas próprias
  são retiradas da apresentação, preservando exemplos dentro de cercas de código.
  O plano nativo do Claude usa `/plan-preview`, separado de `planprog`: escritas confirmadas
  pela sessão identificam o arquivo. Um `slug` isolado não prova que existe plano, pois o
  Claude também o grava em conversas comuns. A prévia busca o conteúdo atualizado ao abrir;
  visualizar não aprova execução.

## Duas interfaces, não uma: o front web (`frontend/`, Svelte) e o app nativo (`mobile/`, Expo).


  Mudança de comportamento tem que passar pelas DUAS, e a pergunta certa não é "qual arquivo eu
  edito", é "de onde essa lógica vem". Quem responde é `packages/core`: 90 arquivos do app nativo
  importam dele, sempre pelo barrel `@hangar/core`, e o front web também — foi pra lá que os 46
  arquivos de `frontend/src/lib` migraram. Daí a regra prática:
  - **Lógica (chamada de API, formatação, parser de transcript, tipos) entra no `core`** e serve as
    duas de uma vez. É onde o esforço rende, e é onde um teste cobre as duas.
  - **Tela é por interface, sem exceção**: `.svelte` no front web, `.tsx` no app nativo. Não há
    componente compartilhado e não vai haver — Svelte e React Native não renderizam a mesma coisa.
    Feature nova de tela é escrita duas vezes, de propósito.
  - **A verificação é `npm run check` (ou `npm run test`) NA RAIZ, e ela cobre as três** — core,
    front web e app nativo. Não era assim: cada uma tinha o seu comando, nenhum cobria o outro, e
    quem mexesse no core e rodasse só o `check` do frontend levava verde com o app quebrado. O app
    entra por `scripts/verificar-app.mjs` — não por não ser workspace (ele é), e sim porque o
    `npm ci` do CI, do deploy e dos instaladores é **seletivo** e não instala as dependências dele.
    A regra desse script é a que dá sentido a tudo: **dependência do app ausente FALHA com código 1
    e diz o que fazer**, nunca é pulada em silêncio — um check que passa por não ter olhado é pior
    que um que não roda. Complementos: um hook em `.claude/settings.json`
    (versionado, vale para quem clonar) avisa ao editar `packages/core`, e o CI segue compilando só
    front e backend, por decisão — o build do app é no Expo.
  - **Texto de interface é um `project.inlang` só, compilado três vezes** (`frontend/src/paraglide`,
    `packages/core/src/paraglide`, `mobile/src/paraglide`, todos gerados e gitignored). Chave nova
    nasce nos dois `messages/*.json` da raiz e já vale para as duas interfaces; o que muda é só quem
    a compila.
  Dentro do front web, a divisão continua sendo esta:

## Two views: mobile & desktop (820px breakpoint).

`App.svelte` switches on
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
  - **A LÓGICA da lista também mora num lugar só** — `lib/sessionListModel.svelte.ts`
    (`createSessionListModel({ variant })`): agrupar/colapsar/filtrar, seleção+broadcast+comparar,
    abrir/excluir/renomear/retomar/Git. `Sidebar` e `SessionList` instanciam o modelo com a
    sua variante e só mantêm o chrome próprio (rail/pin/kebab/hover/menu no desktop; drawer/feed/
    scroll no celular). O que diverge entre as views está na tabela `RULES` no topo do arquivo, uma
    regra por linha (como a preferência gravada é lida, modo efetivo de agrupamento, ordem dos
    grupos por servidor, rótulo que o filtro casa, ordem no Comparar, restaurar o servidor ativo
    depois da ação) — convergir é trocar um valor ali, de propósito. Lógica nova da lista entra no modelo, com teste nas DUAS variantes;
    template e CSS continuam por view, e o aviso de "mudar e verificar nas DUAS" vale para eles.
    Loop segue no `SessionList` — só o celular abre pela lista; dar Loop ao desktop seria feature.

## i18n: todo texto de interface vem de `m.<chave>()`

(Paraglide, `frontend/src/paraglide/` gerado;
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

## iOS black-rectangle repaint.

Glass on NavBar/Composer lives in a `::before` leaf with a near-opaque
  solid bg and **no** `backdrop-filter` / `transform` / `translateZ` on WebKit — those promote a layer that
  renders pure black during momentum scroll. Don't reintroduce them. Liquid-glass blur is Chromium-only
  (`html[data-liquid]`).

## Transparência é padrão do app, não enfeite de uma tela.

O app tem papel de parede
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

## Config e opção moram em MODAL, não em painel docado.

Decisão de desenho de 2026-07-30, vale
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
  todas as telas num `BottomSheet` de navegação por seções (Aplicativo · Servidor) com as linhas de
  `LINHAS` — hoje Geral, Aparência, Diário, Sobre (aplicativo) e Máquinas, Contas e modelos,
  Harnesses, Voz, Notificações, Anexos, Avançado, Orquestração (servidor). Quem for adicionar aba: registra no `LINHAS` do `SettingsModal.svelte` e no
  `lib/configRoute.ts` (`TelaConfig`/`TELAS_DE_SERVIDOR`), com chave de idioma nos dois
  `messages/*.json` no mesmo commit. O `lib/gitTabs.ts` + `GitTabs.svelte` continuam sendo o
  precedente de navegação por abas DENTRO de uma tela (incluindo nível por aba no celular).
  Servidores e Acesso viraram **Máquinas** (2026-09-04); as rotas antigas seguem por
  `RENOMEADAS`. Dentro de Máquinas as duas listas — a do navegador (`cp_servers`, este aparelho
  acompanha) e a do servidor (`peers.json`, os servidores se falam) — são **uma linha por máquina,
  casada pelo identificador** (`lib/maquinas.ts`, `unirMaquinas`), com duas caixas. O identificador
  da outra máquina vem dela mesma (`GET /api/peers/identificador` com o token que o navegador
  guarda); nada no navegador o persiste. Casar pela URL viraria duas linhas (IP da LAN no celular,
  Tailscale no servidor). O interruptor nunca muda sozinho: `checked` é o dado, o `onchange` repõe
  o dado e chama a ação, e a ação confirmada é quem muda a lista.

## Bloco rolável dentro da conversa é `position: relative`, ou o `.sr-only` dele vaza pra lista


  (`EditDiff.svelte`, 10/09/2026). Sintoma relatado por dois usuários e nunca reproduzido à mão:
  "espaço vazio no fim do chat que rola sem acabar", às vezes, em sessão trabalhando — e, pior, a
  partir daí mensagem nova não entrava (chegava no terminal, não no app); sair e voltar resolvia.
  Causa, medida por CDP na janela do app no momento do bug: o diff de um Write de 68 linhas tem um
  `<pre>` de 2284px dentro do `.ed-split` de 46vh com `overflow: hidden`; cada linha carrega um
  `<span class="sr-only">` (`position: absolute`, `app.css`), e absoluto só é cortado pelo overflow
  do seu **bloco de contenção** — o `.ed-split` era `static`, então o bloco de contenção ficava
  fora dele e os 200 spans das linhas escondidas contavam na área rolável da CONVERSA:
  `scrollHeight` = base do último `sr-only`, 1464px além do fim do `.messages-inner`, com todos
  os filhos visíveis e de altura normal. A segunda metade do sintoma é consequência: com o fim
  nunca alcançável, `atBottom` (folga < 64px) ficava falso, a janela de eventos congelava e o
  chat parava de mostrar o que chegava. Prova: `position: relative` aplicado ao vivo no
  `.ed-split` da janela do usuário zerou a sobra; reverter trouxe de volta. O mesmo vale pra
  qualquer bloco com overflow próprio que abrigue um `.sr-only` (o `.ed-uni` levou junto). A
  sonda `chat.vazio_no_fim` (`MessageList.svelte`) fica: é o que apontou o elemento.

## The message list is windowed.

`MessageList.svelte` mounts only the last `WINDOW=120` events; scroll-to-top
  reveals older pages (in-memory, no backend call). Don't render the whole transcript at once.

## Queue/pending dedup.

Messages sent while Claude is `working` echo as `pending` / `queued-` bubbles and
  reconcile against the real transcript by normalized text/line. Touch `Chat.svelte` dedup carefully.

## The phone app renders `AskUserQuestion` natively.

The `ask_question` SSE event opens the
  `AskQuestionSheet` stepper; since the pending payload isn't in the jsonl, a PreToolUse hook
  (`askq_capture.py`, installed idempotently by `hook_installer.py`) captures it into a sidecar. Verified
  live: use AskUserQuestion freely, it shows as the stepper. Numbered plain text is only a fallback for a
  session where that capture hook isn't installed. Raw TUI option pickers (not the tool) surface separately
  via `OptionButtons` (selection sent by `terminal_input.py`), so free composer text does not answer a picker.

## Vite HMR servindo componente VAZIO (stub de ~800 bytes).

Medido 2× em 2026-08-04 (vite 8.1.0 +
  vite-plugin-svelte 7.1.2 + svelte 5.56.4): depois de editar um `.svelte` com a página aberta, o dev
  server passa a servir o módulo transformado como um stub sem template (`function X(...) { ...; return
  $.pop(...) }` e mais nada) — o componente monta ZERO nós, SEM erro no console e SEM overlay do Vite.
  Sintoma na UI: a tela/componente some (ex: o chat inteiro vira papel de parede; cliques em cards não
  fazem nada porque a rota nunca monta). O `svelte-check` passa — o arquivo está bom, quem corrompeu é o
  cache de transform do dev server, e ele vale pra TODOS os clientes (não é por-browser). Diagnóstico:
  `fetch('/src/<modulo>')` na página — stub tem <1KB e não contém o markup. Remédio: `systemctl --user
  restart hangar-frontend.service` + reload ignorando cache. Verificação pós-edição de front
  SEMPRE inclui abrir a tela afetada e conferir que ela montou (não só o `check`/`vitest`).

## Markdown NUNCA aparece cru.

Todo conteúdo `.md` exibido no app passa por `lib/markdown.ts`
  (`renderMarkdown`) com tipografia própria — contrato do par (`PairSheet`), prompt/transcript de
  subagente (`ActivitySheet`), plano, README, qualquer arquivo lido do disco. Um `<pre>` com
  `**Tarefa:**` e `##` à mostra é sempre bug, não estilo. Vale também pro que vem de fora do
  transcript: se o texto é markdown, renderize.

## CSS animations.

Shared tokens/keyframes live in `app.css` (`--ease-out`, `--spring`, …); a global
  `prefers-reduced-motion` rule neutralizes loops, so new keyframes don't each need their own guard.
  **Animação de `transform` NUNCA em `<svg>`, `<g>` ou `<path>` — só em elemento HTML** (medido
  05/09/2026, `icons/HangarWorking.svelte`, Chrome 150 no Electron 43). O indicador de "trabalhando"
  girava `<path>`s dentro do SVG: o Chromium não compõe isso na GPU, e cada quadro refazia style +
  layout + paint da PÁGINA INTEIRA — 144 layouts e 576 paints em 3 s numa página com 3 sessões
  trabalhando. Custo real: os dois renderers visíveis do app desktop a ~96% de CPU cada e o
  gpu-process a 139%, por horas, com o JS ocioso (o profiler mostrava 85% em `(program)`). Pausar só
  essas animações via CDP levou os renderers a 0–11%. Três armadilhas no conserto, todas medidas:
  (1) mover a animação pra RAIZ do `<svg>` tirou layout e paint, mas o compositor ainda recusou
  (`compositeFailed=1024`, `kTransformRelatedPropertyCannotBeAcceleratedOnTarget`) — num Chrome
  headless avulso a mesma raiz compunha, dentro do app não; quem compõe de verdade é um `<span>` em
  volta do svg; (2) a propriedade `rotate` (individual, usada pra compor com `transform` no mesmo
  elemento) também não compõe — compor é aninhar spans, um por transformação; (3) nome de
  `@keyframes` passado por `var()` a partir do markup não recebe o escopo do Svelte (só o nome
  escrito na folha é reescrito) — a espiral final ficou meses sem rodar por isso, calada; escolher
  por `:nth-child` no CSS. Diagnóstico reutilizável: CDP na 9223 com `Tracing`
  (`disabled-by-default-devtools.timeline`) contando `Layout`/`Paint`/`UpdateLayoutTree` por 3 s —
  composto é ~3 eventos, não-composto é um por quadro; e `blink.animations` traz o
  `compositeFailed` de cada animação ao (re)iniciar. Depois do conserto: 3 eventos em 3 s e os
  renderers a ~9%.

## Real terminal in the desktop footer

(`app/termsock.py` + `components/TerminalPanel.svelte`,
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

## O mesmo terminal no CELULAR

(`components/TerminalMobile.svelte`, aberto pelo botão Terminal do
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

## Fechar `x` e criar outra `x`: a chave do Chat e a marca de exclusão eram só o NOME

(10/09/2026).
No desktop o `Chat` remonta por `{#key serverId::nome}`; com o mesmo nome a chave não muda, o
componente da morta segue montado com a conversa antiga e estado `dead` (sem reconectar), e o que
o usuário via era a conversa do Claude enquanto o Codex de mesmo nome subia — o backend serviu o
transcript certo em todas as aberturas do dia (medido no log). Hoje `sessionsStore.epoca()` sobe
quando um nome some dos `slots` e volta (`epocasDeRecriacao`, no core), e as duas `{#key}` do
`DesktopShell` a incluem. Segundo furo do mesmo nome: `markDeleting` escondia `serverId::nome` até
a lista vir SEM ele; recriada antes disso, a nova ficava escondida pra sempre (a varredura via um
`x` na lista e mantinha a marca). A marca agora guarda o `jsonl` da excluída, e `sweepHidden` só a
mantém pra sessão com o mesmo nome E mesmo jsonl. Vale nas duas interfaces (store do front e
`mobile/src/stores/sessions.ts`). O cache de cauda do chat (`chat-cauda`, TanStack) já era por
jsonl e não entrou nisso. Não reproduzido de ponta a ponta pelo navegador embutido: o item
"Fechar" do menu de contexto não aceitou o clique sintético.
