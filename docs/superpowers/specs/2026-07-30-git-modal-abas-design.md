# Git no desktop: modal com abas, layout empilhado do TortoiseGit — Design

**Data:** 2026-07-30
**Status:** aprovado no brainstorming, revisado adversarialmente, pendente de plano de implementação

## Problema

O painel git de hoje (`GitPanel.svelte`) é uma grade de três colunas — `210px | 1fr | 380px` —
dentro de uma folha docada. Nessas colunas convivem **seis responsabilidades ao mesmo tempo**: lista
de branches, arquivos alterados, caixa de commit, busca no log, log com grafo, e a zona de detalhe
(que fica vazia dizendo "selecione um commit" enquanto nada está selecionado).

Palavras do usuário ao ver a tela pronta: *"isso aqui está péssimo, difícil de entender, não era pra
ser cada tela com uma responsabilidade?"*.

Não é problema de largura — o painel ocupa a janela quase toda. É problema de **divisão**.

Dois agravantes:

1. **O roteamento tem dois donos.** `GitSheet.svelte` (mobile, push-view por um type union
   `GitView`) e `GitPanel.svelte` (desktop, 3 zonas por seleção) decidem navegação cada um do seu
   jeito. Na entrega do log hub isso quase mordeu: o desenho original punha a busca na
   `GitToolbar`, que no mobile só é renderizada na view `list` (`GitSheet.svelte:263`) — ficaria
   invisível justamente na view onde se busca. Foi pego na escrita do plano e virou o componente
   `LogSearch`, hoje montado na view certa (`GitSheet.svelte:218`). O código atual está correto; o
   que fica é que **cada feature nova paga esse pedágio de atenção**.
2. **É a segunda vez que o usuário pede modal.** Em 2026-07-27 pediu o mesmo pra seção "Avançado"
   do `EnginesSheet`. Ver a memória `desktop-modal-nao-painel-docado`.

## O que o TortoiseGit realmente faz

Pesquisado em <https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html> (2026-07-30), depois
que o usuário apontou que a análise de layout não tinha sido feita — a leitura anterior extraiu
apenas o **inventário de ações**, não a forma.

- **Três painéis empilhados** com divisórias arrastáveis: lista de revisões em cima, mensagem
  completa do commit selecionado no meio, arquivos alterados embaixo.
- **Filtros no topo**: seletor de branch/revisão, intervalo de datas, caixa de busca.
- **Quase nenhum botão.** A documentação é explícita: *"a maioria das funcionalidades avançadas fica
  acessível apenas pelo menu de contexto ao clicar com botão direito"*. O único botão citado no
  diálogo é o *Statistics*.

**É daí que vem o "enxuto"**: o poder mora no botão direito, não numa barra de ferramentas. Copiar o
inventário de ações sem copiar essa decisão foi o erro da primeira entrega.

## Desenho

### Forma

Um **modal único com abas**. Desktop: `ModalDialog.svelte`. Mobile: o mesmo conteúdo dentro do
`BottomSheet`.

```
┌─ git · claude-cockpit ·  ⎇ main ──────────────────── ⋯  ✕ ┐
│  ⚠ cherry-pick em conflito — 2 arquivos    [abortar…]     │ ← só quando há sequenciador
├───────────────────────────────────────────────────────────┤
│ Mudanças 88 │ Histórico │ Branches 6                      │
├───────────────────────────────────────────────────────────┤
│ [🔍 buscar na mensagem                                 ]  │
├───────────────────────────────────────────────────────────┤
│ │ ● 1b1942e docs(plan): marca os 28…   jeff  12:14        │
│ │ ● 8195de5 fix(git): 10 achados do…   jeff  11:30        │
│ │ ● cd869cd docs: menu de contexto…    jeff  10:57        │
├───────────────────────────────────────────────────────────┤
│ docs(plan): marca os 28 steps do log hub como feitos       │
├───────────────────────────────────────────────────────────┤
│ M docs/superpowers/plans/2026-07-29-git-log-hub.md         │
├───────────────────────────────────────────────────────────┤
│ saída do git / erro                                        │ ← faixa única, dona da saída
└───────────────────────────────────────────────────────────┘
```

O **cabeçalho carrega** nome do repo e branch atual. **Não carrega ahead/behind**: nenhuma rota git
devolve isso (vive no `SessionInfo`, `models.py:64-65`, que não chega igual aos três pontos de
montagem). Fora de escopo.

