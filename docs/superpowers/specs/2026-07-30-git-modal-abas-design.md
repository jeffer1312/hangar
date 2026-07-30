# Git no desktop: modal com abas, layout empilhado do TortoiseGit — Design

**Data:** 2026-07-30
**Status:** aprovado no brainstorming, pendente de plano de implementação

## Problema

O painel git de hoje (`GitPanel.svelte`) é uma grade de três colunas — `210px | 1fr | 380px` —
dentro de uma folha docada. Nessas colunas convivem **seis responsabilidades ao mesmo tempo**: lista
de branches, arquivos alterados, caixa de commit, busca no log, log com grafo, e a zona de detalhe
(que fica vazia dizendo "selecione um commit" enquanto nada está selecionado).

Palavras do usuário ao ver a tela pronta: *"isso aqui está péssimo, difícil de entender, não era pra
ser cada tela com uma responsabilidade?"*.

Não é problema de largura — o painel ocupa a janela quase toda. É problema de **divisão**.

Dois agravantes registrados:

1. **O drift entre as duas views.** `GitSheet.svelte` (mobile, push-view por enum `GitView`) e
   `GitPanel.svelte` (desktop, 3 zonas por seleção) duplicam o roteamento. Na entrega do log hub
   isso já mordeu: o campo de busca foi parar na `GitToolbar`, que no mobile **só existe na view
   `list`** — ou seja, invisível exatamente na view onde se busca. Só não foi ao ar porque o review
   pegou.
2. **É a segunda vez que o usuário pede modal.** Em 2026-07-27 ele pediu o mesmo pra seção
   "Avançado" do `EnginesSheet`. Ver a memória `desktop-modal-nao-painel-docado`.

## O que o TortoiseGit realmente faz

Pesquisado em <https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html> (2026-07-30), depois
que o usuário apontou que a análise de layout não tinha sido feita — a leitura anterior extraiu
apenas o **inventário de ações**, não a forma.

O Log Dialog dele é:

- **Três painéis empilhados** com divisórias arrastáveis: lista de revisões em cima, mensagem
  completa do commit selecionado no meio, arquivos alterados embaixo.
- **Filtros no topo**: seletor de branch/revisão, intervalo de datas, caixa de busca.
- **Colunas da lista**: ícones de ação (modificado/adicionado/removido/renomeado), grafo, mensagem
  com decoração de branch/tag, autor, data.
- **Quase nenhum botão.** A documentação é explícita: *"a maioria das funcionalidades avançadas fica
  acessível apenas pelo menu de contexto ao clicar com botão direito"*. O único botão citado no
  diálogo é o *Statistics*.

**É daí que vem o "enxuto"**: o poder mora no botão direito, não numa barra de ferramentas. Copiar o
inventário de ações sem copiar essa decisão foi o erro da primeira entrega — as ações viraram
botões e barras que competem com o conteúdo.

## Desenho

### Forma

Um **modal único com abas**. Desktop: `ModalDialog.svelte` largo. Mobile: o mesmo conteúdo dentro do
`BottomSheet`, com a fileira de abas rolável.

```
┌─ git · claude-cockpit ───────────────────────────── ⋯  ✕ ┐
│ Mudanças 88 │ Histórico │ Branches 6                     │
├──────────────────────────────────────────────────────────┤
│ main ▾              [🔍 buscar na mensagem            ]  │
├──────────────────────────────────────────────────────────┤
│ │ ● 1b1942e docs(plan): marca os 28…   jeff  12:14       │
│ │ ● 8195de5 fix(git): 10 achados do…   jeff  11:30       │
│ │ ● cd869cd docs: menu de contexto…    jeff  10:57       │
│┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ arrastável ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈│
│ docs(plan): marca os 28 steps do log hub como feitos      │
│┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈│
│ M docs/superpowers/plans/2026-07-29-git-log-hub.md        │
└──────────────────────────────────────────────────────────┘
```

O **cabeçalho carrega o estado** que hoje está espalhado pelas colunas: nome do repo, branch atual,
contagem de alterados, ahead/behind. Assim nenhuma aba gasta espaço repetindo isso.

### Abas

| Aba | Quando aparece | Conteúdo |
|---|---|---|
| **Mudanças** `N` | sempre | working tree |
| **Histórico** | sempre | log |
| **Branches** `N` | sempre | locais e remotas |
| **⚠ Conflito** | só com sequenciador em andamento | qual operação travou, arquivos em conflito, e o abort |

A aba de conflito existir **condicionalmente** é o que torna o abort descobrível: ela nasce
exatamente quando um revert/cherry-pick conflita, e some quando resolve. O backend já expõe o estado
(`sequencer_state` em `git_ops.py`, servido no `GET /git/files`), então é derivado do repo e
sobrevive a fechar e reabrir o modal.

### Layout por aba (desktop)

**Histórico** — o empilhado do Tortoise, com divisórias arrastáveis:
1. filtros (branch + busca)
2. lista de commits (grafo, hash, mensagem, autor, data)
3. mensagem completa do commit selecionado
4. arquivos alterados do commit selecionado

