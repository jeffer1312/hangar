# Git como modal com abas (layout empilhado do TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Substituir o painel git de 3 colunas por um modal único com abas (Mudanças / Histórico / Branches), layout empilhado no desktop e drill-down no celular, com as ações de repositório saindo das barras pro menu de contexto.

**Architecture:** Um componente `Git.svelte` escolhe o invólucro (`ModalDialog` no desktop, `BottomSheet` no celular) e monta `GitTabs.svelte`, que é o ÚNICO dono da navegação. A navegação em si é um módulo puro testável (`lib/gitTabs.ts`). `GitPanel.svelte` e `GitSheet.svelte` deixam de existir. Backend muda só num ponto: `_LOG_FMT` ganha `%b` pro painel da mensagem completa.

**Tech Stack:** Svelte 5 (runes) + TypeScript, vitest pros módulos puros de `lib/`; Python 3.14 + FastAPI + pytest no backend.

**Spec:** [`../specs/2026-07-30-git-modal-abas-design.md`](../specs/2026-07-30-git-modal-abas-design.md) — ler antes de começar. Ele registra POR QUE cada decisão ficou assim, incluindo as que foram revertidas.

## Global Constraints

- **Duas views SEMPRE.** Toda mudança de UI entra no desktop E no celular, e a verificação manual testa as duas. A única diferença deliberada é empilhado (desktop) vs drill-down (mobile).
- **UI em pt-BR**; identificadores em inglês; comentários em pt-BR. Match de indentação/estilo do arquivo vizinho. **NUNCA rodar formatter** (sem prettier, sem `biome --write`).
- **Falha aparece, não some.** Erro do git chega ao usuário com o texto do git. Uma faixa única de saída/erro no modal — componentes filhos param de imprimir por conta própria.
- **Backend git:** argv list sempre, shell string nunca. Rotas FastAPI de git são `def` (threadpool), com `Depends(require_auth)`.
- **iOS:** não introduzir `backdrop-filter`/`transform`/`translateZ` em folha de vidro fora do `html[data-liquid]` — é a regra do retângulo preto do CLAUDE.md.
- **Gate de tipos:** `npm --prefix frontend run check` (o `build` NÃO checa tipos). Gate de testes do front: `npm --prefix frontend run test`. Backend: `cd backend && uv run pytest -q`.
- **Commits frequentes**, conventional commits, stage por path explícito — **nunca `git add -A`**.
- **Não criar nem trocar de branch.** O trabalho é na branch atual.

## O que já existe (não recriar)

`CommitList`, `CommitMenu`, `DiffView`, `BranchList`, `LogSearch`, `GitToolbar` — reaproveitados como estão. `gitStore.svelte.ts` com todo o estado e ações. As 18 rotas git. `ModalDialog.svelte` (já faz portal pro body e focus-trap com restore) e `BottomSheet.svelte`. `lib/focusCycle.ts` pra Tab trap. O long-press de `Sidebar.svelte:302-313` como referência de mecânica.

## Non-goals

Escolher qual branch logar; paginação além dos 50 commits; ahead/behind no cabeçalho; divisórias arrastáveis; unificar o menu de contexto do modal com o da linha da sidebar; unificar `Sidebar`/`SessionList`.

---

### Task 1: Backend — mensagem completa do commit (`%b`)

O painel do meio do empilhado mostra a mensagem COMPLETA. Hoje só existe o assunto (`%s`).

**Files:**
- Modify: `backend/app/git_ops.py` (`_LOG_FMT` e o parse em `git_log`)
- Modify: `frontend/src/lib/api.ts` (interface `GitCommit`)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_LOG_FMT`, `git_log` (existentes)
- Produces: cada dict de `git_log` ganha a chave `body: str` (corpo da mensagem sem o assunto, `''` quando não há corpo); `GitCommit` no front ganha `body: string`

- [ ] **Step 1: Escrever o teste que falha**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_git_log_traz_corpo_da_mensagem(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "corpo.txt").write_text("C\n")
    git_ops._run(d, "add", "corpo.txt")
    git_ops._run(d, "commit", "-q", "-m", "assunto curto", "-m", "primeira linha do corpo\nsegunda linha")
    c = git_ops.git_log(d)[0]
    assert c["subject"] == "assunto curto"
    assert "primeira linha do corpo" in c["body"] and "segunda linha" in c["body"]


def test_git_log_corpo_vazio_vira_string_vazia(tmp_path):
    d, _ = _repo_with_file(tmp_path)          # "add tracked" nao tem corpo
    assert git_ops.git_log(d)[0]["body"] == ""
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "corpo" -v`
Expected: FAIL com `KeyError: 'body'`

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, achar `_LOG_FMT` (perto da linha 210) e acrescentar `%b` como último
campo, ANTES do separador de registro. O formato usa `\x1f` entre campos e `\x1e` entre registros —
manter exatamente esse esquema e só somar um campo no fim.

O corpo pode conter `\n`, e é o ÚLTIMO campo, então o `split("\x1f")` continua funcionando: use
`maxsplit` ou pegue o resto. No parse de `git_log`, onde hoje há:

```python
        f = rec.split("\x1f")
        if len(f) < 8:
            continue
        full, short, parents, refs, author, ts, rel, subject = f[:8]
```

passa a ser:

```python
        f = rec.split("\x1f")
        if len(f) < 9:
            continue
        full, short, parents, refs, author, ts, rel, subject = f[:8]
        # O corpo (%b) e o ULTIMO campo de proposito: ele pode ter \n dentro, e como nao ha mais
        # nenhum \x1f depois dele, o split nao se perde.
        body = f[8].strip("\n")
```

