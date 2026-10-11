# Frontend — telas, CSS, i18n, terminal embutido

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Paraglide compila um módulo por idioma (11/10/2026)

Regra: toda compilação do Paraglide (`i18n:compile` do core, do front e o `i18n:compile:core` do
mobile, e o plugin do `vite.config.ts`) usa `locale-modules`. As duas formas escrevem na mesma pasta;
misturar faz uma desfazer a outra.

Medido na árvore do `verificar-local`, `vitest --maxWorkers=4`, Node 22:

| Suíte | Um módulo por mensagem | Um por idioma |
|---|---|---|
| `@hangar/core` (59 arquivos) | 101 s (import 374 s somados) | 23 s (import 55 s) |
| `frontend` (188 arquivos) | 833 s (import 3004 s) | 252 s (import 687 s) |
| Arquivos gerados (core + front) | ~15.800 | 27 |
| Bundle principal do `vite build` | 3,77 MiB, gzip 1,06 MiB | 3,77 MiB, gzip 1,03 MiB |

O padrão `message-modules` existe para descartar mensagens não usadas no bundle, mas o app usa quase
todas, e o bundle não mudou. O custo estava nos testes: cada arquivo de teste importa algum módulo com
`m.*()`, e o vitest, que isola cada arquivo, transformava e carregava os ~8 mil módulos de novo.
O build do Expo com o core nesse formato não foi conferido.

## Resize não é encolhimento de conteúdo na lista nativa (05/10/2026)

A compensação do fim (`ListState::hold_tail`, teto de 160 px) compara alturas antes e depois de
uma mudança de conteúdo. A régua e a folga só valem na geometria em que foram medidas. Quando a
largura ou a altura muda, `List::prepaint` invalida `tail_ref`, `tail_slack` e `tail_visible`
antes da remedição; a âncora e os handles permanecem. Não se resolve com `reset` da lista nem
com a retirada dos caches de desenho. A próxima medição reconstrói a referência; sem resize,
encolher conteúdo continua criando folga limitada e crescer continua consumindo-a.

Medição em compositor Wayland isolado, pele Terminal e conversa longa sintética: na base,
1200×800 → 820×600 acrescentava 160 px de vão além dos 67 px do aviso de plano. Após uma nova
mensagem e a restauração, sobravam 54 px artificiais. Com a invalidação, fim do conteúdo/composer
ficaram em 594/661 na janela grande, 394/461 na pequena, 394/461 após mensagem enquanto pequena
e 594/661 ao restaurar sem outro evento: só os 67 px do aviso, sem folga artificial. As decisões
puras de geometria e compensação ficam em `elements/list_tail.rs`, fonte usada pela lista e
incluída na suíte do app; isso não equivale a executar a suíte completa do GPUI vendorizado.

## Nova sessão: busca entre raízes e seleção de conta (05/10/2026)

Com texto e “Pesquisar em todas as pastas” marcado, o modal nativo procura nos filhos das
raízes autorizadas do servidor escolhido. Não varre todo o disco nem outros servidores. Sem
texto, permanece a navegação normal; desmarcado, a busca é local. A preferência nasce marcada
e é salva ao alternar, mesmo fechando sem criar. O seletor compacto da tela sem sessão mantém
sua busca local na pasta navegada: não oferece os controles globais do modal. Cada raiz tem
cache e geração: digitar refiltra
sem novo scan; trocar servidor invalida respostas anteriores. Resultados deduplicam por caminho,
mostram a origem, mantêm homônimos e usam sua própria raiz ao selecionar ou abrir. Falhas por
raiz aparecem junto dos resultados válidos; tentar novamente repete só as leituras com erro.

O foco na busca é dado uma vez ao abrir, não a cada resposta. Ctrl+Tab e Ctrl+Shift+Tab percorrem
as raízes circularmente e preservam a busca, somente no modal. Ao trocar o provedor, a conta
ChatGPT compatível é mantida ou a primeira elegível é selecionada; conta sem identificador ou
credencial válida não entra no seletor. O catálogo e o payload usam a conta exibida. No Codex,
a criação nova prefere conta conectada, sem substituir uma conta pedida em transferência.

Medição com fixture sintética: digitação imediata encontrou pasta de outra aba; preferência
falsa sobreviveu ao fechamento sem criar e ao reinício; dois projetos homônimos mostraram suas
origens. Conta ChatGPT 1 foi escolhida sem clique adicional, trocar para a 2 refez o catálogo e
mudar para provedor compatível preservou a 2. O POST de criação levou a conta 2 e o caminho do
resultado de outra raiz. Duas leituras do contador antes/depois de mudar a busca deram 17 scans
em ambas: a digitação não consultou o disco novamente. A fixture não cria sessões reais.

## Reiniciar pela bandeja não é instalar atualização (05/10/2026)

“Reiniciar Hangar” relança somente o app desktop pelo caminho do executável capturado no
startup, preservando argumentos. A confirmação de atualização revalida a exclusão mútua ao
iniciar a procura: um diálogo aberto antes não pode atropelar o reinício. Usa o handshake de
single-instance: o processo antigo só sai depois da prova de vida do filho; falha restaura a
instância anterior, mostra a janela e um erro próprio. Reinício manual não entra no estado de
falha de instalação nem oferece download ao tentar novamente. Backend, terminais e sessões
não são encerrados. Disponível nos menus Linux e Windows; o macOS continua sem bandeja.

Medição no Linux com hospedeiro de bandeja em barramento D-Bus isolado: o menu real expôs
Abrir/Reiniciar/Sair, e dois pedidos de reinício seguidos deixaram um único processo e ícone
novos, com a configuração preservada. No Windows 11 em VM, clique real no item do menu trocou
o processo antigo por um único filho na mesma sessão gráfica, falando com a fixture sintética.
Renomear o executável em execução para `.old` e pôr a cópia no caminho normal também relançou
pelo caminho normal. Remover o caminho de relançamento preservou o processo anterior nos dois
sistemas; no Linux, um filho que saiu sem o handshake também manteve a instância anterior e
mostrou o erro próprio. Os binários da prova eram isolados dos instalados.