### Conflito é faixa, não aba

A informação de um sequenciador travado — qual operação, quantos arquivos em conflito, e o abort —
cabe numa faixa. Vira **banner fixo no cabeçalho**, visível de qualquer aba, e não uma aba que nasce
e some. Motivo: uma aba condicional que aparece no meio da fileira **troca a aba debaixo do
usuário** se a seleção for por índice, e o conteúdo dela é do tamanho de um aviso. O estado vem do
repo (`sequencer` no `GET /git/files`), relido a cada `refresh()`.

### Abas

| Aba | Conteúdo | Vazio |
|---|---|---|
| **Mudanças** `N` | working tree | "nada alterado" |
| **Histórico** | log | "sem commits ainda" |
| **Branches** `N` | locais e remotas + filtro | "nenhuma branch" |

**A aba ativa é identificada por id, nunca por índice.** Mesma classe de bug do `plan_name` no
`_list_sig`.

**Todo estado vazio é explícito.** Hoje `ChangedFiles.svelte:26` não renderiza nada com repo limpo —
a aba Mudanças nasceria em branco, sem uma palavra. Só `CommitBox` e `BranchList` têm vazio hoje.

**Sessão que não é repositório git:** `list_branches` estoura `GitError(409)` com o stderr cru. O
modal mostra "esta pasta não é um repositório git" **antes de qualquer aba**, sem exibir stderr.

### Layout por aba (desktop)

**Histórico** — o empilhado do Tortoise:
1. busca (dentro da aba, não acima das abas — ela só vale aqui)
2. lista de commits
3. mensagem completa do commit selecionado
4. arquivos alterados do commit selecionado

**Mudanças** — o mesmo idioma, que é o commit dialog do Tortoise:
1. lista de arquivos com checkbox **e** descartar (uma lista só — ver "O que muda de verdade")
2. diff do arquivo selecionado
3. mensagem + opções (amend, branch nova) + confirmar

**Branches** — locais e remotas com a atual destacada, mais o **filtro por nome** (hoje só existe no
mobile, `GitSheet.svelte:264-274`; o desktop passa `filter=""`). Criar branch/tag fica aqui e no
menu do commit.

**Proporções fixas, não divisórias arrastáveis.** O app não tem nenhum splitter — os únicos
`col-resize` são bordas de painel com largura persistida (`Sidebar.svelte:153-166`,
`BottomSheet.svelte:224-230`). Quatro divisórias novas custariam persistência, `touch-action`,
suporte a teclado, e ainda brigariam com os `max-height: 52vh/68vh` que os painéis já trazem
(`CommitList.svelte:76-80`, `CommitDetail.svelte:48-53`, `CommitBox.svelte:108`). Versão desta
entrega: `flex` com proporção fixa e `overflow: auto` por painel; os `max-height` internos saem.
Divisória arrastável só se o usuário reclamar do tamanho.

**O diff ocupa a janela.** Clicar num arquivo abre o diff por cima do modal; fechar volta pro
empilhado. É o único conteúdo que merece a tela toda.

### Layout por aba (mobile)

Empilhar três painéis não cabe. No mobile cada aba é **drill-down**: nível 1 é a lista, tocar
empurra o nível 2, voltar retorna. A profundidade máxima é 3 (Histórico → commit → diff), 2
(Mudanças → diff) e 1 (Branches) — então é **um `$state` de nível por aba, não um roteador com
pilha**. Trocar de aba preserva o nível de cada uma.

Esta é a **única** diferença deliberada entre as views.

A fileira de abas precisa de `touch-action: pan-x` próprio: o `BottomSheet` declara
`touch-action: pan-y` (`:276`) e sem isso a fileira não rola no dedo.

### Ações: menu de contexto, não barra

As ações de repositório (`status`, `fetch`, `pull`, `push`, `stash`, `stash-pop`) saem da barra e
viram menu de contexto. Gatilhos:

- **Desktop:** botão direito no chip do repo (`Composer.svelte:745`) e no cabeçalho do modal.
- **Mobile:** toque longo nos mesmos dois alvos — **não na linha do commit**, onde o gesto concorre
  com selecionar/copiar o hash. É exatamente por isso que long-press saiu das bolhas de mensagem
  (`UserBubble.svelte:22`). Na linha do commit fica o `⋯` que já existe (`CommitList.svelte:66`).