e o dict ganha `"body": body,` junto de `"subject"`.

Em `frontend/src/lib/api.ts`, na interface `GitCommit`, acrescentar depois de `subject`:

```typescript
  body: string;       // corpo da mensagem (%b), sem o assunto; '' quando o commit nao tem corpo
```

- [ ] **Step 4: Rodar e ver passar** (suíte git inteira + self-check + gate do front)

Run: `cd backend && uv run pytest tests/test_git_ops.py -q && cd backend && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/tests/test_git_ops.py frontend/src/lib/api.ts
git commit -m "feat(git): git_log traz o corpo da mensagem do commit"
```

---

### Task 2: Camadas de z-index

O `CommitMenu` usa 110/120 **porque** a `BottomSheet` é 100. O backdrop do `ModalDialog` é **1000** —
dentro dele o menu renderizaria atrás. Números soltos por componente viram variáveis.

**Files:**
- Modify: `frontend/src/app.css` (bloco de tokens)
- Modify: `frontend/src/components/git/CommitMenu.svelte` (CSS)

**Interfaces:**
- Produces: `--z-overlay-back` e `--z-overlay-card` em `app.css`

- [ ] **Step 1: Declarar as camadas**

Em `frontend/src/app.css`, junto dos outros tokens, acrescentar:

```css
  /* Camadas de sobreposicao. O que existe hoje no projeto, do maior pro menor: 1100
     (ModalDialog.svelte:148), 1000 (ModalDialog.svelte:139, AttachmentsSheet.svelte:211,
     ImageBubble.svelte:110), 120/110 (CommitMenu), 100 (BottomSheet.svelte:259). Um overlay que
     precisa ficar acima dos DOIS involucros nao pode chutar olhando so pra um deles — por isso
     1200/1220, acima do maior que ja existe, e nao 1100 (que EMPATA com o ModalDialog). */
  --z-overlay-back: 1200;
  --z-overlay-card: 1220;
```

- [ ] **Step 2: Usar no CommitMenu**

Em `frontend/src/components/git/CommitMenu.svelte`, trocar os literais no CSS:

```css
  .cm-back { position: fixed; inset: 0; z-index: var(--z-overlay-back); background: color-mix(in srgb, var(--bg-base) 60%, transparent); }
  .cm { position: fixed; z-index: var(--z-overlay-card); /* …resto igual… */ }
```

e atualizar o comentário que explica o porquê, citando as duas referências (folha 100, modal 1000).

- [ ] **Step 3: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [ ] **Step 4: Commit**

```bash
git add frontend/src/app.css frontend/src/components/git/CommitMenu.svelte
git commit -m "refactor(git): camadas de sobreposicao viram tokens, acima da folha e do modal"
```

---

### Task 3: `lib/gitTabs.ts` — a navegação como módulo puro

A profundidade máxima é 3 (Histórico) / 2 (Mudanças) / 1 (Branches). É um nível por aba, não uma
pilha. Módulo puro porque é a única parte testável sem navegador — e é onde mora o bug do
`_list_sig` (identificar por índice em vez de id).

**Files:**
- Create: `frontend/src/lib/gitTabs.ts`
- Test: `frontend/src/lib/gitTabs.test.ts`

**Interfaces:**
- Produces:
  - `type GitTabId = 'changes' | 'history' | 'branches'`
  - `const GIT_TABS: readonly { id: GitTabId; label: string; maxLevel: number }[]`
  - `interface GitNav { tab: GitTabId; levels: Record<GitTabId, number> }`
  - `initialNav(): GitNav`
  - `selectTab(nav: GitNav, tab: GitTabId): GitNav`
  - `pushLevel(nav: GitNav): GitNav`
  - `popLevel(nav: GitNav): GitNav`
  - `currentLevel(nav: GitNav): number`

- [ ] **Step 1: Escrever os testes que falham**

Criar `frontend/src/lib/gitTabs.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';
import { initialNav, selectTab, pushLevel, popLevel, currentLevel, GIT_TABS } from './gitTabs';

describe('gitTabs', () => {
  it('começa em Mudanças, nível 0', () => {
    const n = initialNav();
    expect(n.tab).toBe('changes');
    expect(currentLevel(n)).toBe(0);
  });

  it('cada aba guarda o próprio nível', () => {
    let n = initialNav();
    n = pushLevel(n);                    // changes -> 1
    n = selectTab(n, 'history');
    expect(currentLevel(n)).toBe(0);     // history ainda no 0
    n = pushLevel(n); n = pushLevel(n);  // history -> 2
    n = selectTab(n, 'changes');
    expect(currentLevel(n)).toBe(1);     // changes lembra onde estava
    n = selectTab(n, 'history');
    expect(currentLevel(n)).toBe(2);
  });

  it('não passa do teto de cada aba', () => {
    // branches é lista e ponto: teto 1. history vai ate o diff: teto 3.
    let b = selectTab(initialNav(), 'branches');
    for (let i = 0; i < 5; i++) b = pushLevel(b);
    expect(currentLevel(b)).toBe(GIT_TABS.find((t) => t.id === 'branches')!.maxLevel);

    let h = selectTab(initialNav(), 'history');
    for (let i = 0; i < 9; i++) h = pushLevel(h);
    expect(currentLevel(h)).toBe(GIT_TABS.find((t) => t.id === 'history')!.maxLevel);
  });

  it('não desce abaixo de zero', () => {
    let n = popLevel(popLevel(initialNav()));
    expect(currentLevel(n)).toBe(0);
  });

  it('é identificada por id, nunca por índice', () => {
    // Uma aba que some/aparece nao pode trocar a aba ativa debaixo do usuario: a selecao guarda o
    // id, entao mudar a ORDEM ou o TAMANHO da lista de abas nao muda quem esta ativo.
    const n = selectTab(initialNav(), 'branches');
    expect(n.tab).toBe('branches');
    expect(typeof n.tab).toBe('string');
  });

  it('não muta a entrada', () => {
    const a = initialNav();
    const b = pushLevel(a);
    expect(currentLevel(a)).toBe(0);
    expect(b).not.toBe(a);
  });
});
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `npm --prefix frontend run test -- gitTabs`
Expected: FAIL — o módulo não existe

- [ ] **Step 3: Implementar**

Criar `frontend/src/lib/gitTabs.ts`:

```typescript
// Navegacao do modal de git: qual aba esta ativa e em que nivel cada uma parou.
//
// Nivel, nao pilha: a profundidade maxima e 3 (Historico -> commit -> diff), 2 (Mudancas -> diff) e
// 1 (Branches). Uma pilha de navegacao seria maior que o problema.
//
// A aba ativa e guardada por ID, nunca por indice: uma faixa/aba que aparece e some trocaria a aba
// debaixo do usuario se a selecao fosse posicional (mesma classe do plan_name no _list_sig).