## Voltar fecha a imagem ampliada antes de navegar

(04/10/2026.) O visor compartilhado (`frontend/src/lib/visor.ts`) tratava Escape e arrasto,
mas não registrava sua abertura no histórico. No PWA, o Voltar do Android navegava para a
tela anterior enquanto a imagem continuava aberta.

`viewerHistory.ts` cria uma entrada temporária com a mesma URL, preservando os campos do estado
anterior. O primeiro `popstate` fecha o visor; como o fragmento da URL não mudou, o roteador
continua na conversa. Fechar pelo botão, Escape ou arrasto consome essa entrada uma única vez.
Trocar a mídia aberta não empilha outra entrada. Reabrir ou executar uma ação espera a retirada
pendente, para um Voltar atrasado não fechar a nova tela. Os listeners saem ao fechar.

Conferido no navegador Chromium em viewport de celular, com o visor de produção e uma página
de teste de navegação: imagem → Voltar fecha o visor e mantém a conversa → próximo Voltar
navega à lista. Escape e botão de fechar também mantiveram a conversa, e o Voltar seguinte
navegou à lista sem uma parada extra. A verificação simulou o Voltar pelo histórico do navegador;
não foi executada em um aparelho Android físico.

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
  Rótulo de stub/fixture de teste que vive em árvore varrida pela trava é identificador
  (`abrir-term`), nunca frase: numa orquestração de setembro/2026 um stub escrito como frase disparou
  a trava e tentou uma exceção global no `i18n-allow.json`; renomeado pra identificador, o extrator
  voltou vazio e o build real mostrou que ele não vaza pro produto. A regra saiu da skill
  `orquestrar` (é deste repo, não do fluxo) e mora aqui.

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

**Who owns the PTY.** With the `hangar-server` up, every panel is a Rust PTY, Windows included
(`crates/hangar-server/src/term/`): the owner on 8765 goes straight to it, and whoever reaches the
Python (share guest, guest with login, owner via Connect) passes the `termsock` front door and is
piped to `/__hangar_server/term`. One panel per session across ALL ports lives in the Rust `Terms`;
the 409 asks it (`term.active`), and `terminal_panel` comes from its health. The Python engines
below run only in the `python` reserve mode.

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

## Aba ativa do navegador embutido é UMA só, compartilhada entre painel e CLI

(14/09/2026).
O navegador da sessão ganhou abas (até 8). A alternativa era cada lado ter o seu foco — a pessoa
numa aba, o agente noutra —, e ela quebra o que o preview existe pra fazer: o print que o agente
lê deixaria de ser a tela que a pessoa está olhando, e "vê como ficou" viraria duas telas
diferentes discutindo a mesma. Então a ativa é única: clicar numa aba no painel muda onde os
próximos comandos do CLI caem. Para o caso legítimo de mexer numa página sem tirar a outra da
frente existe `--aba <id>`, que age na aba escondida sem trocar a ativa — o print dela custa 2-3 s
a mais, porque o Electron precisa render a view que não está composta na tela.

O sidecar `~/.hangar/nav/<chave>.json` é **aditivo** pelo mesmo motivo prático: `url` e `targetId`
no topo continuam sendo os da aba ativa, e `ativa`/`abas` entram ao lado. Três leitores dependem do
topo — `backend/app/navsock.py` (espelho do celular e teclado remoto, que resolve a sessão por
`sc.get("targetId")`), `backend/app/navshell.py` e o `GET /api/sessions/{name}/navegador` que o
front usa pra saber se a sessão tem navegador. Mover qualquer um desses campos para dentro de
`abas` quebraria os três de uma vez, e a suíte do backend passando **sem mudança** é a prova de que
não quebrou.

## O clique do preview some depois que a aba escondida navega

18/09/2026, Linux/Hyprland, painel fora da tela, página local escrita para o caso. Com um ouvinte
de clique em fase de captura no `document`, o gatilho é **navegação de documento**, e não a página
nem a geometria:

- `open clinica.html` do zero, `click @e9` repetido → **15 de 15 entregues**.
- `eval 'location.reload()'` e repetir o mesmo `click` → **0 de 8**, sempre respondendo `ok`.

No estado quebrado, descartados por medição: **coordenada e elemento coberto** (`innerWidth` 1280,
`innerHeight` 800, `scrollY` 0, `getBoundingClientRect` certo e `document.elementFromPoint` no
centro calculado devolvendo o próprio botão); **página congelada** (um `setInterval` seguia
contando); **canal** (`Input.dispatchMouseEvent` por CDP cru em
`ws://127.0.0.1:9223/devtools/page/<id>`, nas mesmas coordenadas, entregou **zero** eventos — o que
desmente a leitura anterior, de que o CDP externo entregava e o `webContents.debugger` não).

O que sobra é o **quadro**. No mesmo estado:

- `press a` → keydown **não** chega; `hover @e9` → mousemove **chega**. Só o evento discreto morre.
- `Runtime.evaluate` esperando dois `requestAnimationFrame` **pendura**. Não há quadro sendo
  produzido — o view escondido não compõe, e depois de navegar não volta a compor sozinho.
- `Page.captureScreenshot` (o `shot`) devolve clique e tecla na hora.
- `Emulation.setDeviceMetricsOverride` com o **mesmo** 1280×800 não muda nada; com **1280×801**,
  devolve. É a mudança de medida — surface nova — que reancora, não a reemissão do comando.

Daí a forma do conserto, em `shell/preview_ctl.cjs`: o gancho `aoNavegar` reaplicava a emulação com
a medida idêntica, que é justo a que não reancora; agora ele pede `medir()` com a altura vizinha
antes da real, e só ali (`definirOculto` e `layout` já trocam a medida por conta própria). E o
`click`/`press` deixou de responder `ok` sem conferir: arma um ouvinte em captura antes de
disparar, lê o marcador depois, reancora e tenta **uma** vez, e só então responde `erro:`. Marcador
sumido conta como entregue — é documento novo, ou seja o evento navegou a página.