- **Ambos:** o `⋯` no cabeçalho, porta visível pra quem não descobre o gesto.

O chip do repo já tem clique primário. O menu precisa da **guarda `longPressed`** que
`Sidebar.svelte:313` e `SessionCard.svelte` usam pra suprimir o clique seguinte — senão o toque
longo abre o menu *e* o modal ao soltar.

Já existe um menu de contexto de repo na linha da sidebar (`SessionContextMenu.svelte:182-184`, com
Git pull e Trocar branch). **Os dois convivem**: aquele é atalho da lista de sessões, este é do
modal aberto. Não unificar nesta entrega.

**A saída do git ganha dona única.** Hoje `git.error` é impresso pelo container **e** pelo
`CommitBox.svelte:102` **e** pelo `CommitMenu.svelte:163`, e a saída de `status`/`fetch`/`pull` cai
num `<pre>` no rodapé do painel. Tirando as ações pra um menu que fecha, essa saída ficaria sem
lugar — "falha aparece" viraria "falha some". Passa a existir **uma** faixa de saída/erro no modal,
e os componentes filhos param de imprimir por conta própria.

## O que muda de verdade (e o spec anterior negava)

### Backend — muda, pouco

- **`_LOG_FMT` ganha `%b`** (`git_ops.py:216`) e `GitCommit` ganha `body: string`
  (`frontend/src/lib/api.ts:861-873`). Sem isso o painel do meio não tem o que mostrar: hoje só
  existe `%s`, o assunto. É um campo a mais no formato e no parse.
- Nada mais. As 18 rotas git (`api.py:1908-2071`), o resto do `git_ops.py` e a suíte ficam.

**Fora de escopo, declarado:** escolher qual branch logar (`git_log` não aceita ref e `getGitLog`
não passa branch — o `⎇ main` do cabeçalho é rótulo da branch atual, não seletor) e paginação do log
(fixo em `n=50`, sem "carregar mais").

### Componentes — três mudam de conteúdo, não só de dono

- **`CommitDetail.svelte:22-45` se parte em dois**: hoje é mensagem + metadados + lista de arquivos
  num componente só; o empilhado quer mensagem num painel e arquivos noutro.
- **`ChangedFiles` e `CommitBox` se fundem numa lista só.** Os dois renderizam a lista de arquivos
  alterados hoje — um com ⟲ descartar (`ChangedFiles`), outro com checkbox (`CommitBox:69-78`). A
  aba Mudanças precisa de **uma** lista com as duas affordances, senão nasce com duas listas do
  mesmo.
- Os outros cinco (`CommitList`, `CommitMenu`, `DiffView`, `BranchList`, `LogSearch`) realmente só
  mudam de dono.

### z-index — o `CommitMenu` muda

O menu do commit usa 110/120 **porque** a `BottomSheet` é 100 (`CommitMenu.svelte:170-171`). O
backdrop do `ModalDialog` é **1000** (`ModalDialog.svelte:139`) — no desktop o menu renderizaria
atrás do modal. As camadas viram variáveis, não números soltos por componente.

Nota: `ModalDialog` já faz portal pro `<body>` e focus-trap com restore, então o `use:portal` manual
do `CommitMenu` pode ficar redundante lá dentro — **verificar na implementação, não assumir**.

### Tamanho do modal

`ModalDialog` não tem prop de largura: o padrão é `min(560px, 100%)`, `height: auto`
(`:166-186`). Um empilhado de três painéis precisa de altura explícita, senão não há o que dividir.
Passar `className` + regra `:global` com `width`/`height`, como o `PairChatModal.svelte:23` faz.

## Arquitetura

As telas viram **componentes puros**: recebem o `git` e callbacks, e não sabem se estão num modal ou
numa folha. Um componente guarda a aba ativa e o nível de cada aba.

```
GitTabs.svelte            ← aba ativa (por id) + nível por aba. Única lógica de navegação.
├── GitChangesTab.svelte
├── GitHistoryTab.svelte
└── GitBranchesTab.svelte

Git.svelte   → escolhe o invólucro e monta GitTabs
             → desktop: ModalDialog + className próprio
             → mobile:  BottomSheet
```