export type GitTabId = 'changes' | 'history' | 'branches';

export const GIT_TABS = [
  { id: 'changes',  label: 'Mudanças',  maxLevel: 1 },   // lista -> diff do arquivo
  { id: 'history',  label: 'Histórico', maxLevel: 2 },   // lista -> commit -> diff
  { id: 'branches', label: 'Branches',  maxLevel: 0 },   // so a lista
] as const satisfies readonly { id: GitTabId; label: string; maxLevel: number }[];

export interface GitNav {
  tab: GitTabId;
  levels: Record<GitTabId, number>;
}

const maxOf = (tab: GitTabId): number => GIT_TABS.find((t) => t.id === tab)!.maxLevel;

export function initialNav(): GitNav {
  return { tab: 'changes', levels: { changes: 0, history: 0, branches: 0 } };
}

export function selectTab(nav: GitNav, tab: GitTabId): GitNav {
  return { tab, levels: { ...nav.levels } };
}

export function currentLevel(nav: GitNav): number {
  return nav.levels[nav.tab];
}

export function pushLevel(nav: GitNav): GitNav {
  const teto = maxOf(nav.tab);
  return { tab: nav.tab, levels: { ...nav.levels, [nav.tab]: Math.min(nav.levels[nav.tab] + 1, teto) } };
}

export function popLevel(nav: GitNav): GitNav {
  return { tab: nav.tab, levels: { ...nav.levels, [nav.tab]: Math.max(nav.levels[nav.tab] - 1, 0) } };
}
```

Atenção ao teto: os testes do Step 1 usam `maxLevel` do próprio `GIT_TABS`, então os números acima
(1 / 2 / 0) são a fonte da verdade — nível 0 é a lista.

- [ ] **Step 4: Rodar e ver passar**

Run: `npm --prefix frontend run test -- gitTabs && npm --prefix frontend run check`
Expected: 6 testes passando, 0 erros de tipo

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/gitTabs.ts frontend/src/lib/gitTabs.test.ts
git commit -m "feat(git): modulo puro da navegacao por abas do modal"
```

---

### Task 4: A casca — `Git.svelte` + `GitTabs.svelte`, e a morte do `GitPanel`/`GitSheet`

Esta é a task estrutural. No fim dela o modal já abre nas duas views com as três abas montando os
componentes que já existem; o refino de cada aba vem depois.

**Files:**
- Create: `frontend/src/components/git/GitTabs.svelte`
- Create: `frontend/src/components/Git.svelte`
- Delete: `frontend/src/components/GitPanel.svelte`
- Delete: `frontend/src/components/GitSheet.svelte`
- Modify: `frontend/src/screens/Chat.svelte` (montagem, linha ~1268)
- Modify: `frontend/src/components/Sidebar.svelte` (montagem, linha ~1304)
- Modify: `frontend/src/screens/SessionList.svelte` (montagem, linha ~957)

**Interfaces:**
- Consumes: `lib/gitTabs.ts` (Task 3), `GitStore`, `ModalDialog`, `BottomSheet`
- Produces:
  - `Git.svelte` props: `{ open: boolean; sessionName: string; desktop: boolean; onClose: () => void }`
  - `GitTabs.svelte` props: `{ git: GitStore; desktop: boolean }`

**`Git.svelte` recebe `sessionName`, não o store — e é ele quem CRIA o store.** Os três call sites
passam `sessionName` hoje (`Chat.svelte:1268`, `Sidebar.svelte:1304`, `SessionList.svelte:957`), e
quem instancia é o `GitSheet.svelte:19-31`, num `$effect` que **recria o store ao trocar de sessão**.
Essa responsabilidade migra inteira pro `Git.svelte` — se ela sumir, trocar de sessão com o modal
aberto mostra o git da sessão anterior.

- [ ] **Step 1: Ler os três pontos de montagem antes de mexer**

Run: `grep -n "GitSheet" frontend/src/screens/Chat.svelte frontend/src/components/Sidebar.svelte frontend/src/screens/SessionList.svelte`