**A sonda do clique escuta `pointerdown` E `mousedown`, nessa ordem, e não só o segundo.** Um
`preventDefault()` no `pointerdown` SUPRIME o `mousedown` de compatibilidade, e é exatamente o que
todo combobox do Radix faz. Com a sonda só em `mousedown`, um clique que CHEGOU era lido como não
entregue, e a retentativa disparava um segundo `pointerdown` que alternava o componente de volta ao
estado inicial — resposta `erro:` numa tela que não mudou, a mesma classe de falha que a sonda veio
eliminar. Medido numa página feita para o caso, com contador por tipo em captura: um `click` dava
`pointerdown: 2`, `pointerup: 2`, `click: 2`, **`mousedown` ausente**, `aria-expanded` de volta em
`false`. O `pointerdown` a página não consegue suprimir; por isso ele é o primeiro da lista.

**Por que parecia depender da página:** não dependia. `open` numa aba que já existe é `loadURL`, ou
seja navegação. Abrindo `fluxo.html` e depois `clinica.html` na mesma aba, quem falha é a segunda;
invertendo a ordem, falha a outra. O mesmo no site real: o primeiro clique num link da barra
lateral navega, e da navegação em diante nada mais chega — a URL parece presa.

**Antes de concluir que uma página não reage, confirme com um ouvinte em captura**
(`document.addEventListener('click', …, true)`) que o evento chega. Um teste inteiro já foi
conduzido sobre cliques que nunca chegaram.