**Quem escolhe o invólucro precisa de dono nomeado.** Hoje a escolha desktop/mobile mora *dentro* do
`GitSheet` (`:54-61`, `:203`), que é montado em três lugares (`Chat.svelte:1268`,
`Sidebar.svelte:1304`, `SessionList.svelte:957`). Os três call sites passam a montar `<Git …>`, e
`GitSheet.svelte`/`GitPanel.svelte` deixam de existir.

**O `desktop` vem por prop**, não de um `matchMedia` próprio. `GitSheet.svelte:57` é a terceira
cópia da mesma media query (já em `App.svelte:162` e `BottomSheet.svelte:28`), e a primeira pintura
sai mobile e troca depois. Com dois invólucros diferentes, atravessar 820px passaria a **desmontar**
o modal e perder aba, nível e seleção.

Descartar `resizable={!isDesktop}` (`GitSheet.svelte:202`): é inerte — abaixo de 820px o handle é
`display:none` e acima o `wide` também o esconde. De passagem, corrigir o comentário desatualizado
do `PairSheet.svelte:174` que cita esse comportamento.

## Estado verificado do código (2026-07-30)

- **`GitPanel.svelte` tem um consumidor só:** importado em `GitSheet.svelte:12`, montado em `:204`
  sob `{#if isDesktop}`. Removê-lo quebra apenas o `GitSheet`.
- **Componente de abas reutilizável NÃO existe.** Três fileiras de `<button role="tab">` inline
  (`Costs.svelte:134-177`, `Archive.svelte:225`, `FolderScanner.svelte:130-131`), nenhuma com
  navegação. As abas são código novo.
- **Splitter vertical NÃO existe** — só redimensionamento de largura. Por isso o desenho usa
  proporções fixas.
- **Long-press existe, duplicado:** `Sidebar.svelte:302-313` e `SessionCard.svelte:100-137`
  (timer de 500ms, cancelado por movimento, flag que suprime o clique). Removido de propósito das
  bolhas de mensagem.
- **`oncontextmenu` existe num lugar:** `Sidebar.svelte:939`.
- **`ModalDialog` aceita:** `open`, `ariaLabel`, `onClose`, `initialFocus`, `closeOnBackdrop`,
  `className`, `layer`, `role`, `children`.
- **Bug pré-existente a não confundir com regressão:** um `Chat` dentro do `PairChatModal` (que é
  `ModalDialog`, z 1000) monta a folha de git em z 100 — ela já abre atrás do modal hoje.

## Critério de sucesso

1. Nenhuma tela mostra mais de uma responsabilidade. Teste: dá pra dizer numa frase o que ela faz.
2. A mesma navegação nas duas views — diferença só entre empilhado (desktop) e drill-down (mobile).
3. Zero lógica de roteamento duplicada.
4. Buscar, abortar e o menu por commit funcionam nas duas views, **verificado com o navegador
   aberto** — não só por typecheck.
5. A busca sobrevive à troca de aba e some ao fechar o modal.
6. Nenhuma regressão: a contagem de testes do backend não cai e o `npm run check` fica em 0 erros.
7. Repo limpo, repo sem commits e pasta que não é repo têm cada um seu estado explícito.

## Não-objetivos

- Redesenhar o `CommitMenu` (conteúdo; o z-index entra).
- Escolher branch pra logar; paginação do log além dos 50 commits.
- ahead/behind no cabeçalho.
- Divisórias arrastáveis.
- Unificar o menu de contexto do modal com o da linha da sidebar.
- Unificar `Sidebar`/`SessionList` — outro drift, registrado no polish-backlog.

## Decisões registradas

- **Abas, não portas.** A primeira proposta foi painel de status com quatro portas; o usuário trocou
  por abas sempre à vista.
- **Empilhado no desktop, drill-down no mobile.** O usuário primeiro escolheu drill-down nos dois; ao
  ver que o layout do Tortoise é empilhado, trocou pro empilhado no desktop.
- **Ações no menu de contexto.** Vem da observação do usuário de que o Tortoise é enxuto *porque* o
  poder está no botão direito.
- **Conflito virou faixa, não aba** — evita a aba que troca debaixo do usuário, e o conteúdo é do
  tamanho de um aviso.
- **Nível por aba, não pilha de navegação** — profundidade máxima 3; uma pilha seria maior que o
  problema.
- **Proporções fixas em vez de divisórias** — o app não tem splitter, e os `max-height` dos painéis
  brigariam com um.
