# Git como modal com abas (layout empilhado do TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) ou superpowers:executing-plans. Steps usam checkbox (`- [ ]`) pra rastreio.

**Goal:** Substituir o painel git de 3 colunas por um modal único com abas (Mudanças / Histórico / Branches), layout empilhado no desktop e drill-down no celular, com as ações de repositório saindo das barras pro menu de contexto.

**Architecture:** `GitTabs.svelte` é o único dono da navegação, e a navegação em si é um módulo puro testável (`lib/gitTabs.ts`). O invólucro é o `BottomSheet` que **já existe**, com `wide`+`centered` — no desktop ele já vira modal centrado `min(1100px, 92vw)` (é o que o `EnginesSheet.svelte:304` faz). As oito tasks primeiras só CRIAM arquivos; a nona troca os três pontos de montagem e apaga os cinco componentes velhos de uma vez.

**Tech Stack:** Svelte 5 (runes) + TypeScript, vitest pros módulos puros de `lib/`; Python 3.14 + FastAPI + pytest no backend.

**Spec:** [`../specs/2026-07-30-git-modal-abas-design.md`](../specs/2026-07-30-git-modal-abas-design.md) — ler antes de começar.

## Global Constraints

- **Duas views SEMPRE.** Toda mudança de UI entra no desktop E no celular, e a verificação manual testa as duas. A única diferença deliberada é empilhado (desktop) vs drill-down (mobile).
- **UI em pt-BR**; identificadores em inglês; comentários em pt-BR. Match de indentação/estilo do arquivo vizinho. **NUNCA rodar formatter.**
- **Falha aparece, não some.** Erro do git chega ao usuário com o texto do git. Uma faixa única de saída/erro no modal.
- **Nenhum commit pode deixar o git do app pior do que estava.** As tasks 1-8 só acrescentam arquivos; a 9 troca tudo de uma vez. Se uma task intermediária for interrompida, o app segue funcionando como hoje.
- **Backend git:** argv list sempre, shell string nunca. Rotas `def` com `Depends(require_auth)`.
- **Gates:** `npm --prefix frontend run check` (o `build` NÃO checa tipos), `npm --prefix frontend run test`, `cd backend && uv run pytest -q`.
- **Commits frequentes**, conventional commits, stage por path explícito — **nunca `git add -A`**.
- **Não criar nem trocar de branch.**

## O que já existe (não recriar)

`CommitList`, `CommitMenu`, `DiffView`, `BranchList`, `LogSearch` — reaproveitados como estão.
`gitStore.svelte.ts`. As 18 rotas git. **`BottomSheet` com `wide`+`centered` já é modal centrado no
desktop** (`BottomSheet.svelte:17-18`, regras em `:414-426`; precedente vivo no
`EnginesSheet.svelte:304`) — **não** usar `ModalDialog`, não inventar tokens de z-index: a folha
segue em z 100 e o 110/120 do `CommitMenu` continua correto.

`ChangedFiles`, `CommitDetail`, `GitToolbar`, `GitPanel`, `GitSheet` **morrem na Task 9** — não estão
na lista de reaproveitados.

## Non-goals

Escolher qual branch logar; paginação além dos 50 commits; ahead/behind no cabeçalho; divisórias
arrastáveis; unificar o menu de contexto do modal com o da linha da sidebar; `ModalDialog`.

---

### Task 1: Backend — mensagem completa do commit (`%b`)

**Files:**
- Modify: `backend/app/git_ops.py` (`_LOG_FMT` linha 216; parse em `git_log` linhas 236-253)
- Modify: `frontend/src/lib/api.ts` (`GitCommit`, linhas 861-873)
- Test: `backend/tests/test_git_ops.py` (fim do arquivo)

**Interfaces:**
- Produces: cada dict de `git_log` ganha `body: str` (`''` quando não há corpo); `GitCommit` ganha `body: string`

- [x] **Step 1: Escrever os testes que falham**

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
    d, _ = _repo_with_file(tmp_path)
    assert git_ops.git_log(d)[0]["body"] == ""


def test_git_log_corpo_com_separador_nao_trunca(tmp_path):
    # O corpo e texto livre: se contiver o proprio \x1f, um split sem maxsplit cortaria a mensagem
    # calada. Com maxsplit=8 o resto inteiro cai em f[8].
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "sep.txt").write_text("S\n")
    git_ops._run(d, "add", "sep.txt")
    git_ops._run(d, "commit", "-q", "-m", "assunto", "-m", "antes\x1fdepois")
    body = git_ops.git_log(d)[0]["body"]
    assert "antes" in body and "depois" in body
```

- [x] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "corpo" -v`
Expected: FAIL com `KeyError: 'body'`

- [x] **Step 3: Implementar**

`_LOG_FMT` hoje é (`git_ops.py:216`):

```python
_LOG_FMT = "%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1e"
```

São 8 campos, separador `\x1f`, registro `\x1e`. O `%b` entra como **9º**, antes do `%x1e`:

```python
_LOG_FMT = "%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1f%b%x1e"
```