Não bate com [a entrada do Windows](windows.md#o-navegador-embutido-funciona-no-windows--com-a-sessão-gráfica-ativa),
que mediu a janela **ocluída**: lá a sessão gráfica é que não existe; aqui a aba tem sessão e só
não tem quadro.

## Navegar uma aba CONGELADA derruba a sessão do depurador

18/09/2026. `a aba escondida nao descongelou; tente de novo` é o `economia()` do `preview_ctl`
falhando, e o `shell.log` diz por quê: `[nav] aba nao foi para active: Not attached to an active
page`, quatro vezes (o laço de retentativa). Depois disso todo verbo é recusado até `close` +
`open`, e o alvo na 9223 fica com `url: ""`.

A causa é o **congelamento**. A aba escondida e ociosa vai para `frozen`, e navegar um documento
congelado o descarta junto com o `DevToolsAgentHost` — o `webContents.debugger` fica sem página.
Medido, com o mesmo par de páginas e o mesmo `open` numa aba que já existe:

- navegações emendadas, sem ociosidade entre elas: **0 falhas em 16** (a aba nem chega a congelar);
- com 6 s de ociosidade antes de cada `open`: **6 falhas em 16**.

Por isso o `preview_ctl` ganhou a janela `navegando(true/false)`: enquanto há carga em voo, a aba
escondida fica acordada, custe o que custar em GPU — é segundo, não minuto. O `main` abre a janela
**antes** do `loadURL` que ele mesmo dispara (aí a aba está congelada desde o último verbo, e o
`did-start-loading` chegaria tarde) e a fecha no `did-stop-loading`, que também cobre a navegação
que um clique causou.

No mesmo caminho havia um segundo jeito de emular em cima da troca de página: `hangar:nav-open`
chamava `trocarAba` → `avisarOculto` → `definirOculto` → `setDeviceMetricsOverride` logo depois do
`loadURL`, e o guarda só recusava `about:blank`/URL vazia — logo depois do `loadURL` o `getURL()`
ainda devolve a URL **antiga**. O guarda agora conta navegação em voo (`isLoadingMainFrame()`) e
espera `did-stop-loading`; `did-finish-load` não serve porque carga que **falha** não o emite, e a
aba escondida ficaria sem medida nenhuma.

## Arrastar sessão sobre sessão pra formar o grupo de trabalho (18/09/2026)

Gesto nas quatro superfícies — Sidebar, Board, Canvas, lista do celular —, regra pura em
`packages/core/src/pairDrop.ts` (`canPair`/`canLeave`), estado compartilhado em
`frontend/src/lib/arrastarGrupo.svelte.ts` e um único `GrupoDropDialog.svelte` montado no
`App.svelte`.

- **Soltar não pareia direto: abre diálogo de confirmação.** `join_group` (`backend/app/pair.py`)
  funde os grupos INTEIROS dos dois lados — não só origem e alvo, os pares de cada um também — e
  escreve o texto do protocolo na conversa de cada membro resultante; não existe desfazer. Sair do
  grupo confirma pelo mesmo motivo: o backend avisa quem saiu e quem ficou (`api.py:4138`). Por
  isso o `GrupoDropDialog` lista todos os afetados, não só os dois nomes arrastados — a lista é a
  união de `origemSessao`/`alvoSessao` com o `pair_peers` de cada um (`afetados`,
  `GrupoDropDialog.svelte`), relida do `sessionsStore` a cada render em vez do objeto capturado no
  clique: um SSE que mude o grupo (ou mate a sessão) enquanto o diálogo está aberto não pode
  confirmar dado velho.
- **No Canvas o alvo válido é só a faixa de cabeçalho do card (`HEADER_HIT_H = 34`, em
  `canvasLayout.ts`), não o corpo inteiro.** Lá arrastar já significa MOVER o tile e tiles se
  sobrepõem livremente — sobrepor corpos continua sendo só mover, senão qualquer reorganização
  visual dispararia um pedido de parear. O hit-test (`findDropTarget`) varre cards e depois grupos
  recolhidos na ordem INVERSA da lista recebida, que é a ordem de renderização: sem inverter, dois
  cabeçalhos sobrepostos pareavam sempre com o card desenhado primeiro (o de BAIXO na tela), nunca
  o que a pessoa está vendo por cima.
- **No celular o arrasto sai de uma alça na trilha do swipe** (`SessionCard.svelte`), porque os
  outros três gestos já estão ocupados: toque longo abre o menu de ações, swipe horizontal abre a
  trilha, arrasto vertical é a rolagem da lista — não sobrava um gesto livre pra "arrastar a
  linha". Duas rodadas de correção depois de escrito (`21b6bf3e`): o hit-test caindo sobre a
  própria origem (dedo ainda dentro da trilha recém-aberta) tem que virar "sem alvo" em vez de
  piscar como recusado antes de qualquer movimento real; e o cabeçalho de um cluster de pareamento
  precisa resolver pra um membro representante (1º da lista), senão soltar ali caía no "fundo" da
  lista e pedia SAÍDA do grupo da origem — nada a ver com o que a pessoa mirou.
- **Limite aceito: em tablet na largura desktop (iPad com a `Sidebar`) o arrasto HTML5 não responde
  ao toque, e não haverá gesto ali** — o caminho é o `PairSheet`. Esse mesmo `PairSheet` é também a
  alternativa SEM arrasto exigida pela WCAG 2.2 SC 2.5.7 (todo atalho de arrastar precisa de um
  caminho equivalente por clique/toque simples): o arrasto é atalho, nunca o único jeito de parear
  ou sair de um grupo.
- **Cross-server não entra pelo arrasto.** `canPair` recusa com `cross_server` quando origem ou
  alvo já tem algum `pair_peers` com `::` (peer remoto) — o backend recusaria a mesma combinação
  com `400 erro_pareamento_mistura_cross` (`pair.py:169`), então o front nem deixa tentar. Parear
  entre máquinas continua só pelo `PairSheet`/`hangar-send`.

## A aba que NASCE com a sessão fora da tela vem 0×0

17/09/2026. A skill promete que a página escondida é medida em 1280×800, e o desenho entrega isso:
`definirOculto(true)` liga `VIEWPORT_OCULTO` em `shell/preview_ctl.cjs`, e o `nav-hide` do
`main.cjs` chama isso quando o view sai da tela. O que falha é o estado INICIAL. Em
`shell/main.cjs:688` (e `:813`):

```js
const escondida = oculto ?? (anterior ? !anterior.view.getVisible() : false);
```

Sem `oculto` explícito e **sem aba anterior** — o caso de um navegador nascendo do zero para uma
sessão que não está na tela — o padrão é `false`. O controlador nunca recebe `definirOculto(true)`,
não liga a emulação, cai no `clearDeviceMetricsOverride` e o agente lê uma página de 0×0: sem
layout, `snapshot` e `shot` de uma tela sem dimensão.

**Saída enquanto não for consertado:** `hangar-preview layout 1280 800` (modo custom, que aplica
`setDeviceMetricsOverride` direto). `layout desktop` NÃO serve — ele cai no ramo que limpa a
emulação e devolve o tamanho real, que é zero. Confira com `eval 'innerWidth'` antes de concluir
que a página está vazia.

## Navegador embutido com janela estreita

22/09/2026. Em uma janela Electron de 758×366 com outra conversa aberta, `hangar-preview open`
registrava o pedido de uma sessão Codex sem terminal, mas nenhum navegador nascia. A identidade
estava correta: abaixo de 820px o `DesktopShell` não montava, e o chat sozinho não mantinha o SSE
da lista que entrega os pedidos `nav`. O `App` agora retém o `sessionsStore` enquanto a ponte nativa
existe e o login/sync permite entrar. O contador existente compartilha a conexão com as telas.
Conferido no mesmo app e largura: o pedido passou a criar o navegador de uma sessão fora da tela.

## Pares entre servidores e nomes compridos na lista

22/09/2026. `jefferson-2` no notebook e `setup-vm` no Delphi-02 apareciam em dois grupos de um
membro: cada ponta tem seu próprio `pair_gid`, e a lista separava servidores antes de agrupar.
Pares remotos recíprocos agora são reunidos antes da divisão por servidor/projeto. A identidade
vem de `/api/peers/identificador`, consultado uma vez após um quadro válido com par remoto;
nome ou rótulo do servidor não basta. Os IDs originais continuam nas ações e no arrasto.

O grupo mostra a origem de cada sessão. Títulos compridos quebram linha dentro da largura da
barra. Na prova real, o par apareceu uma vez com contador 2, sem exceder a largura do cabeçalho.
Os servidores offline ficam em um resumo recolhido; expandi-lo não dispara consultas.

## Dois clientes desktop: o PWA no Electron e o app nativo em Rust (27/09/2026)

Registro histórico: a exigência de paridade abaixo foi substituída pela decisão
[Desktop no Rust; web para PWA/mobile](#desktop-no-rust-web-para-pwamobile).

Com o app nativo (`desktop-native/`, Rust/GPUI) publicado na release `native-latest`, o desktop
passou a ter dois clientes do mesmo backend: o front web (`frontend/`), que o Electron de `shell/`
carrega, e o nativo. Toda mudança agora tem dois lugares para entrar, e sem regra os dois se
afastam calados: o nativo não importa `packages/core`, então nada do que vai para o core chega
nele sozinho.

- **O que os dois compartilham**: a API do backend e os textos de `messages/*.json` (o nativo lê
  os dois arquivos em `desktop-native/src/i18n.rs`, com as chaves próprias no prefixo `native_`).
  Lógica que puder morar no backend serve os dois clientes de uma vez; lógica no `core` serve só
  o web e o celular.
- **Mudança de tela ou comportamento na visão desktop do `frontend/` entra também no nativo no
  mesmo trabalho.** Se não der, vira uma linha `pendente` com o motivo em
  `desktop-native/docs/chat-parity.md`. Vale no sentido inverso: o que nascer no nativo entra no
  web ou fica registrado ali.
- **Diferença deliberada** entre os dois também vai para `chat-parity.md`, com o motivo, para
  não ser "corrigida" depois como se fosse descuido.

### Feito (28/09/2026): o nativo virou o padrão

Decisão do usuário, depois dos testes de uso do dia. `install.sh`/`install.ps1` chamam
`scripts/install-native.sh`/`.ps1`, que escolhem o pacote pela máquina (Linux x64, macOS Apple
Silicon, Windows x64), conferem o sha256 do `native-latest.json`, instalam o app com o atalho
"Hangar", gravam a conexão inicial com o token (só se o app ainda não tiver uma) e deixam a marca
`~/.hangar/native/release.json`. Máquina sem build na release fica com o Electron. O Electron
continua instalado como "Hangar (Electron)": o navegador embutido do `hangar-preview` mora nele.
As máquinas já instaladas recebem a troca pelo passo `2026-09-28-app-nativo-padrao`. As linhas
`pendente` de `chat-parity.md` seguem abertas; o usuário aceitou trocar com elas.

### Feito (01/10/2026): o Electron saiu do instalador

Pedido do usuário: o nativo passa a ser o único app de desktop baixado. `install.sh`/`install.ps1`
não rodam mais o `npm ci` do `shell/`, não gravam o lançador `hangar.desktop` nem os atalhos
"Hangar (Electron)", e o fim da instalação abre o nativo ou, sem ele, o navegador. Instalação
existente mantém o Electron que já tinha (o botão Atualizar não roda o instalador). No Linux o
navegador embutido com tela remota e abas do `hangar-preview` só existia no Electron, então
instalação nova no Linux ficou sem ele até o nativo do Linux passar a atendê-lo (entrada de
02/10/2026 sobre o Chromium sem janela).

O plano original da troca, mantido como registro:

- `shell/hangar.desktop`: `Name=Hangar (Electron)`. O `install.sh` (linha que grava
  `$APPS_DIR/hangar.desktop`) regrava o lançador a partir desse arquivo, então editar só o
  lançador instalado volta atrás na próxima instalação.
- `install.sh`/`install.ps1`: baixar o nativo da `native-latest`, conferir o `.sha256` e registrar
  o lançador "Hangar" dele (`StartupWMClass=com.hangar.native`, o `app_id` de
  `desktop-native/src/main.rs`).
- `desktop-native/README.md` e `docs/USAGE.md`: trocar "não substitui o Electron" pela instrução
  nova.
- Antes: fechar ou aceitar por escrito as linhas `pendente` de `chat-parity.md` e os módulos
  listados ali como fora do nativo (terminal, navegador, Git completo, voz, Board/Canvas).

## Desktop no Rust; web para PWA/mobile

30/09/2026. Jefferson definiu a descontinuação gradual do Electron/desktop web.
Toda alteração voltada ao desktop passa a ser feita no cliente Rust (`desktop-native/`).
A web (`frontend/`) continua recebendo o que precisa funcionar no celular, pelo navegador
ou como PWA, sem exigir tela, layout ou implementação equivalente para desktop web.
O aplicativo móvel Expo (`mobile/`) continua com seu escopo próprio.

Isso substitui a obrigação de alterar `Sidebar` e `SessionList` juntas e a paridade
bidirecional entre desktop web e Rust. Pendências de implementação no desktop web em
`desktop-native/docs/chat-parity.md` passam a ser histórico, não exigência para concluir
trabalhos. Recursos móveis continuam sendo implementados nos clientes móveis pertinentes.

O pedido muda a direção do desenvolvimento; não remove o Electron nem telas existentes.
Código compartilhado deve preservar os usos atuais. API e textos seguem compartilhados
com o Rust; `packages/core` continua servindo web e Expo. Dependências ainda atendidas pelo
Electron permanecem até substituição ou remoção explicitamente autorizada.

## Navegador do app nativo: CDP dentro do processo no Windows (29/09/2026)

29/09/2026. No Windows o app nativo (`desktop-native/`) atende o `hangar-preview` e as tools MCP de
navegador sem o Electron. Cada sessão tem o seu WebView2, e o app o dirige por CDP dentro do
próprio processo, **sem porta de depuração**. O servidor local `/cmd` segue o mesmo contrato do
shell Electron (`~/.hangar/nav/_srv.json`); o sidecar grava o `pid`, o `hangar-preview list`
confere o navegador nativo por esse pid, e o `navshell._chave` do backend acha o sidecar sem
`targetId` de CDP.

- **Nativo e Electron abertos juntos disputam o `_srv.json`, e vale o último que subiu.**
- **Sem abas**: `tab ...` e `--aba` respondem `erro: o app nativo ainda nao tem abas: e um
  navegador por sessao`.
- **`open` só é atendido com o servidor ativo do app nesta máquina por loopback**
  (`127.0.0.1`/`localhost`). Com outra máquina ativa ele não faz nada, e a marca do pedido
  pendente espera o prazo dela.
- **`open` com a sessão na tela cria o navegador, mas não abre a aba Navegador sozinho** (o
  Electron monta o painel). Linha `pendente` em `desktop-native/docs/chat-parity.md`.
- **A tela remota do navegador no celular** (`backend/app/navsock.py`) ficou só no Electron até
  02/10/2026; hoje o nativo a atende pelo repasse `/cdp` (entrada abaixo).
- **macOS não muda**: um navegador por janela, sem controle pelo CLI. O Linux passou a ter o
  mesmo arranjo do Windows em 02/10/2026, sobre um Chromium sem janela (entrada abaixo).

Medição (29/09/2026, build debug, app rodando como Administrator, DevTools desligado):

- **Navegador na tela:** viewport do tamanho do painel (1045×838); `click` chega com
  `isTrusted=true`; `fill` com acento ("ação") ok; `press` ok; `shot` ~0,3 s; `layout mobile`
  390×844 ok; `tema escuro` ok; dois comandos simultâneos entram em fila; console capturado; as
  tools MCP (navshell) leem url e snapshot.
- **Navegador escondido:** com `set_visible(false)` o Chromium parava de compor quadros —
  `click`/`fill`/`press` não chegavam e o `shot` estourava 15 s. Regra: navegador escondido fica
  visível para o Windows, estacionado fora da área do app (x=-3000 lógico, 1280×800). Medido
  depois: `visibilityState=visible`, `click` com `isTrusted=true`, `shot` ~220–240 ms. Custo: cada
  navegador escondido continua desenhando.
- **Foco:** `Engine::focus(true)` passa o foco do teclado à página (WebView2 `MoveFocus`); sem
  isso, digitar logo após o Enter na barra de endereço não chegava à página.
- **App fechado:** `list` mostra MORTO e os verbos respondem "fora do ar"; ao reabrir, sidecars de
  outro pid são apagados.
- **Atalho `hangar-preview.cmd` + PowerShell 5.1** estragam aspas e `||` em `eval` (anterior a
  este trabalho); pelo Git Bash funciona.

## Tela remota do navegador no app nativo do Windows, e o Electron sai dessas máquinas (02/10/2026)

02/10/2026. A tela remota do navegador no celular era a última função que, no Windows, só o
Electron atendia: o `navsock.py` falava CDP direto com a porta 9223 que o `shell/main.cjs` abre, e o
nativo não tem porta de depuração. O pedido do usuário foi tirar o Electron uma plataforma por vez,
um PR para cada, começando pelo Windows.

- **O nativo repassa, não abre porta.** O servidor local dele (`browser/server.rs`, o mesmo do
  `/cmd`, porta efêmera e token no `_srv.json`) ganhou `GET /cdp?chave=<servidor::sessão>`, um
  WebSocket com o mesmo Bearer. Ali passa só a lista fechada que o `navsock` usa
  (`browser/relay.rs`, `RELAYED`): screencast, ack, print e `Input.*`. `Runtime.evaluate` fica de
  fora; a url inicial vem do sidecar, que o nativo regrava a cada navegação.
- **O formato é o do CDP**, então o `_Cdp` do backend só troca URL e cabeçalho (`_conexao`). Sidecar
  com `targetId` é o Electron; com `pid` é o nativo, e só se o `_srv.json` for do mesmo `pid`.
- **Um espectador por navegador; o último assume.** O WebView2 tem uma sessão CDP só. O anterior
  recebe `Inspector.detached` com `reason: replaced_by_another_viewer`, que o celular mostra como
  "a tela remota foi aberta em outro aparelho".
- **Quadro sem entrega é confirmado pelo nativo.** A fila do espectador guarda 4 quadros; o mais
  velho sai quando chega outro (`force_send`), e o ack dele é mandado ali mesmo, senão o Chromium
  para o screencast esperando.
- **Os comandos do celular não passam pelo turno do controlador**, como no Electron, em que o
  celular era outra sessão CDP: um toque não espera o `wait` de 15 s do agente.
- **Remoção do Electron no Windows** pelo passo `2026-10-02-electron-removido-windows`
  (`scripts/remover-electron.ps1`). Ele nunca falha, porque passo que falha derruba o Atualizar
  inteiro: sem o nativo instalado não mexe em nada; com o Electron aberto tira só os atalhos e
  deixa o `shell\node_modules` (binário em uso não se apaga). Ficam o código de `shell/` (o
  `hangar-preview` importa `preview_fmt.cjs`, `folha.cjs` e `jev_objetivo.cjs`) e o
  `%APPDATA%\Electron` (o nativo importa servidores e aparência de lá, `src/electron.rs`).
- **O aviso "feche e abra o app" do Atualizar** só aparece com `shell/node_modules/electron`
  presente.
- **A tela remota aceita sessão sem terminal.** O `nav_ws` conferia só o tmux e recusava (403 no
  handshake, "Conexão caiu" na tela) a sessão sem terminal que tem navegador; agora usa a mesma
  checagem da rota que abre o navegador (`_session_exists`). Valia também para o Electron.
- **Tecla nomeada do celular leva código virtual** (`_TECLAS` no `navsock`, a tabela do `press`).
  Só com `key`, o evento chegava à página, mas o Chromium não editava: Backspace não apagava e
  Enter não enviava. Valia também para o Electron.
- **Fora do escopo:** Linux e macOS (o motor deles não fala CDP) e o app Expo, que não tem a tela
  remota. O Linux veio no PR seguinte, trocando o motor (entrada abaixo).

Medição (02/10/2026, Windows 11, app nativo compilado da branch com MSVC, backend na mesma
máquina, PWA em 390×844 pelo túnel SSH): quadros chegam com o navegador escondido (WebView2
estacionado fora da tela); com a página parada o screencast não emite e o `navsock` cai no print
em laço, como no Electron. Toque, texto acentuado (`insertText`), Backspace, rolagem por arrasto
(220 px), navegação com a barra acompanhando a url, troca de layout (390×844 no navegador do
nativo), troca de aparelho ("a tela remota foi aberta em outro aparelho") e `hangar-preview close`
("o navegador desta sessão fechou"), todos conferidos lendo o estado da página pelo
`hangar-preview eval`. O script de remoção foi conferido nos cenários sem nativo, Electron aberto,
Electron fechado (com arquivo somente-leitura dentro do `node_modules`) e segunda execução.

## No Linux, o navegador do app nativo é um Chromium sem janela, e o Electron sai (02/10/2026)

02/10/2026. Segundo PR da retirada do Electron, depois do Windows. No Linux o nativo usava a WPE
WebKit (um navegador por processo, sem CDP) e não atendia o `hangar-preview` nem a tela remota;
como o instalador já não instalava o Electron, instalação nova no Linux estava sem os dois.

- **Por que Chromium e não a WPE.** Para o agente, o que importa é o motor em que os sites são
  feitos e testados, a árvore de acessibilidade real (`Accessibility.getFullAXTree`, que dá as
  refs `@eN`), entrada nativa (`Input.*`) e screencast. A WPE não fala CDP: seria um tradutor
  inteiro, com o `snapshot` montado em JavaScript e sem tema nem toque. O inspetor remoto do
  WebKit e o WebDriver do WPE não têm `Input`, screencast nem a árvore. O CEF dentro do processo
  daria o painel mais fluido, a um pacote de ~250 MB e um build com processos auxiliares. Com o
  Chromium, `control.rs`, `relay.rs`, `server.rs` e o `navsock.py` servem aos dois sistemas sem
  mudar.
- **Um Chromium por app, um alvo por sessão.** Nasce no primeiro navegador aberto e termina quando
  o último fecha (e com o app: `PR_SET_PDEATHSIG`). O perfil é persistente
  (`<config do app>/chromium`), compartilhado entre as sessões como no Electron.
- **CDP por pipe, sem porta** (`--remote-debugging-pipe`, fd 3 e 4, mensagens terminadas em NUL),
  sessões multiplexadas por `sessionId`. Respostas resolvem direto da thread de leitura; eventos
  vão à thread da interface, como no WebView2. `browser::cdp::Cdp` é o mesmo nome nos dois
  sistemas.
- **O painel pinta o screencast.** Cada quadro (PNG) é decodificado na thread de leitura e
  gravado numa textura do device wgpu da GPUI, a mesma a cada quadro (`paint_surface`); o ack sai
  dali mesmo. Painel escondido para o screencast; o controlador mantém a página em 1280×800 para
  o `shot`. O quadro é esticado até o painel: no resize, o do tamanho antigo cobre tudo até chegar
  o do novo, em vez de deixar sobra vazia.
- **PNG, não JPEG (05/10/2026).** O JPEG q85 borrava o texto. Medido com Chrome 154 sem janela,
  página de 890×764 com animação e texto: JPEG 59,6 quadros/s e 65 KB por quadro, PNG 59,8
  quadros/s e 84 KB. A tela remota do celular tem screencast próprio e não mudou.
- **Duas sessões por alvo.** A do painel serve o controlador, o screencast do painel e a entrada.
  O espectador da tela remota ganha uma sessão própria no primeiro `Watch`, com screencast do
  tamanho do aparelho, e ela cai no `Unwatch`.
- **Escala inteira, arredondada para cima** (`--force-device-scale-factor`). Com escala
  fracionária o Chromium arredonda o viewport (`layout 800 600` dava 801×600); com inteira o
  tamanho sai exato e o quadro sai no máximo do tamanho físico do painel.
- **Chrome completo e `chrome-headless-shell`.** Ordem de busca: `HANGAR_CHROMIUM`, o shell baixado
  em `~/.hangar/native/chromium`, e Chrome/Chromium do sistema. O do snap é recusado (não lê o
  perfil em pasta oculta da home). No Chrome completo cada alvo pede janela própria
  (`newWindow`), e a barra que ele desconta da altura (`outerHeight - innerHeight`) é medida no
  nascimento.
- **Página do agente se comporta como focada** (`Emulation.setFocusEmulationEnabled`), diálogo de
  JavaScript é aceito sozinho (sem resposta a página trava), `target=_blank` e `window.open`
  abrem na mesma página (sem abas), download é negado, e a regra de endereço do painel
  (`model::allowed_request`) vale por `Fetch` só para documentos.
- **Limitação conhecida:** a lista aberta de um `<select>` não aparece nos quadros (o Chromium sem
  janela não pinta o popup). O agente escolhe opção por `click`/`fill` na ref ou pelo Jev; no
  painel, as setas do teclado trocam a opção.
- **Instalação.** `scripts/install-chromium.sh` (chamado pelo `install-native.sh` e pelo passo
  `2026-10-02-chromium-navegador-nativo-linux`) usa o Chrome/Chromium da máquina ou baixa o
  `chrome-headless-shell` estável do Chrome for Testing (~100 MB de download, ~260 MB em disco).
  Nunca falha. A WPE deixa de ser necessária.
- **Remoção do Electron no Linux** pelo passo `2026-10-02-electron-removido-linux`
  (`scripts/remover-electron.sh`), com as mesmas regras do Windows e duas guardas a mais: só
  remove com um Chromium disponível e com o app nativo instalado já no motor novo (o binário
  contém `--remote-debugging-pipe`). Electron aberto é reconhecido pelo executável em `/proc`,
  não pela linha de comando, que também aparece em shells. Sai o lançador `hangar.desktop` só se
  ele abre o Electron deste checkout; ficam o código de `shell/` e o `~/.config/Electron`.
- **macOS continua no Electron.**

Medição (02/10/2026, Linux, monitor com escala 1,2, app nativo compilado da branch, Chrome 154 do
sistema e `chrome-headless-shell` 154 baixado): o Chromium responde em ~0,13 s; screencast a
60 quadros/s com animação e dois screencasts simultâneos no mesmo alvo em tamanhos diferentes;
`snapshot`, `fill` com acento, `click`, `type`, `press`, `hover`, `eval`, `console`, `tema`,
`layout` (390×844, 800×600 e 1280×800 exatos), `wait --idle`, `text`, `network` e `shot` pelo
`hangar-preview`; o painel mostra a página nítida. Tela remota por um cliente no papel do
celular: quadros, url, layout celular/desktop, toque e rolagem, troca de aparelho e `close`
("o navegador desta sessão fechou"). Fechar o último navegador e fechar o app encerram o
Chromium. O script de remoção foi conferido sem nativo, com nativo antigo, sem Chromium, com o
Electron aberto, fechado e numa segunda execução.

## Bandeja do nativo: fechar esconde a janela (04/10/2026)

A opção "Manter na bandeja ao fechar" (Configurações → Geral, desligada por padrão) põe um ícone
na bandeja e faz o pedido de fechar esconder a janela em vez de encerrar o app.

- **A janela é escondida, nunca destruída.** A tela (`app::Hangar`) guarda dezenas de assinaturas
  presas à janela e não sobrevive a fechar e reabrir. O pedido de fechar é interceptado com
  `on_window_should_close`; o app esconde ali mesmo e responde `false`. O backend Wayland chama
  esse aviso com os callbacks da janela emprestados, e um `cx.defer` não sai desse empréstimo (o
  efeito adiado roda antes de o aviso voltar): por isso `set_hidden` não toca nos callbacks.
- **`Window::set_hidden` é ajuste nosso no GPUI vendorizado** (`vendor/PATCHES.md`). Windows:
  `SW_HIDE`/`SW_SHOW`. X11: `UnmapWindow`/`MapWindow`. Wayland: esconder destrói o toplevel e o
  `xdg_surface`, tira o buffer da `wl_surface` e cria os dois de novo nela, sem commit; mostrar
  reaplica título, `app_id`, tamanhos e decoração e faz o commit inicial. A superfície e o
  renderer continuam os mesmos. Vale só para a janela principal (sem pai e sem diálogo), e o
  estado de maximizada ou tela cheia não é reaplicado: quem decide o tamanho na volta é o
  compositor.
- **Só desmapear com buffer nulo não serve.** Medido com `WAYLAND_DEBUG=1` no Hyprland 0.56.2: o
  buffer nulo desmapeia a janela, mas o commit sem buffer que viria depois não recebe
  `xdg_surface.configure`, e a janela não volta. Desenhar sem esperar o configure funcionaria
  ali e seria erro de protocolo num compositor estrito; um `xdg_surface` novo recebe o configure
  inicial em qualquer compositor.
- **Fechar só esconde com o ícone de pé e uma bandeja presente** (`hides_on_close`). Sem
  `StatusNotifierWatcher`, ou se o ícone não pôde ser criado, fechar encerra como antes, e a linha
  da opção diz o motivo. Se a bandeja some com a janela escondida, a janela volta.
- **Linux: `ksni` 0.3.6 sem a feature padrão `tokio`.** Ela ligaria `zbus/tokio` para todos os
  usuários do `zbus` (`notify-rust`, `ashpd`, `accesskit`), e o `notify-rust` é chamado fora do
  runtime. Entram as features `async-io` e `blocking`. O ícone vai como pixmap, que não depende
  do tema de ícones instalado, e `assume_sni_available(true)` deixa o serviço esperando a barra
  que sobe depois do app.
- **No Linux, "bandeja presente" é haver um HOSPEDEIRO, não só o serviço.** O serviço
  (`org.kde.StatusNotifierWatcher`) pode sobreviver à barra: com o `kded6` no ar, ele assume o
  nome quando a barra cai e responde `IsStatusNotifierHostRegistered = true` mesmo sem barra
  nenhuma. O app acompanha os nomes que as barras registram no barramento
  (`org.kde.StatusNotifierHost-*`, `org.freedesktop.StatusNotifierHost-*`); com o `kded` de dono
  do serviço só esses nomes valem, e com outro dono vale a resposta do próprio serviço.
- **O registro do ícone é repetido quando o serviço o recusa.** Ao voltar, a barra derruba e
  reinicia o `kded6`, e o registro chega antes de ele atender (`No such object path`). O `ksni`
  não tenta de novo; o app repete em 1, 2, 4, 8 e 16 s, pelo nome que o `ksni` deu ao ícone.
- **Windows: `Shell_NotifyIconW` numa thread própria**, com janela oculta e laço de mensagens
  dela. Não é janela "só de mensagens": essas não recebem o `TaskbarCreated`, usado para pôr o
  ícone de volta quando o Explorer reinicia.

- **Escondida, a janela Wayland ignora os eventos do toplevel e do `xdg_surface`**: o que chega
  nesse intervalo é resto dos objetos trocados, e confirmar um configure antigo no objeto novo
  seria erro de protocolo. O estado de apresentação volta a "sem quadro", para uma falha no
  primeiro desenho repetir por timer.
- **Dois pedidos do ícone em menos de meio segundo contam como um.** O duplo clique chega como
  dois cliques, e sem isso a janela aparecia e sumia.
- **"Sair" solta o ícone antes de encerrar**, e no Windows soltar espera a janela oculta morrer
  (`SendMessageW`): encerrar com o ícone de pé deixa um ícone morto na bandeja.
- **O clique só esconde a janela que está na tela.** Minimizada conta como fora da tela
  (`Window::is_visible`), e o clique a traz de volta.
- **No Windows o ícone tem estado.** O `TaskbarCreated` também chega com o ícone ainda lá (mudança
  de escala): acrescentar falha e atualizar confirma que ele existe. Se o Explorer voltou e ainda
  não aceita ícone, o app tenta de novo a cada 2 s e, enquanto isso, fechar não esconde.

Medição (04/10/2026, Hyprland 0.56.2 com a bandeja do Quickshell 0.2.1, build de
desenvolvimento): três ciclos de esconder e mostrar no Wayland e três no X11 (XWayland), com a
janela redesenhada a cada volta e o processo vivo; fechar pelo compositor esconde com o ícone de
pé; clique no ícone mostra e esconde, e dois pedidos seguidos valem por um; segunda execução e
link `hangar://` mostram a janela escondida, o link com o diálogo de convite preenchido; conversa
em andamento aberta volta atual depois de escondida; troca de idioma muda os textos do menu;
desligar a opção remove o ícone e fechar encerra; "Sair" encerra; numa sessão D-Bus sem bandeja a
linha avisa e fechar encerra.

Medição no Windows (04/10/2026, Windows 11 build 26200 numa VM, build de desenvolvimento, com os
cliques do ícone entregues como a mensagem que o Shell manda): fechar (`WM_CLOSE`) esconde a
janela com o processo vivo e o aviso único aparece; clique esquerdo mostra e esconde; a segunda
execução sai e a janela aparece; depois de reiniciar o Explorer o ícone continua registrado e o
clique volta a mostrar a janela; o clique direito abre o menu com "Abrir Hangar" e "Sair", e
"Sair" encerra o processo e a janela oculta; com a opção desligada não há ícone e fechar
encerra; com a janela minimizada o clique a restaura; um `TaskbarCreated` repetido não derruba
o ícone. Navegador embutido com a janela na bandeja (sessão fora da tela): `eval`, `snapshot`,
`press`, `shot` de uma página repintada depois de escondida e `click` num link que navegou
responderam igual a com a janela à mostra.

Barra caindo com a janela escondida (04/10/2026, Quickshell 0.2.1 como hospedeiro e `kded6` como
serviço): ao derrubar a barra, duas janelas escondidas voltaram sozinhas e fechar sem barra
encerrou; com a barra de volta o ícone foi registrado de novo, fechar escondeu e o clique no
ícone mostrou a janela. Antes do conserto a janela ficava escondida sem ícone em lugar nenhum, e
depois da volta da barra fechar encerrava o app.

Não conferido no uso real: o serviço da bandeja sumindo de vez, sem outro assumir (Linux); no Windows, o
duplo clique físico, o Explorer que demora a aceitar o ícone e o navegador embutido com o
painel dele aberto na tela na hora de esconder; compositores Wayland além do Hyprland.