Clicar num arquivo abre o diff **por cima do modal**, ocupando a janela; fecha e volta pro
empilhado. O diff é o único conteúdo que merece a tela inteira.

**Mudanças** — o mesmo idioma, que é também o layout do commit dialog do Tortoise:
1. lista de arquivos alterados com checkbox
2. diff do arquivo selecionado
3. mensagem do commit + opções (amend, branch nova) + confirmar

**Branches** — lista de locais e remotas, com a atual destacada. Criar branch/tag fica aqui e no
menu de contexto do commit.

### Layout por aba (mobile)

Empilhar três painéis não cabe num celular. No mobile cada aba é **drill-down**: nível 1 é a lista,
tocar empurra o nível 2, voltar retorna. A pilha é por aba — trocar de aba não perde o lugar da
outra.

Esta é a **única** diferença deliberada entre as duas views. Tudo o mais — quais abas existem, o que
cada uma contém, o que o menu de contexto oferece — é idêntico.

### Ações: menu de contexto, não barra

As ações de repositório (`status`, `fetch`, `pull`, `push`, `stash`, `stash-pop`) **saem da barra** e
viram menu de contexto, seguindo o Tortoise. Dois gatilhos, porque celular não tem botão direito:

- **Desktop:** botão direito no chip do repo (no `Composer`) e no cabeçalho do modal.
- **Mobile:** toque longo nos mesmos alvos — padrão que o app já usa pra renomear sessão na sidebar.
- **Ambos:** o `⋯` no cabeçalho do modal, como porta visível pra quem não descobriria o gesto.

O menu de contexto **por commit** (`CommitMenu.svelte`) já existe e não muda: diff completo, comparar
com a working tree, copiar hash/mensagem/detalhes, branches que contêm, criar branch/tag, cherry-pick,
revert, reset. Ele passa a ser aberto também por botão direito / toque longo na linha do commit, além
do `⋯` que já tem.

### Arquitetura

As telas viram **componentes puros**: recebem o `git` (store) e callbacks, e não sabem se estão num
modal ou numa folha. Um roteador único (`GitTabs.svelte`) guarda a aba ativa e a pilha de navegação
de cada aba.

```
GitTabs.svelte            ← aba ativa + pilha por aba (a ÚNICA lógica de roteamento)
├── GitChangesTab.svelte
├── GitHistoryTab.svelte
├── GitBranchesTab.svelte
└── GitConflictTab.svelte

GitModal.svelte   → ModalDialog  + GitTabs     (desktop)
GitSheet.svelte   → BottomSheet  + GitTabs     (mobile)
```

`GitPanel.svelte` deixa de existir; `GitSheet.svelte` perde toda a lógica e vira invólucro. Isso
elimina o roteamento duplicado que produziu o bug da busca invisível.

Os componentes de apresentação existentes (`CommitList`, `CommitDetail`, `CommitMenu`, `DiffView`,
`BranchList`, `ChangedFiles`, `CommitBox`, `LogSearch`) são reaproveitados — mudam de dono, não de
conteúdo.

### O que NÃO muda

- **Backend inteiro.** As 12 rotas git, o `git_ops.py` e os 1197 testes ficam como estão.
- **`gitStore.svelte.ts`.** O estado e as ações continuam iguais; só ganha o que a navegação por abas
  exigir.
- **O `CommitMenu`.** Conteúdo idêntico; ganha um gatilho a mais.

## Critério de sucesso

1. Nenhuma tela mostra mais de uma responsabilidade. O teste: dá pra dizer numa frase o que a tela
   faz.
2. A mesma navegação nas duas views — a diferença é só empilhado (desktop) vs drill-down (mobile).
3. Zero lógica de roteamento duplicada entre desktop e mobile.
4. Buscar, abortar e o menu por commit funcionam nas duas views, verificado com o navegador aberto.
5. Nenhuma regressão: os 1197 testes do backend seguem passando e o `npm run check` fica em 0 erros.

## Não-objetivos

- Redesenhar o `CommitMenu` (acabou de ser entregue e revisado).
- Mexer no backend.
- Trazer do Tortoise: coluna de datas com filtro por intervalo, estatísticas, bisect, rebase
  interativo, format-patch.
- Unificar `Sidebar`/`SessionList` — é outro drift, registrado no polish-backlog, fora deste escopo.

## Decisões registradas

- **Abas, não portas.** A primeira proposta foi um painel de status com quatro portas; o usuário
  trocou por abas sempre à vista.
- **Empilhado no desktop, drill-down no mobile.** O usuário primeiro escolheu drill-down nos dois;
  ao ver que o layout do Tortoise é empilhado, trocou pro empilhado no desktop. Empilhado no celular
  não cabe.
- **Ações no menu de contexto.** Vem da observação do usuário de que o Tortoise é enxuto *porque* o
  poder está no botão direito. A barra de rodapé com `status/fetch/pull/push/stash` que chegou a ser
  proposta foi descartada por isso.
- **A aba de conflito é condicional.** Nasce do estado real do repo, não da memória da sessão — o
  bug que o review final pegou na entrega anterior.