No parse (`git_ops.py:240-243`), trocar por:

```python
        # maxsplit=8: o corpo (%b) e texto livre e pode conter o proprio \x1f — sem o limite, um
        # commit com esse byte no corpo sairia truncado calado.
        f = rec.split("\x1f", 8)
        if len(f) < 9:
            continue
        full, short, parents, refs, author, ts, rel, subject = f[:8]
        body = f[8].strip("\n")
```

e o dict ganha `"body": body,` junto de `"subject"`.

Em `frontend/src/lib/api.ts`, na interface `GitCommit`, depois de `subject`:

```typescript
  body: string;       // corpo da mensagem (%b), sem o assunto; '' quando o commit nao tem corpo
```

- [x] **Step 4: Rodar e ver passar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -q && cd backend && uv run python app/git_ops.py && npm --prefix frontend run check`
Expected: PASS + `git_ops self-check OK` + 0 erros de tipo

- [x] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/tests/test_git_ops.py frontend/src/lib/api.ts
git commit -m "feat(git): git_log traz o corpo da mensagem do commit"
```

---

### Task 2: `lib/gitTabs.ts` — a navegação como módulo puro

**Files:**
- Create: `frontend/src/lib/gitTabs.ts`
- Test: `frontend/src/lib/gitTabs.test.ts`

**Interfaces:**
- Produces: `GitTabId`, `GIT_TABS`, `GitNav`, `initialNav()`, `selectTab(nav, tab)`, `pushLevel(nav)`, `popLevel(nav)`, `currentLevel(nav)`