Anotar, pra cada um: como o `open` é controlado, que store de git é passado, e o que o `onClose`
faz. O `Sidebar` tem um `closeGitSheet` que restaura o servidor ativo — **esse comportamento não
pode se perder**.

- [ ] **Step 2: `GitTabs.svelte`**

```svelte
<script lang="ts">
  import { GIT_TABS, initialNav, selectTab, type GitTabId, type GitNav } from '../../lib/gitTabs';
  import type { GitStore } from '../../lib/gitStore.svelte';
  import BranchList from './BranchList.svelte';
  import ChangedFiles from './ChangedFiles.svelte';
  import CommitList from './CommitList.svelte';
  import LogSearch from './LogSearch.svelte';

  interface Props { git: GitStore; desktop: boolean }
  let { git, desktop }: Props = $props();

  let nav = $state<GitNav>(initialNav());

  // Contagem no rotulo da aba: so quando ha o que contar (0 nao vira badge).
  const contagem = (id: GitTabId) =>
    id === 'changes' ? git.files.length
    : id === 'branches' ? git.branches.length
    : 0;
</script>

<div class="gt">
  <div class="gt-tabs" role="tablist" aria-label="Seções do git">
    {#each GIT_TABS as t (t.id)}
      <button class="gt-tab" class:sel={nav.tab === t.id} role="tab"
        aria-selected={nav.tab === t.id}
        onclick={() => (nav = selectTab(nav, t.id))}>
        {t.label}{#if contagem(t.id)}<span class="gt-count">{contagem(t.id)}</span>{/if}
      </button>
    {/each}
  </div>

  <div class="gt-body">
    {#if nav.tab === 'changes'}
      <ChangedFiles {git} onOpenDiff={() => {}} onCommit={() => {}} />
    {:else if nav.tab === 'history'}
      <LogSearch {git} />
      <CommitList commits={git.commits} wtCount={0} noGraph={!!git.logQuery}
        onSelect={() => {}} onMenu={() => {}} />
    {:else}
      <BranchList {git} filter="" />
    {/if}
  </div>
</div>

<style>
  .gt { display: flex; flex-direction: column; gap: var(--space-3); height: 100%; min-height: 0; }
  /* pan-x proprio: a BottomSheet declara touch-action: pan-y (BottomSheet.svelte:276) e sem isto a
     fileira de abas nao rola no dedo dentro dela. */
  .gt-tabs {
    display: flex; gap: var(--space-1); overflow-x: auto; touch-action: pan-x;
    border-bottom: 1px solid var(--border-subtle); flex-shrink: 0;
  }
  .gt-tab {
    display: flex; align-items: center; gap: var(--space-1); flex-shrink: 0;
    padding: var(--space-2) var(--space-3); border: 0; background: transparent;
    color: var(--text-muted); font-size: var(--text-sm); cursor: pointer;
    border-bottom: 2px solid transparent;
  }
  .gt-tab.sel { color: var(--text-primary); border-bottom-color: var(--accent); }
  .gt-count {
    padding: 0 6px; border-radius: 999px; background: var(--bg-elevated);
    font-size: var(--text-xs); font-family: var(--font-mono);
  }
  .gt-body { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: var(--space-2); }
</style>
```

Os callbacks vazios (`() => {}`) são **temporários desta task** — as Tasks 5 a 7 os preenchem. O
`nav` já existe pra elas usarem.

- [ ] **Step 3: `Git.svelte` — o invólucro**

```svelte
<script lang="ts">
  import ModalDialog from './ModalDialog.svelte';
  import BottomSheet from './BottomSheet.svelte';
  import GitTabs from './git/GitTabs.svelte';
  import { createGitStore } from '../lib/gitStore.svelte';

  // `desktop` vem por PROP, nao de um matchMedia proprio: o GitSheet antigo era a terceira copia da
  // mesma media query (App.svelte:158-167, BottomSheet.svelte:28), a primeira pintura saia mobile e
  // trocava depois — e com dois involucros diferentes isso passaria a DESMONTAR o modal ao
  // atravessar 820px, perdendo aba e nivel.
  interface Props { open: boolean; sessionName: string; desktop: boolean; onClose: () => void }
  let { open, sessionName, desktop, onClose }: Props = $props();

  // Dono do store (era do GitSheet.svelte:19-31). Recria ao TROCAR de sessao — sem isto, abrir o
  // modal numa sessao e trocar pra outra mostraria o git da anterior. Copiar a forma exata do
  // GitSheet antes de apaga-lo.
  let git = $state(createGitStore(sessionName));
  $effect(() => { git = createGitStore(sessionName); });
  $effect(() => { if (open) git.load(); });
</script>

{#if desktop}
  <!-- className + regra :global porque o ModalDialog nao tem prop de largura: o padrao e
       min(560px,100%) com height auto, e um empilhado de tres paineis precisa de altura explicita
       (mesmo recurso que o PairChatModal usa). -->
  <ModalDialog {open} {onClose} ariaLabel="Git" className="git-modal">
    <GitTabs {git} desktop={true} />
  </ModalDialog>
{:else}
  <!-- Sem `wide` nem `resizable`: eram do dock de desktop do GitSheet antigo (`wide={isDesktop}`,
       `resizable={!isDesktop}`), e o desktop agora é o ModalDialog. No celular a folha é a folha. -->
  <BottomSheet {open} {onClose} ariaLabel="Git">
    <GitTabs {git} desktop={false} />
  </BottomSheet>
{/if}

<style>
  /* Mesmo padrão do PairChatModal.svelte:42-47, inclusive o teto de altura e a tela cheia no
     celular — sem eles o modal estoura a viewport em janela baixa. */
  :global(.git-modal) {
    width: min(1100px, 100%); height: min(760px, 100%);
    max-height: calc(100dvh - var(--space-8)); overflow: hidden;
  }
  @media (max-width: 819px) {
    :global(.git-modal) { width: 100%; height: 100%; max-height: 100dvh; }
  }
</style>
```

`className` existe mesmo (`ModalDialog.svelte:6-28`), e a classe cai no elemento em `:123`
(`class="modal-dialog {className}"`), com `:175-177` documentando o truque de especificidade zero
que deixa o consumidor sobrepor. Ler o `PairChatModal.svelte:42-51` e seguir a forma dele.

- [ ] **Step 4: Trocar os três pontos de montagem**

Em cada um dos três arquivos, trocar `<GitSheet …>` por `<Git …>`, mantendo o `sessionName` que já
passam e acrescentando `desktop`. Levantado, não suposto:

- **`Chat.svelte:1268`** → `desktop={desktop}`. O `Chat` já tem a prop `desktop?: boolean`
  (`:54`, default `false` em `:68`); quem passa `true` é o `DesktopShell.svelte:218,235,253`, e o
  ramo mobile do `App.svelte:409-414` monta sem passar. Dentro do `Chat` o booleano é confiável.
- **`SessionList.svelte:957`** → `desktop={false}` fixo. A tela **não tem** prop de desktop
  (`:27-32` só traz `onNavigateToChat`, `onCompare`, `onLogout`) e é mobile-only por construção: o
  `App.svelte:398-403` só a renderiza no ramo que vem **depois** do `{:else if isDesktop}`. Não
  inventar prop nova.
- **`Sidebar.svelte:1304`** → `desktop={true}`. A sidebar é desktop-only (comentado em `:1324`).

**Preservar o `closeGitSheet`** de `Sidebar.svelte:451-454` (restaura o servidor ativo via
`selectServer`) e o gêmeo em `SessionList.svelte:390`, ligando-os no `onClose`.

**Preservar o `closeGitSheet` do `Sidebar`** (restaura o servidor ativo ao fechar) ligando-o no
`onClose`.

- [ ] **Step 5: Apagar os dois arquivos**

```bash
git rm frontend/src/components/GitPanel.svelte frontend/src/components/GitSheet.svelte
```

- [ ] **Step 6: Gate + varredura de referências órfãs**

Run: `npm --prefix frontend run check`
Expected: 0 erros