- [x] **Step 1: Escrever os testes que falham**

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
    expect(currentLevel(n)).toBe(0);
    n = pushLevel(n); n = pushLevel(n);  // history -> 2
    n = selectTab(n, 'changes');
    expect(currentLevel(n)).toBe(1);
    n = selectTab(n, 'history');
    expect(currentLevel(n)).toBe(2);
  });

  it('para no teto de cada aba — valores cravados', () => {
    // Cravado de proposito: comparar com GIT_TABS.maxLevel passaria com qualquer numero.
    let c = initialNav();
    for (let i = 0; i < 9; i++) c = pushLevel(c);
    expect(currentLevel(c)).toBe(1);                       // changes: lista -> diff

    let h = selectTab(initialNav(), 'history');
    for (let i = 0; i < 9; i++) h = pushLevel(h);
    expect(currentLevel(h)).toBe(2);                       // history: lista -> commit -> diff

    let b = selectTab(initialNav(), 'branches');
    for (let i = 0; i < 9; i++) b = pushLevel(b);
    expect(currentLevel(b)).toBe(0);                       // branches: so a lista
  });

  it('não desce abaixo de zero', () => {
    expect(currentLevel(popLevel(popLevel(initialNav())))).toBe(0);
  });

  it('a aba ativa sobrevive a mudar a ordem das abas', () => {
    // O ponto do teste: a selecao guarda o ID. Se guardasse indice, mexer na lista de abas trocaria
    // a aba ativa debaixo do usuario (a mesma classe de bug do plan_name no _list_sig).
    const n = selectTab(initialNav(), 'branches');
    const ordemInvertida = [...GIT_TABS].reverse();
    const aindaExiste = ordemInvertida.some((t) => t.id === n.tab);
    expect(aindaExiste).toBe(true);
    expect(n.tab).toBe('branches');
  });

  it('não muta a entrada', () => {
    const a = initialNav();
    const b = pushLevel(a);
    expect(currentLevel(a)).toBe(0);
    expect(b).not.toBe(a);
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npm --prefix frontend run test -- gitTabs`
Expected: FAIL — módulo não existe

- [x] **Step 3: Implementar**

```typescript
// Navegacao do modal de git: qual aba esta ativa e em que nivel cada uma parou.
//
// Nivel, nao pilha: a profundidade maxima e 2 (Historico -> commit -> diff), 1 (Mudancas -> diff) e
// 0 (Branches). Uma pilha de navegacao seria maior que o problema.
//
// A aba ativa e guardada por ID, nunca por indice: mexer na lista de abas nao pode trocar a aba
// ativa debaixo do usuario (mesma classe do plan_name no _list_sig).

export type GitTabId = 'changes' | 'history' | 'branches';

export const GIT_TABS = [
  { id: 'changes',  label: 'Mudanças',  maxLevel: 1 },
  { id: 'history',  label: 'Histórico', maxLevel: 2 },
  { id: 'branches', label: 'Branches',  maxLevel: 0 },
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

- [x] **Step 4: Rodar e ver passar**

Run: `npm --prefix frontend run test -- gitTabs && npm --prefix frontend run check`
Expected: 6 testes passando, 0 erros

- [x] **Step 5: Commit**

```bash
git add frontend/src/lib/gitTabs.ts frontend/src/lib/gitTabs.test.ts
git commit -m "feat(git): modulo puro da navegacao por abas do modal"
```

---

### Task 3: Mover as quatro funções de diff pro `gitStore`

Hoje `openDiff`, `openCommitFileDiff`, `openCommitFullDiff` e `openCommitWorktreeDiff` existem
**duplicadas**: `GitSheet.svelte:77-180` e `GitPanel.svelte:33-130` (≈130 linhas cada cópia). Elas
carregam o Shiki por import dinâmico, tratam `truncated`, mexem em `git.busy` e desfazem o estado no
erro. Se os dois donos morrem sem substituto escrito, esse comportamento se perde.

**Files:**
- Modify: `frontend/src/lib/gitStore.svelte.ts`

**Interfaces:**
- Produces, no store: estado `diffPath`, `diffRows`, `diffLoading`, `diffSha`, `diffTruncated`; e os métodos `openFileDiff(path)`, `openCommitFileDiff(sha, path)`, `openCommitFullDiff(c)`, `openCommitWorktreeDiff(c)`, `closeDiff()`

- [x] **Step 1: Ler as quatro funções antes de mover**

Run: `sed -n '77,182p' frontend/src/components/GitSheet.svelte`

As quatro têm a MESMA forma: zera estado → `diffLoading = true` → `git.busy = …` → fetch → `import('../lib/highlight')` → `highlightDiff(diff, titulo)` → no erro limpa `diffPath`/`diffSha` e grava `git.error` → `finally` desliga `diffLoading` e `git.busy`. Preservar isso; a diferença entre elas é só o client chamado e o título.

- [x] **Step 2: Mover pro store**

Acrescentar ao `createGitStore`, seguindo o estilo do arquivo (funções `async`, `cleanErr` no catch):

```typescript
  // Estado do diff aberto. Vivia duplicado no GitSheet e no GitPanel; com o modal de abas ha um
  // dono so, e as abas passam a ser burras.
  let diffPath = $state('');
  let diffRows = $state<DiffRow[]>([]);
  let diffLoading = $state(false);
  let diffSha = $state('');          // '' = diff da working tree
  let diffTruncated = $state(false); // backend cortou em 200KB

  function closeDiff() {
    diffPath = ''; diffSha = ''; diffRows = []; diffTruncated = false;
  }

  // Helper unico das quatro entradas: muda so o titulo e o fetch.
  async function _abrirDiff(titulo: string, sha: string, chave: string, buscar: () => Promise<{ diff: string; truncated?: boolean }>) {
    if (busy) return false;
    diffSha = sha; diffPath = titulo; diffRows = []; diffTruncated = false;
    diffLoading = true; busy = chave; error = '';   // chave EXPLICITA: no diff de arquivo dentro de commit o valor certo e o path, nao o sha
    try {
      const r = await buscar();
      diffTruncated = !!r.truncated;
      const { highlightDiff } = await import('./highlight');
      diffRows = await highlightDiff(r.diff, titulo);
      return true;
    } catch (e) {
      error = cleanErr(e);
      closeDiff();
      return false;
    } finally {
      diffLoading = false; busy = '';
    }
  }

  const openFileDiff = (path: string) =>
    _abrirDiff(path, '', path, () => getFileDiff(sessionName, path));
  const openCommitFileDiff = (sha: string, path: string) =>
    _abrirDiff(path, sha, path, () => getCommitFileDiff(sessionName, sha, path));
  const openCommitFullDiff = (c: GitCommit) =>
    _abrirDiff(`commit ${c.short}`, c.hash, c.hash, () => getCommitDiff(sessionName, c.hash));
  const openCommitWorktreeDiff = (c: GitCommit) =>
    _abrirDiff(`commit ${c.short} ↔ working tree`, c.hash, c.hash, () => getCommitDiffVsWorktree(sessionName, c.hash));
```

Expor os cinco métodos e os cinco getters no `return` do store. Importar `getFileDiff`,
`getCommitFileDiff`, `getCommitDiff`, `getCommitDiffVsWorktree` e `type DiffRow`.

O retorno `boolean` é o que as abas usam pra decidir se descem de nível: falhou, não desce.

- [x] **Step 3: Gate**

Run: `npm --prefix frontend run check && npm --prefix frontend run test`
Expected: 0 erros, testes passando

Os dois componentes velhos continuam com as cópias deles e seguem funcionando — **nada quebra
nesta task**. As cópias morrem junto com os arquivos, na Task 9.

- [x] **Step 4: Commit**

```bash
git add frontend/src/lib/gitStore.svelte.ts
git commit -m "refactor(git): estado do diff vira dono unico no gitStore"
```

---

### Task 4: `GitChangesTab.svelte` — uma lista só, com checkbox e descartar

Hoje `ChangedFiles` e `CommitBox` renderizam **cada um** a lista de arquivos alterados: um com ⟲
descartar (`ChangedFiles.svelte:26-48`), outro com checkbox (`CommitBox.svelte:65-78`). Numa aba só,
viraria duas listas do mesmo.

**Files:**
- Create: `frontend/src/components/git/GitChangesTab.svelte`

**Interfaces:**
- Consumes: `GitStore` (com os métodos de diff da Task 3), `CommitBox`, `DiffView`
- Produces: props `{ git: GitStore; desktop: boolean; level: number; onPush: () => void; onPop: () => void }`

- [x] **Step 1: Ler os dois componentes que se fundem**

Run: `cat frontend/src/components/git/ChangedFiles.svelte && sed -n '1,110p' frontend/src/components/git/CommitBox.svelte`

Anotar, pra migrar sem perder:
- o confirm em 2 passos do descartar (`confirmDiscard`, `ChangedFiles.svelte:13,43-48`)
- **a seleção padrão**: `CommitBox.svelte:9-16` marca TODOS por padrão e usa `selectionInitialized`
  pra que um desmarque manual não seja refeito no próximo `refresh()`. Sem isso, ou nasce nada
  marcado, ou a seleção do usuário é sobrescrita a cada poll.
- os botões `todos`/`nenhum` (`CommitBox.svelte:65-68`)

- [x] **Step 2: Escrever o componente**

A aba é dona de `sel: Set<string>`, `selectionInitialized`, `toggle`, `todos`/`nenhum` — migrados
literalmente do `CommitBox`. Cada linha tem: checkbox, código do status, caminho (clicável → chama
`git.openFileDiff(path)` e, se voltar `true`, `onPush()`), e o ⟲ descartar com confirm em 2 passos.

Vazio obrigatório — hoje `ChangedFiles.svelte:26` (`{#if git.dirty && git.files.length}`) não
renderiza NADA com repo limpo, e a aba nasceria em branco:

```svelte
  {#if !git.files.length}
    <p class="git-muted">nada alterado — a working tree está limpa</p>
  {:else}
    <!-- … -->
  {/if}
```

Desktop (`desktop === true`): lista, diff do arquivo, `CommitBox` — três painéis em `flex` com
proporção fixa e `overflow: auto` cada.
Mobile: `level === 0` mostra lista + `CommitBox`; `level === 1` mostra `DiffView` com voltar
(`onPop()` + `git.closeDiff()`).

O `CommitBox` é montado **com a lista dele escondida** — a Task 9 remove o bloco de lá; até lá, esta
aba passa `chosen` por prop e o `CommitBox` ainda tem a própria lista. Pra não duplicar visualmente
antes da hora, **esta task monta o `CommitBox` só a partir da Task 9**; até lá renderiza a lista, o
diff, e um `<textarea>` + botão que chamam `git.doCommit(msg, chosen)` diretamente.

> Simplificação deliberada: a aba nasce com um commit box mínimo (mensagem + confirmar) e ganha o
> `CommitBox` completo (recentes, amend, branch nova) na Task 9, quando o componente perde a lista
> própria. Isso mantém a regra de "nenhuma task intermediária piora o app" — o `CommitBox` velho
> segue intacto e em uso pelo `GitSheet` até lá.

- [x] **Step 3: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 4: Commit**

```bash
git add frontend/src/components/git/GitChangesTab.svelte
git commit -m "feat(git): aba Mudancas com uma lista so (checkbox + descartar)"
```

---

### Task 5: `GitHistoryTab.svelte` + a mensagem e os arquivos em painéis próprios

`CommitDetail.svelte:22-45` é mensagem + metadados + lista de arquivos num componente só, com
`max-height: 52vh` (`:49`) e `68vh` no desktop (`:53`). O empilhado quer dois painéis, e sem
`max-height` próprio — quem limita altura passa a ser o empilhado.

**Files:**
- Create: `frontend/src/components/git/GitHistoryTab.svelte`
- Create: `frontend/src/components/git/CommitMessage.svelte`
- Create: `frontend/src/components/git/CommitFiles.svelte`

**Interfaces:**
- Consumes: `CommitList`, `CommitMenu`, `LogSearch`, `DiffView`, `commit.body` (Task 1), métodos de diff (Task 3)
- Produces:
  - `CommitMessage.svelte` props `{ commit: GitCommit }`
  - `CommitFiles.svelte` props `{ commit: GitCommit; sessionName: string; onOpenFile: (p: string) => void; onMenu?: (c: GitCommit) => void }`
  - `GitHistoryTab.svelte` props `{ git: GitStore; desktop: boolean; level: number; onPush: () => void; onPop: () => void }`

- [x] **Step 1: Ler o `CommitDetail` antes de partir**

Run: `cat frontend/src/components/git/CommitDetail.svelte`

Ele busca os arquivos sozinho (`getCommitFiles` num `$effect`, `:15-19`). Esse fetch vai pro
`CommitFiles`.

- [x] **Step 2: `CommitMessage.svelte` e `CommitFiles.svelte`**

`CommitMessage`: assunto em destaque, `commit.body` em `white-space: pre-wrap` (vazio some, não
deixa espaço morto), autor e data. **Sem `max-height`.**

`CommitFiles`: o `$effect` com `getCommitFiles` copiado do `CommitDetail`, a lista com `onOpenFile`,
e o botão `⋯ ações` sob `{#if onMenu}` — prop **opcional** de propósito (outro plano reusa isto sem
menu). **Sem `max-height`.**

- [x] **Step 3: `GitHistoryTab.svelte`**

**Carrega o log ao entrar.** Hoje quem chama `git.openLog()` é o `GitSheet.svelte:73` — e **só no
desktop** (`if (isDesktop)`) — mais o botão `log` da `GitToolbar`. Os dois morrem na Task 9, e o
`RepoMenu` não terá `log`. Sem isto a aba nasce vazia nas duas views:

```svelte
  // Carrega na primeira vez que a aba aparece. Sem isto o log fica vazio: git.load() so faz
  // refresh() (branches + arquivos), quem preenche `commits` e openLog().
  let carregou = false;
  $effect(() => { if (!carregou) { carregou = true; git.openLog(); } });
```

Desktop, empilhado com proporção fixa e `overflow: auto` por painel:
1. `<LogSearch {git} />` — **dentro da aba**, porque a busca só vale aqui
2. `<CommitList commits={git.commits} wtCount={0} noGraph={!!git.logQuery} … />`
3. `<CommitMessage commit={selecionado} />`
4. `<CommitFiles commit={selecionado} … />`

Sem commit selecionado: "selecione um commit" **uma vez só**, não em dois painéis. Log vazio: "sem
commits ainda".

> `wtCount={0}`: a linha sintética "Working tree changes" sai do log — ela agora é a aba Mudanças, e
> repetir a mesma porta em dois lugares é o tipo de duplicação que motivou este redesenho.

Mobile: `level 0` = busca + lista; `level 1` = `CommitMessage` + `CommitFiles`; `level 2` = diff.

O `CommitMenu` é montado aqui (`menuCommit` é estado desta aba), com `onShowDiff` e
`onShowWorktreeDiff` ligados nos métodos do store (Task 3).

- [x] **Step 4: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 5: Commit**

```bash
git add frontend/src/components/git/GitHistoryTab.svelte frontend/src/components/git/CommitMessage.svelte frontend/src/components/git/CommitFiles.svelte
git commit -m "feat(git): aba Historico empilhada, com mensagem e arquivos em paineis proprios"
```

---

### Task 6: `GitBranchesTab.svelte` + `GitStatusBar.svelte`

**Files:**
- Create: `frontend/src/components/git/GitBranchesTab.svelte`
- Create: `frontend/src/components/git/GitStatusBar.svelte`

**Interfaces:**
- Produces:
  - `GitBranchesTab.svelte` props `{ git: GitStore }`
  - `GitStatusBar.svelte` props `{ git: GitStore; menuAberto: boolean }`

- [x] **Step 1: `GitBranchesTab.svelte`**

`BranchList` mais o campo de filtro. Hoje o filtro só existe no mobile
(`GitSheet.svelte:265-274`) e ainda **condicionado** a
`{#if git.branches.length > 6 || git.remotes.length}` (`:264`); o desktop passava `filter=""`. Na aba
dedicada ele é **incondicional**.

`BranchList` exige `filter: string` (obrigatória, sem default — `BranchList.svelte:6-10`), então a
aba guarda o estado e passa.

Vazio: **não** escrever texto novo — `BranchList.svelte:29` já tem "nenhuma branch local" e a
variante com filtro. Dois textos concorrentes é pior que um.

- [x] **Step 2: `GitStatusBar.svelte`**

Vai no **rodapé** do modal (o spec desenha ali, e é onde o `<pre>` de saída mora hoje):

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  interface Props { git: GitStore; menuAberto: boolean }
  let { git, menuAberto }: Props = $props();

  // Confirm em 2 passos, e o reset SO no sucesso: um abort que o git recusou nao pode voltar o
  // botao pro estado inicial, senao o proximo conflito ja aparece em "confirmar" (regra que hoje
  // vive em GitToolbar.svelte:12-17).
  let confirmar = $state(false);
  async function doAbort() {
    if (await git.abortOp()) confirmar = false;
  }
</script>

{#if git.pendingAbort}
  <div class="gsb-conflito" role="status">
    <span>⚠ {git.pendingAbort === 'revert-abort' ? 'revert' : 'cherry-pick'} em conflito</span>
    {#if confirmar}
      <button class="git-mini danger" disabled={!!git.busy} onclick={doAbort}>confirmar abort</button>
      <button class="git-mini" onclick={() => (confirmar = false)}>não</button>
    {:else}
      <button class="git-mini danger" onclick={() => (confirmar = true)}>abortar…</button>
    {/if}
  </div>
{/if}
{#if git.error && !menuAberto}<p class="gsb-erro">{git.error}</p>{/if}
{#if git.output}<pre class="gsb-saida">{git.output}</pre>{/if}
```

`.gsb-saida` leva `max-height: 200px; overflow: auto` — igual ao `<pre>` de hoje
(`GitSheet.svelte:343`). Sem teto, um `git status` de repo sujo empurra o conteúdo pra fora da tela.

O `menuAberto` existe porque o `CommitMenu` fica por cima: com ele aberto, quem mostra o erro é o
menu (é o padrão `{#if git.error && !menuCommit}` que hoje vive em `GitPanel.svelte:163` e
`GitSheet.svelte:288`).

- [x] **Step 3: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 4: Commit**

```bash
git add frontend/src/components/git/GitBranchesTab.svelte frontend/src/components/git/GitStatusBar.svelte
git commit -m "feat(git): aba Branches com filtro e faixa de conflito/saida"
```

---

### Task 7: `RepoMenu.svelte` — as ações de repositório

**Files:**
- Create: `frontend/src/components/git/RepoMenu.svelte`

**Interfaces:**
- Produces: props `{ git: GitStore; onClose: () => void; soltoNaTela?: boolean }`

- [x] **Step 1: Ler o vocabulário do menu que já existe**

Run: `cat frontend/src/components/SessionContextMenu.svelte`

Seguir a forma dele (backdrop, posicionamento, fechar no Esc).

- [x] **Step 2: Escrever o componente**

Seis itens. **Atenção ao `push`:** `GitAction` (`api.ts:771`) e `_ACTIONS`
(`git_ops.py:189-201`) **não têm `push`** — a toolbar de hoje usa `git.doPush()`
(`GitToolbar.svelte:25`). Os outros cinco vão por `git.runAction(...)`:

| Item | Chamada |
|---|---|
| status | `git.runAction('status')` |
| log | `git.openLog()` |
| fetch | `git.runAction('fetch')` |
| pull | `git.runAction('pull')` |
| push | `git.doPush()` ← **não** `runAction` |
| stash | `git.runAction('stash')` |
| pop | `git.runAction('stash-pop')` |

`soltoNaTela` existe porque o menu abre em dois contextos: **dentro** do modal (a saída aparece na
`GitStatusBar`) e **a partir do chip do repo com o modal fechado** (aí não há faixa nenhuma). Com
`soltoNaTela`, o próprio menu mostra `git.output`/`git.error` depois da ação, em vez de a falha
sumir.

- [x] **Step 3: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 4: Commit**

```bash
git add frontend/src/components/git/RepoMenu.svelte
git commit -m "feat(git): menu de contexto das acoes de repositorio"
```

---

### Task 8: `GitTabs.svelte` — cabeçalho, abas e corpo

**Files:**
- Create: `frontend/src/components/git/GitTabs.svelte`

**Interfaces:**
- Consumes: tudo das Tasks 2 e 4-7
- Produces: props `{ git: GitStore; desktop: boolean; onClose: () => void }`

- [x] **Step 1: O cabeçalho — porque não vem de graça**

Nem o `BottomSheet` nem o `ModalDialog` desenham chrome: o `×` da folha só existe no modo
`persistent` (`BottomSheet.svelte:237-240`). Sem cabeçalho próprio, o modal sai só por Esc/backdrop
e **nunca diz de que repositório é** — que importa porque ele abre pela linha da sidebar, sem abrir
o chat.

O cabeçalho tem: nome do repo · branch atual · `⋯` (abre o `RepoMenu`) · `✕` (chama `onClose`).

- [x] **Step 2: Abas e corpo**

```svelte
<script lang="ts">
  import { GIT_TABS, initialNav, selectTab, pushLevel, popLevel, currentLevel, type GitNav } from '../../lib/gitTabs';
  import GitChangesTab from './GitChangesTab.svelte';
  import GitHistoryTab from './GitHistoryTab.svelte';
  import GitBranchesTab from './GitBranchesTab.svelte';
  import GitStatusBar from './GitStatusBar.svelte';
  import RepoMenu from './RepoMenu.svelte';
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props { git: GitStore; desktop: boolean; onClose: () => void }
  let { git, desktop, onClose }: Props = $props();

  let nav = $state<GitNav>(initialNav());
  let repoMenu = $state(false);
  let menuAberto = $state(false);   // CommitMenu aberto na aba Historico -> a faixa cala o erro

  // Contagem no rotulo: `branches` conta locais + remotas porque o BranchList mostra as duas
  // (BranchList.svelte) — contar so as locais daria um numero que nao bate com a lista.
  const contagem = (id: string) =>
    id === 'changes' ? git.files.length
    : id === 'branches' ? git.branches.length + git.remotes.length
    : 0;
</script>
```

Corpo: `{#if nav.tab === 'changes'}` → `GitChangesTab` com
`level={currentLevel(nav)} onPush={() => (nav = pushLevel(nav))} onPop={() => (nav = popLevel(nav))}`;
idem pras outras duas. `GitStatusBar` no **rodapé**, com `{menuAberto}`.

Pasta que não é repo git — antes de qualquer aba:

```svelte
  {#if git.error && /not a git repository/i.test(git.error)}
    <p class="git-muted">esta pasta não é um repositório git</p>
  {:else}
    <!-- cabecalho + abas + corpo + faixa -->
  {/if}
```

O `_run` força `LC_ALL=C` (`git_ops.py:54`), então a mensagem do git não vem traduzida e o teste de
texto é estável. **Não** exibir o stderr cru.

CSS das abas: `overflow-x: auto` e **`touch-action: pan-x` próprio** — o `BottomSheet` declara
`touch-action: pan-y` (`:276`) e sem isso a fileira não rola no dedo.

- [x] **Step 3: Gate**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 4: Commit**

```bash
git add frontend/src/components/git/GitTabs.svelte
git commit -m "feat(git): GitTabs com cabecalho, abas e faixa de estado"
```

---

### Task 9: A troca — montar o novo e apagar os cinco velhos

**A única task destrutiva.** Tudo que ela monta já existe e já passou pelo gate.

**Files:**
- Create: `frontend/src/components/Git.svelte`
- Modify: `frontend/src/components/git/CommitBox.svelte` (perde a lista própria)
- Modify: `frontend/src/screens/Chat.svelte:1268`, `frontend/src/components/Sidebar.svelte:1304`, `frontend/src/screens/SessionList.svelte:957`
- Modify: `frontend/src/components/git/GitChangesTab.svelte` (passa a montar o `CommitBox`)
- Delete: `GitPanel.svelte`, `GitSheet.svelte`, `git/ChangedFiles.svelte`, `git/CommitDetail.svelte`, `git/GitToolbar.svelte`
- Modify: `docs/USAGE.md`

- [ ] **Step 1: `Git.svelte`**

```svelte
<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import GitTabs from './git/GitTabs.svelte';
  import { createGitStore } from '../lib/gitStore.svelte';

  // `desktop` por PROP, nao matchMedia proprio: o GitSheet era a terceira copia da mesma media
  // query (App.svelte:158-167, BottomSheet.svelte:28) e a primeira pintura saia mobile.
  interface Props { open: boolean; sessionName: string; desktop: boolean; onClose: () => void }
  let { open, sessionName, desktop, onClose }: Props = $props();

  // Dono do store — era do GitSheet.svelte:27-31, COM o guard que evita recriar a cada render.
  // Sem ele, trocar de sessao com o modal aberto mostraria o git da anterior.
  let git = $state(createGitStore(sessionName));
  $effect(() => { if (git.sessionName !== sessionName) git = createGitStore(sessionName); });
  $effect(() => { if (open) git.load(); });
</script>

<!-- `wide` + `centered` = no desktop a folha JA vira modal centrado min(1100px, 92vw)
     (BottomSheet.svelte:17-18, regras em :414-426). Mesmo par que o EnginesSheet.svelte:304 usa.
     Nao usar ModalDialog: a folha fica em z 100, e o 110/120 do CommitMenu segue correto. -->
<BottomSheet {open} {onClose} ariaLabel="Git" wide={desktop} centered={desktop}>
  <GitTabs {git} {desktop} {onClose} />
</BottomSheet>
```

- [ ] **Step 2: `CommitBox` perde a lista, e a aba Mudanças o adota**

Remover do `CommitBox.svelte` o bloco `.cb-sel-row` (`:65-68`) e `.cb-files` (`:69-78`), e o
`git.error` de `:102` (a faixa é a dona agora). A prop vira:

```typescript
  interface Props { git: GitStore; chosen: string[]; onDone?: () => void }
```

usando `chosen` onde hoje usa o `$derived` interno (`:23`). **Todo o resto fica idêntico** —
mensagens recentes (`MSG_KEY` segue em `git.sessionName`), amend, branch nova, `canCommit`,
`doCommit`. É código entregue e revisado; não reescrever.

No `GitChangesTab`, trocar o commit box mínimo da Task 4 pelo `<CommitBox {git} chosen={chosen} />`.

- [ ] **Step 3: Trocar os três pontos de montagem**

Levantado, não suposto:

- **`Chat.svelte:1268`** → `<Git open={gitOpen} {sessionName} desktop={desktop} onClose={() => (gitOpen = false)} />`. O `Chat` já tem `desktop?: boolean` (`:54`, default `false` em `:68`); quem passa `true` é o `DesktopShell.svelte:218,235,253`.
- **`SessionList.svelte:957`** → `desktop={false}` fixo. A tela **não tem** prop de desktop (`:27-32`) e é mobile-only por construção (`App.svelte:398-403` só a renderiza depois do ramo `isDesktop`).
- **`Sidebar.svelte:1304`** → `desktop={true}` (desktop-only, comentado em `:1324`).

**Preservar o `closeGitSheet`** (`Sidebar.svelte:451-454`, restaura o servidor via `selectServer`) e
o gêmeo em `SessionList.svelte:390`, ligados no `onClose`.

- [ ] **Step 4: Apagar os cinco**

```bash
git rm frontend/src/components/GitPanel.svelte frontend/src/components/GitSheet.svelte frontend/src/components/git/ChangedFiles.svelte frontend/src/components/git/CommitDetail.svelte frontend/src/components/git/GitToolbar.svelte
```

- [ ] **Step 5: Varredura de referências órfãs**

Run: `grep -rn "from '.*GitSheet.svelte'\|from '.*GitPanel.svelte'\|from '.*ChangedFiles.svelte'\|from '.*CommitDetail.svelte'\|from '.*GitToolbar.svelte'" frontend/src/`
Expected: nenhum resultado

Grep por nome puro casa também `closeGitSheet`/`gitSheet` (variáveis legítimas em `Sidebar.svelte:439-451,644,1287` e `SessionList.svelte:380-390`) — **essas ficam**, é só o nome.

Corrigir os comentários que passam a mentir: `PairSheet.svelte:174`, `LoopSheet.svelte:18`,
`SessionContextMenu.svelte:18`, `DesktopSessionContext.svelte:50`, `CommitMenu.svelte:165-167` (que
já cita linhas erradas hoje: `GitSheet:206`/`GitPanel:102`, quando o real é `:288`/`:163`),
`DiffView.svelte:19-20`, `BranchList.svelte:4`, `gitStore.svelte.ts:1`.

- [ ] **Step 6: Gates completos**

Run: `cd backend && uv run pytest -q`
Expected: 1200 passed (1197 de hoje + 3 da Task 1), 1 skipped

Run: `npm --prefix frontend run test && npm --prefix frontend run check && npm --prefix frontend run build`
Expected: 259+6 testes passando, 0 erros de tipo, build ok

- [ ] **Step 7: Verificação manual — mobile E desktop** 🙋 verificação manual

Num repo de brinquedo (com commits que dá pra perder), nas DUAS views:

1. Abrir pelo chip do repo, pelo botão da linha da sidebar e pelo `MoreSheet` — os três abrem, e o cabeçalho diz de que repo é.
2. Desktop: é modal **centrado**, não painel colado na direita. Celular: folha subindo de baixo.
3. Trocar de aba e voltar: cada aba lembra o nível onde estava.
4. **Mudanças:** uma lista só, com checkbox E descartar na mesma linha; commitar funciona com amend e branch nova; repo limpo mostra "nada alterado".
5. **Histórico:** carrega sozinha ao entrar, **inclusive no celular** (hoje o log só carregava no desktop); empilhado no desktop, drill-down no celular; a mensagem completa aparece (corpo, não só assunto); buscar filtra, grafo some, e a busca sobrevive à troca de aba.
6. **Branches:** filtro por nome funciona **no desktop também**, e aparece sempre (hoje só no celular e só com mais de 6 branches).
7. O `⋯` de um commit abre o menu **por cima** do modal.
8. Cherry-pick que conflita → faixa no rodapé, visível de qualquer aba; fechar e reabrir mantém.
9. Erro do git aparece **uma vez**.
10. `⋯` do cabeçalho: `pull` e `push` funcionam (push é `doPush`, não `runAction`).
11. Botão direito no chip do repo **com o modal fechado** → menu solto; um `pull` que falha mostra o erro no próprio menu.
12. Trocar de sessão com o modal aberto → mostra o git da sessão nova.
13. Sessão cujo cwd não é repo git: "esta pasta não é um repositório git", sem stderr cru.
14. Atravessar 820px com o modal aberto: não perde aba nem nível.

- [ ] **Step 8: Docs**

`docs/USAGE.md`, seção `### Git`: reescrever pro modal com abas. E conferir
`docs/future-features.md:71-86` e `docs/git-manager-research.md`, que descrevem a `GitSheet` — se
citarem a estrutura antiga, corrigir.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/Git.svelte frontend/src/components/git/CommitBox.svelte frontend/src/components/git/GitChangesTab.svelte frontend/src/screens/Chat.svelte frontend/src/components/Sidebar.svelte frontend/src/screens/SessionList.svelte docs/USAGE.md
git commit -m "feat(git): modal com abas substitui o painel e a folha de git"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** modal com abas (T8, T9), empilhado/drill-down (T4, T5), conflito como faixa
  (T6), ações no menu de contexto (T7), filtro de branches nas duas views (T6), busca dentro da aba
  (T5), estados vazios (T4, T5, T6) e não-repo (T8), `%b` (T1), `desktop` por prop e morte dos cinco
  componentes (T9).
- **Ordem:** as tasks 1-8 **só criam arquivos**. Nenhuma delas mexe no que está no ar — o `GitSheet`
  velho segue montado e funcionando até a T9. Isso responde ao achado de que a versão anterior deste
  plano deixava quatro commits com o git manco (sem commit, sem revert, sem toolbar, sem erro na
  tela).
- **Consistência de tipos:** `GitTabId`/`GitNav`/`initialNav`/`selectTab`/`pushLevel`/`popLevel`/
  `currentLevel` na T2, consumidos com os mesmos nomes na T8. Os métodos de diff nascem na T3 e são
  consumidos em T4/T5. `Git.svelte` recebe `{open, sessionName, desktop, onClose}` — casa com o que
  os três call sites já passam. `CommitBox` só muda de assinatura na T9, junto de quem o monta.
- **O que fica pra depois:** divisórias arrastáveis, seletor de branch no log, paginação, ahead/behind.

## Loop-readiness

- `check_cmd` por fase: T1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; T2-T8 →
  `npm --prefix frontend run check && npm --prefix frontend run test`; T9 →
  `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers.