Run: `grep -rn "GitSheet\|GitPanel" frontend/src/`
Expected: nenhuma referência de código. Comentários que citem os nomes (ex.: `PairSheet.svelte:174`,
`CommitMenu.svelte`) devem ser corrigidos, não deixados mentindo.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/Git.svelte frontend/src/components/git/GitTabs.svelte frontend/src/screens/Chat.svelte frontend/src/components/Sidebar.svelte frontend/src/screens/SessionList.svelte
git commit -m "feat(git): modal unico com abas substitui o painel e a folha de git"
```

---

### Task 5: Aba Mudanças — uma lista só, com checkbox e descartar

Hoje `ChangedFiles` e `CommitBox` renderizam **cada um** a lista de arquivos alterados: um com ⟲
descartar, outro com checkbox. Numa aba só, isso viraria duas listas do mesmo.

**Files:**
- Create: `frontend/src/components/git/ChangesTab.svelte`
- Delete: `frontend/src/components/git/ChangedFiles.svelte`
- Modify: `frontend/src/components/git/CommitBox.svelte` (perde a própria lista)
- Modify: `frontend/src/components/git/GitTabs.svelte` (monta a aba nova)

**Interfaces:**
- Consumes: `GitStore`, `CommitBox`, `DiffView`, `lib/gitTabs`
- Produces: `ChangesTab.svelte` props `{ git: GitStore; desktop: boolean; level: number; onPush: () => void; onPop: () => void }`

- [ ] **Step 1: Ler os dois componentes que se fundem**

Run: `cat frontend/src/components/git/ChangedFiles.svelte && sed -n '60,110p' frontend/src/components/git/CommitBox.svelte`

Anotar as duas affordances por linha: o ⟲ com confirm em 2 passos (`confirmDiscard`) do
`ChangedFiles`, e o checkbox de seleção do `CommitBox`. **A lista fundida tem as duas.**

- [ ] **Step 2: `ChangesTab.svelte`**

Uma lista de arquivos onde cada linha tem: checkbox de seleção, código do status (`M`, `??`, …),
caminho (clicável → abre o diff), e o ⟲ descartar com confirm em 2 passos. Abaixo da lista, o
`CommitBox` **sem lista própria** (mensagem, recentes, amend, branch nova, botões).

Estados obrigatórios:

```svelte
  {#if !git.files.length}
    <p class="git-muted">nada alterado — a working tree está limpa</p>
  {:else}
    <!-- lista + CommitBox -->
  {/if}
```

O vazio é obrigatório porque hoje `ChangedFiles.svelte:26` (`{#if git.dirty && git.files.length}`)
não renderiza NADA com repo limpo — a aba nasceria em branco, sem uma palavra.

Desktop (`desktop === true`): lista em cima, diff do arquivo selecionado no meio, `CommitBox`
embaixo, em `flex` com proporção fixa e `overflow: auto` por painel.
Mobile: `level === 0` mostra lista + `CommitBox`; clicar num arquivo chama `onPush()` e `level === 1`
mostra o diff com voltar.

- [ ] **Step 3: `CommitBox` perde a lista**

Remover do `CommitBox.svelte` o bloco `.cb-files` (a lista com checkbox) e os botões `todos`/`nenhum`
— eles migram pro `ChangesTab`, que passa a ser dono da seleção. O `CommitBox` recebe os paths
selecionados por prop:

```typescript
  interface Props { git: GitStore; chosen: string[]; onDone?: () => void }
```

e usa `chosen` onde hoje usa o `chosen` derivado da própria seleção. O resto (mensagem, recentes,
amend, branch nova, `canCommit`, `doCommit`) fica **idêntico** — é código que acabou de ser
entregue e revisado.

- [ ] **Step 4: Montar no `GitTabs` e apagar o `ChangedFiles`**

```bash
git rm frontend/src/components/git/ChangedFiles.svelte
```

No `GitTabs.svelte`, o ramo `changes` passa a montar `<ChangesTab {git} {desktop} level={currentLevel(nav)} onPush={() => (nav = pushLevel(nav))} onPop={() => (nav = popLevel(nav))} />`.

- [ ] **Step 5: Gate**

Run: `npm --prefix frontend run check && grep -rn "ChangedFiles" frontend/src/`
Expected: 0 erros de tipo, nenhuma referência órfã

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/git/ChangesTab.svelte frontend/src/components/git/CommitBox.svelte frontend/src/components/git/GitTabs.svelte
git commit -m "feat(git): aba Mudancas com uma lista so (checkbox + descartar)"
```

---

### Task 6: Aba Histórico — empilhado, com o `CommitDetail` partido

`CommitDetail.svelte:22-45` é hoje mensagem + metadados + lista de arquivos num componente só. O
empilhado quer mensagem num painel e arquivos noutro.

**Files:**
- Create: `frontend/src/components/git/HistoryTab.svelte`
- Create: `frontend/src/components/git/CommitMessage.svelte`
- Create: `frontend/src/components/git/CommitFiles.svelte`
- Delete: `frontend/src/components/git/CommitDetail.svelte`
- Modify: `frontend/src/components/git/GitTabs.svelte`

**Interfaces:**
- Consumes: `CommitList`, `CommitMenu`, `LogSearch`, `DiffView`, `GitCommit.body` (Task 1)
- Produces:
  - `CommitMessage.svelte` props `{ commit: GitCommit }` — assunto, corpo (`commit.body`), autor, data
  - `CommitFiles.svelte` props `{ commit: GitCommit; sessionName: string; onOpenFile: (p: string) => void }`
  - `HistoryTab.svelte` props `{ git: GitStore; desktop: boolean; level: number; onPush: () => void; onPop: () => void }`

- [ ] **Step 1: Ler o `CommitDetail` antes de partir**

Run: `cat frontend/src/components/git/CommitDetail.svelte`

Ele busca os arquivos do commit (`getCommitFiles`) e tem `max-height` próprio (`:48-53`) que precisa
sair — quem controla altura agora é o empilhado.

- [ ] **Step 2: `CommitMessage.svelte`**

Assunto em destaque, corpo em `white-space: pre-wrap` (é `commit.body`, que a Task 1 trouxe), autor
e data. Sem `max-height`: o painel do empilhado é quem limita.

Vazio: commit sem corpo mostra só o assunto e os metadados — nada de espaço morto.

- [ ] **Step 3: `CommitFiles.svelte`**

A lista de arquivos do commit, com o mesmo fetch (`getCommitFiles`) e o mesmo `onOpenFile` que o
`CommitDetail` tinha. Mantém o botão `⋯ ações` (prop `onMenu?` OPCIONAL — outro plano reusa isto sem
menu).

- [ ] **Step 4: `HistoryTab.svelte`**

Desktop, empilhado com proporção fixa e `overflow: auto` por painel:
1. `<LogSearch {git} />` — **dentro da aba**, porque a busca só vale aqui
2. `<CommitList … />`
3. `<CommitMessage commit={selecionado} />`
4. `<CommitFiles commit={selecionado} … />`

Sem commit selecionado, os painéis 3 e 4 mostram "selecione um commit" — uma vez só, não dois.
Log vazio: "sem commits ainda".

Mobile: `level 0` = busca + lista; `level 1` = mensagem + arquivos do commit; `level 2` = diff.
`onPush`/`onPop` do `gitTabs`.

O diff (de arquivo ou do commit inteiro, vindo do `CommitMenu`) **ocupa a janela** nas duas views.

- [ ] **Step 5: Apagar o `CommitDetail` e montar no `GitTabs`**

```bash
git rm frontend/src/components/git/CommitDetail.svelte
```

- [ ] **Step 6: Gate**

Run: `npm --prefix frontend run check && grep -rn "CommitDetail" frontend/src/`
Expected: 0 erros, nenhuma referência órfã

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/git/HistoryTab.svelte frontend/src/components/git/CommitMessage.svelte frontend/src/components/git/CommitFiles.svelte frontend/src/components/git/GitTabs.svelte
git commit -m "feat(git): aba Historico empilhada, com mensagem e arquivos em paineis proprios"
```

---

### Task 7: Aba Branches (com filtro nas duas views) + faixa de conflito + faixa única de saída

Três coisas pequenas que fecham a casca.

**Files:**
- Create: `frontend/src/components/git/BranchesTab.svelte`
- Create: `frontend/src/components/git/GitStatusBar.svelte`
- Modify: `frontend/src/components/git/GitTabs.svelte`
- Modify: `frontend/src/components/git/CommitBox.svelte` (para de imprimir erro)
- Modify: `frontend/src/components/git/CommitMenu.svelte` (para de imprimir erro)

**Interfaces:**
- Produces:
  - `BranchesTab.svelte` props `{ git: GitStore }` — `BranchList` + campo de filtro
  - `GitStatusBar.svelte` props `{ git: GitStore }` — faixa de conflito + faixa de saída/erro

- [ ] **Step 1: `BranchesTab.svelte`**

`BranchList` mais o **campo de filtro por nome**. Hoje ele só existe no mobile
(`GitSheet.svelte:265-274`), e ainda por cima **condicionado** a
`{#if git.branches.length > 6 || git.remotes.length}` (`:264`) — o desktop passava `filter=""` e
nunca teve filtro. Na aba dedicada ele fica **incondicional**: a aba existe pra isso, e um campo que
aparece e some conforme a contagem de branches é mais confuso que um campo sempre lá.

`BranchList` exige `filter: string` (prop obrigatória, sem default — `BranchList.svelte:6-10`), então
a aba é quem guarda o estado do filtro e passa pra ele.

Vazio: "nenhuma branch" (não deve acontecer num repo com commits, mas repo sem commit nenhum chega
aqui).

- [ ] **Step 2: `GitStatusBar.svelte`**

Duas faixas, nesta ordem, acima das abas:

```svelte
{#if git.pendingAbort}
  <div class="gsb-conflito" role="status">
    <span>⚠ {git.pendingAbort === 'revert-abort' ? 'revert' : 'cherry-pick'} em conflito</span>
    {#if confirmar}
      <button class="git-mini danger" disabled={!!git.busy} onclick={() => git.abortOp()}>confirmar abort</button>
      <button class="git-mini" onclick={() => (confirmar = false)}>não</button>
    {:else}
      <button class="git-mini danger" onclick={() => (confirmar = true)}>abortar…</button>
    {/if}
  </div>
{/if}
{#if git.error}<p class="gsb-erro">{git.error}</p>{/if}
{#if git.output}<pre class="gsb-saida">{git.output}</pre>{/if}
```

A faixa de conflito é **fixa no cabeçalho, visível de qualquer aba** — não é uma aba que nasce e
some (isso trocaria a aba debaixo do usuário). O estado vem do repo (`sequencer` no `GET
/git/files`), então sobrevive a fechar e reabrir.

- [ ] **Step 3: Tirar os donos duplicados do erro**

Remover `{#if git.error}<p class="git-error">…` de `CommitBox.svelte` e de `CommitMenu.svelte`. A
partir daqui a `GitStatusBar` é a **única** que imprime `git.error` e `git.output`.

Exceção deliberada: o `CommitMenu` fica por cima do modal, então um erro impresso só na faixa
ficaria escondido atrás dele. Manter no `CommitMenu` **apenas** enquanto ele estiver aberto, e a
`GitStatusBar` esconder o erro nesse caso — mesmo padrão do `{#if git.error && !menuCommit}` que
existe hoje em `GitPanel.svelte:163` e `GitSheet.svelte:288`. Passar `menuAberto` por prop.

(O comentário do `CommitMenu.svelte:165-167` cita esses dois pontos como `GitSheet:206` /
`GitPanel:102` — já está desatualizado no repo. Corrigir de passagem, já que o componente muda
nesta task.)

- [ ] **Step 4: Montar no `GitTabs`**

`<GitStatusBar {git} menuAberto={!!menuCommit} />` acima da fileira de abas; ramo `branches` monta
`<BranchesTab {git} />`.

- [ ] **Step 5: Gate**

Run: `npm --prefix frontend run check && npm --prefix frontend run test`
Expected: 0 erros, testes passando

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/git/BranchesTab.svelte frontend/src/components/git/GitStatusBar.svelte frontend/src/components/git/GitTabs.svelte frontend/src/components/git/CommitBox.svelte frontend/src/components/git/CommitMenu.svelte
git commit -m "feat(git): aba Branches com filtro, faixa de conflito e dona unica da saida"
```

---

### Task 8: Ações de repositório saem da barra pro menu de contexto

**Files:**
- Create: `frontend/src/components/git/RepoMenu.svelte`
- Modify: `frontend/src/components/git/GitTabs.svelte` (botão `⋯` no cabeçalho)
- Modify: `frontend/src/components/Composer.svelte` (botão direito / toque longo no chip do repo)
- Delete: `frontend/src/components/git/GitToolbar.svelte`

**Interfaces:**
- Produces: `RepoMenu.svelte` props `{ git: GitStore; x?: number; y?: number; onClose: () => void }` — `status`, `fetch`, `pull`, `push`, `stash`, `stash-pop`

- [ ] **Step 1: `RepoMenu.svelte`**

Os seis itens, cada um chamando `git.runAction(<ação>)`. Mesmo vocabulário visual do
`SessionContextMenu.svelte` (ler antes) e as mesmas camadas da Task 2.

- [ ] **Step 2: Gatilhos**

- **Botão `⋯` no cabeçalho do `GitTabs`** — porta visível, funciona nas duas views.
- **Botão direito no chip do repo** (`Composer.svelte:745`): `oncontextmenu`, seguindo
  `Sidebar.svelte:939`.
- **Toque longo no chip**: mecânica de `Sidebar.svelte:302-313` (timer 500ms no `onpointerdown`,
  cancelado por movimento) **com a guarda `longPressed`** que suprime o clique seguinte — sem ela o
  toque longo abre o menu E o modal ao soltar.

**Não** pôr toque longo na linha do commit: lá o gesto concorre com selecionar e copiar o hash, que
é exatamente por que ele saiu das bolhas de mensagem (`UserBubble.svelte:22`). Lá continua o `⋯`.

- [ ] **Step 3: Apagar a `GitToolbar`**

```bash
git rm frontend/src/components/git/GitToolbar.svelte
```

- [ ] **Step 4: Gate**

Run: `npm --prefix frontend run check && grep -rn "GitToolbar" frontend/src/`
Expected: 0 erros, nenhuma referência órfã

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/git/RepoMenu.svelte frontend/src/components/git/GitTabs.svelte frontend/src/components/Composer.svelte
git commit -m "feat(git): acoes de repositorio no menu de contexto, no lugar da barra"
```

---

### Task 9: Estado "não é um repositório" + gate final + docs

**Files:**
- Modify: `frontend/src/components/git/GitTabs.svelte`
- Modify: `docs/USAGE.md` (seção `### Git`)

- [ ] **Step 1: Pasta que não é repo git**

Hoje `list_branches` estoura `GitError(409)` com o stderr cru do git, que apareceria nas quatro
abas. Antes de renderizar a fileira de abas:

```svelte
  {#if naoEhRepo}
    <p class="git-muted">esta pasta não é um repositório git</p>
  {:else}
    <!-- faixas + abas + corpo -->
  {/if}
```

Detectar pelo erro do `load()` (o texto do git contém `not a git repository` — o `_run` força
`LC_ALL=C`, então a mensagem não vem traduzida). **Não** exibir o stderr cru.

- [ ] **Step 2: Suíte completa**

Run: `cd backend && uv run pytest -q`
Expected: mesma contagem de antes do plano, 0 falhas

Run: `npm --prefix frontend run test && npm --prefix frontend run check && npm --prefix frontend run build`
Expected: testes passando, 0 erros de tipo, build ok

- [ ] **Step 3: Verificação manual — mobile E desktop** 🙋 verificação manual

Num repo de brinquedo (com commits que dá pra perder), nas DUAS views:

1. Abrir o modal: as três abas aparecem; a contagem no rótulo bate.
2. Trocar de aba e voltar: cada aba lembra o nível onde estava.
3. **Mudanças:** uma lista só, com checkbox E descartar na mesma linha; commitar funciona; repo
   limpo mostra "nada alterado" em vez de tela em branco.
4. **Histórico:** empilhado no desktop (lista / mensagem / arquivos), drill-down no celular; a
   mensagem completa do commit aparece (corpo, não só assunto); buscar filtra e o grafo some;
   trocar de aba e voltar preserva a busca.
5. **Branches:** o filtro por nome funciona **no desktop também** (hoje só existe no mobile).
6. O `⋯` de um commit abre o menu **por cima** do modal, não atrás.
7. Cherry-pick que conflita → faixa de conflito no cabeçalho, visível de qualquer aba; fechar e
   reabrir o modal mantém a faixa.
8. Erro do git aparece **uma vez**, na faixa — não duplicado.
9. Botão direito no chip do repo (desktop) e toque longo (celular) abrem o menu de ações; o toque
   longo **não** abre o modal junto ao soltar.
10. Abrir o modal numa sessão cujo cwd não é repo git: "esta pasta não é um repositório git", sem
    stderr cru.
11. Atravessar 820px com o modal aberto: não perde aba nem nível.

- [ ] **Step 4: Docs**

Em `docs/USAGE.md`, na seção `### Git`, reescrever a descrição do painel: agora é um modal com abas,
com o que cada aba faz, onde ficam as ações de repositório (botão direito / toque longo / `⋯`) e
como a faixa de conflito aparece.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/git/GitTabs.svelte docs/USAGE.md
git commit -m "feat(git): estado de pasta sem repositorio + docs do modal com abas"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** modal com abas (T4), empilhado no desktop e drill-down no mobile (T5, T6),
  conflito como faixa (T7), ações no menu de contexto (T8), filtro de branches nas duas views (T7),
  busca dentro da aba (T6), estados vazios (T5, T6, T7) e não-repo (T9), `%b` no backend (T1),
  z-index (T2), `desktop` por prop e morte do `GitPanel`/`GitSheet` (T4). Cada item tem task e
  verificação.
- **Consistência de tipos:** `GitTabId`/`GitNav`/`initialNav`/`selectTab`/`pushLevel`/`popLevel`/
  `currentLevel` definidos na T3 e consumidos com os mesmos nomes em T4-T7. `Git.svelte` recebe
  `{open, git, desktop, onClose}` na T4 e é montado com essas props nos três call sites.
  `CommitBox` muda de assinatura na T5 (`chosen: string[]`) e nada depois volta a passar a lista
  antiga. `commit.body` nasce na T1 e é consumido na T6.
- **Riscos declarados:** a T4 é a única com risco de regressão ampla (deleta dois componentes e
  mexe em três telas). Por isso ela termina com uma varredura de referências órfãs, e o refino das
  abas vem depois — se algo quebrar, quebra com a casca ainda simples.
- **O que fica pra depois:** divisórias arrastáveis (proporção fixa nesta entrega), seletor de
  branch no log, paginação além de 50 commits, ahead/behind no cabeçalho.

## Loop-readiness

- `check_cmd` por fase: T1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; T2-T8 →
  `npm --prefix frontend run check && npm --prefix frontend run test`; T9 →
  `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers.
