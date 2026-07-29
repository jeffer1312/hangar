# Git stash manager (estilo TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Substituir os botões cegos `stash`/`pop` da toolbar por um gerenciador de stash de verdade: listar stashes, ver o diff ANTES de aplicar, aplicar/pop/drop por entrada — o "Stash" do TortoiseGit.

**Architecture:** Backend: `stash_list` (`--format` estruturado), `stash_show` (`-p`) e `stash_op` (`apply`/`pop`/`drop` com ref validado por regex estrita) em `git_ops.py`, expostos como rotas `def` em `api.py`. Front: clients em `api.ts`, estado+métodos no `gitStore`, componente novo `StashList.svelte` usado pelas DUAS views (push-view `stash` no `GitSheet`; zona direita no `GitPanel`), e `GitToolbar` troca os botões `stash`/`pop` por um único `stash` que abre a view. Este plano é a fatia 4 de 5 do antigo plano monolítico (removido); os outros: commit dialog, log hub, blame/histórico, branch/tag.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repos git temporários; Svelte 5 (runes) + TypeScript; diff do stash renderizado com o pipeline Shiki já existente (`lib/highlight.ts`, import dinâmico).

## Pré-requisitos

Nenhum — plano autocontido. Pode rodar em qualquer ordem da série. Se o plano 2 (log hub) já rodou, a mudança no `GitToolbar` é ADITIVA (a prop `onStash` e o botão condicional `abort` dele coexistem).

## Referências

**TortoiseGit (UX a replicar):**
- Stash: https://tortoisegit.org/docs/tortoisegit/tgit-dug-stash.html

**Git (flags usadas):**
- git-stash(1) `list --format`, `show -p`, `apply`/`pop`/`drop` — https://git-scm.com/docs

**Internas (código existente a estender — LER antes de codar cada task):**
- `backend/app/git_ops.py` — `_ACTIONS` (linha 189-198: a ação `stash` com `--include-untracked` PERMANECE, é o "guardar tudo" da view), `git_action`, `_run`, `GitError`
- `backend/tests/test_git_ops.py` — helper `_repo_with_file(tmp_path)` + `test_stash_and_pop_roundtrip` (padrão de teste de stash; NÃO existem fixtures `init_repo`/`head_sha`)
- `backend/app/api.py` — rotas git (1855-1937), import de git_ops (41-42), `_StrictBody` (449), `Literal` já importado
- `frontend/src/lib/api.ts` — `gitAction`/`GitAction` (758-764)
- `frontend/src/lib/gitStore.svelte.ts` — padrão `busy`/`error`/`refresh()` dos métodos (`discard` é o mais próximo)
- `frontend/src/components/git/GitToolbar.svelte` — botões `stash`/`pop` atuais (saem)
- `frontend/src/components/GitSheet.svelte` — enum `GitView`, `openDiff` (a view de diff do stash reusa), botão voltar do diff (`diffSha ? 'commit' : 'list'`)
- `frontend/src/components/GitPanel.svelte` — encadeamento da zona direita
- `frontend/src/components/git/ChangedFiles.svelte` — padrão `confirmDiscard` (confirm inline destrutivo) e `.git-mini`

## Global Constraints

- Backend git: **argv list sempre, shell string nunca**; ref de stash validado por `_STASH_REF_RE` (`^stash@\{[0-9]+\}$` — única forma aceita); op por ENUM.
- **NUNCA `--force`** em nada. `drop` é destrutivo → confirm inline em 2 passos na UI (padrão `confirmDiscard`).
- Rotas FastAPI de git são `def` (não `async def`) → threadpool; `Depends(require_auth)` em toda rota nova; body com `_StrictBody`.
- Falha aparece, não some: apply/pop que conflitam voltam 409 com o stderr do git (o stash fica na lista nesse caso — o usuário resolve na sessão ou dá drop).
- **Duas views SEMPRE**: a view de stash entra no `GitSheet` (mobile) E no `GitPanel` (desktop), e a verificação manual testa as duas. Mobile = push-view por enum `GitView`; desktop = 3 zonas por seleção.
- UI em pt-BR; código/comentários/identificadores seguem o estilo do arquivo. Match de indentação/estilo — sem formatter.
- Diff grande: highlight via `import('../lib/highlight')` dinâmico.
- Gate de tipos do front: `npm --prefix frontend run check`. Gate do backend: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`.
- Commits frequentes, conventional commits, stage por path explícito (nunca `git add -A`).

## O que já existe (não recriar)

Ação allowlist `stash` (`stash push --include-untracked` — continua sendo o "guardar tudo") e `stash-pop` (fica na allowlist, mas perde o botão da toolbar — o pop seguro é por entrada, na view); `DiffView` (renderiza o diff do stash); pipeline Shiki por import dinâmico.

## Non-goals

Stash de subset de arquivos (`stash push -- <paths>`), `stash branch`, ver/aplicar a parte UNTRACKED do stash no diff (`stash show -p` não mostra untracked — limitação conhecida, o diff cobre as mudanças tracked).

---

### Task 1: Backend — stash_list + stash_show + stash_op

**Files:**
- Modify: `backend/app/git_ops.py` (novas funções após `push`, antes do `__main__`)
- Modify: `backend/app/api.py` (import linha 41-42; rotas novas após as de git existentes)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_run`, `GitError` (existentes)
- Produces:
  - `stash_list(cwd: str) -> list[dict]` → `[{"ref": "stash@{0}", "short": str, "rel": str, "subject": str}]`
  - `stash_show(cwd: str, ref: str) -> dict` → `{"ref": str, "diff": str}`
  - `stash_op(cwd: str, op: str, ref: str) -> dict` — `op ∈ {"apply","pop","drop"}`
  - `_STASH_REF_RE = re.compile(r"^stash@\{[0-9]+\}$")`
  - Rotas: `GET /git/stash`, `GET /git/stash/diff?ref=`, `POST /git/stash` (body `{op, ref}`)

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_stash_list_e_show(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("v2\n")
    assert git_ops.git_action(d, "stash")["ok"] is True   # a ação real do app (--include-untracked)
    lst = git_ops.stash_list(d)
    assert len(lst) == 1
    assert lst[0]["ref"] == "stash@{0}" and lst[0]["short"] and lst[0]["rel"]
    assert "WIP" in lst[0]["subject"] or "main" in lst[0]["subject"]
    assert "+v2" in git_ops.stash_show(d, "stash@{0}")["diff"]


def test_stash_apply_mantem_e_drop_remove(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("v2\n")
    git_ops.git_action(d, "stash")
    assert git_ops.stash_op(d, "apply", "stash@{0}")["ok"]
    assert f.read_text() == "v2\n"
    assert len(git_ops.stash_list(d)) == 1                # apply NÃO remove
    f.write_text("linha original\n")                      # limpa a tree antes do drop
    assert git_ops.stash_op(d, "drop", "stash@{0}")["ok"]
    assert git_ops.stash_list(d) == []


def test_stash_pop_reaplica_e_remove(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("v2\n")
    git_ops.git_action(d, "stash")
    assert git_ops.stash_op(d, "pop", "stash@{0}")["ok"]
    assert f.read_text() == "v2\n" and git_ops.stash_list(d) == []


@pytest.mark.parametrize("bad", ["stash@{0}; rm -rf /", "--all", "HEAD", "stash@{abc}"])
def test_stash_ref_invalido(tmp_path, bad):
    with pytest.raises(GitError) as e:
        git_ops.stash_op(_repo(tmp_path), "drop", bad)
    assert e.value.status == 400


def test_stash_op_invalida(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.stash_op(_repo(tmp_path), "explode", "stash@{0}")   # op fora do enum
    assert e.value.status == 400


def test_stash_show_ref_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.stash_show(_repo(tmp_path), "0; echo pwned")
    assert e.value.status == 400
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k stash -v`
Expected: FAIL nos 6 novos (`AttributeError` nas funções); o `test_stash_and_pop_roundtrip` ANTIGO segue passando

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, após `push` (antes do `if __name__`):

```python
_STASH_REF_RE = re.compile(r"^stash@\{[0-9]+\}$")
_STASH_OPS = {"apply", "pop", "drop"}


def stash_list(cwd: str) -> list[dict]:
    """git stash list estruturado. %gd=ref (stash@{N}), %h=hash curto, %cr=data relativa,
    %gs=assunto do stash. Vazio -> []."""
    p = _run(cwd, "stash", "list", "--format=%gd%x1f%h%x1f%cr%x1f%gs")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "stash list falhou").strip())
    out = []
    for line in p.stdout.splitlines():
        f = line.split("\x1f")
        if len(f) == 4:
            out.append({"ref": f[0], "short": f[1], "rel": f[2], "subject": f[3]})
    return out


def _validate_stash_ref(ref: str) -> None:
    if not _STASH_REF_RE.match(ref):
        raise GitError(400, "ref de stash invalido")


def stash_show(cwd: str, ref: str) -> dict:
    """Diff do stash (stash show -p) — pra revisar ANTES de aplicar, coisa que o botão cego
    'pop' não deixava fazer. Cobre as mudanças TRACKED (a parte untracked não entra no diff)."""
    _validate_stash_ref(ref)
    p = _run(cwd, "stash", "show", "-p", ref)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "stash show falhou").strip())
    return {"ref": ref, "diff": p.stdout}


def stash_op(cwd: str, op: str, ref: str) -> dict:
    """apply (mantém na lista) / pop (remove se aplicar limpo) / drop (descarta — DESTRUTIVO,
    confirm na UI). Conflito no apply/pop: o stash FICA na lista e o stderr vai pro usuário
    (falha aparece)."""
    if op not in _STASH_OPS:
        raise GitError(400, "op invalida")
    _validate_stash_ref(ref)
    p = _run(cwd, "stash", op, ref)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or p.stdout or f"stash {op} falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}
```

Em `backend/app/api.py`:

1. Import (linha 41-42) — acrescentar `stash_list`, `stash_show`, `stash_op`.
2. Rotas novas (após as rotas git existentes):

```python
class GitStashBody(_StrictBody):
    op: Literal["apply", "pop", "drop"]
    ref: str   # validado em git_ops por _STASH_REF_RE (só "stash@{N}")


@app.get("/api/sessions/{name}/git/stash", dependencies=[Depends(require_auth)])
def git_stash_list(name: str):
    try:
        return {"stashes": stash_list(_session_cwd(name))}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/stash/diff", dependencies=[Depends(require_auth)])
def git_stash_diff(name: str, ref: str):
    try:
        return stash_show(_session_cwd(name), ref)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/stash", dependencies=[Depends(require_auth)])
def git_stash_op(name: str, body: GitStashBody):
    try:
        return stash_op(_session_cwd(name), body.op, body.ref)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** (+ suíte inteira do git e self-check)

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): stash manager — list/show/apply/pop/drop"
```

---

### Task 2: Front — StashList view (aposenta o pop cego)

**Files:**
- Create: `frontend/src/components/git/StashList.svelte`
- Modify: `frontend/src/lib/api.ts` (`StashEntry`, `getStashList`, `getStashDiff`, `gitStashOp`)
- Modify: `frontend/src/lib/gitStore.svelte.ts` (`stashList`, `loadStash`, `stashApply/Pop/Drop`)
- Modify: `frontend/src/components/git/GitToolbar.svelte` (`stash`/`pop` saem; entra botão `stash` que abre a view)
- Modify: `frontend/src/components/GitSheet.svelte` (view `stash` + `openStashDiff`)
- Modify: `frontend/src/components/GitPanel.svelte` (stash na zona direita)

**Interfaces:**
- Consumes: rotas da Task 1 + ação `stash` existente (o "guardar tudo")
- Produces:
  - api.ts: `StashEntry` (`{ref, short, rel, subject}`), `StashOp = 'apply' | 'pop' | 'drop'`, `getStashList(name)`, `getStashDiff(name, ref)`, `gitStashOp(name, op, ref)`
  - gitStore: estado `stashList: StashEntry[]`; `loadStash()`, `stashApply(ref)`, `stashPop(ref)`, `stashDrop(ref)`
  - `StashList.svelte` props: `{ git: GitStore, onShowDiff: (ref: string) => void }`
  - `GitToolbar` props viram `{ git, onLog, onStash }`

- [ ] **Step 1: Clients (api.ts)**

Acrescentar após `gitPush`:

```typescript
// Uma entrada do stash list estruturado (ref "stash@{N}" + assunto + data relativa).
export interface StashEntry {
  ref: string;
  short: string;
  rel: string;
  subject: string;
}

export function getStashList(name: string): Promise<{ stashes: StashEntry[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/stash`);
}

export function getStashDiff(name: string, ref: string): Promise<{ ref: string; diff: string }> {
  const q = new URLSearchParams({ ref });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/stash/diff?${q}`);
}

export type StashOp = 'apply' | 'pop' | 'drop';

export function gitStashOp(name: string, op: StashOp, ref: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/stash`, {
    method: 'POST',
    body: JSON.stringify({ op, ref }),
  });
}
```

- [ ] **Step 2: Store (gitStore.svelte.ts)**

Importar `getStashList, gitStashOp, type StashEntry, type StashOp` de `./api`. Estado e métodos novos:

```typescript
  let stashList = $state<StashEntry[]>([]);
```

```typescript
  async function loadStash() {
    try { stashList = (await getStashList(sessionName)).stashes; }
    catch (e) { error = cleanErr(e); }
  }
  async function stashOp(ref: string, op: StashOp) {
    if (busy) return false;
    busy = `${op} ${ref}`; error = ''; output = '';
    try { const r = await gitStashOp(sessionName, op, ref); output = r.output || 'ok'; await refresh(); await loadStash(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function stashApply(ref: string) { return stashOp(ref, 'apply'); }
  async function stashPop(ref: string) { return stashOp(ref, 'pop'); }
  async function stashDrop(ref: string) { return stashOp(ref, 'drop'); }
```

No `return` do store, expor `stashList` (getter), `loadStash`, `stashApply`, `stashPop`, `stashDrop`.

- [ ] **Step 3: StashList.svelte (novo)**

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    onShowDiff: (ref: string) => void;
  }
  let { git, onShowDiff }: Props = $props();

  let confirmDrop = $state('');   // ref aguardando confirmacao de drop (DESTRUTIVO)

  async function guardarTudo() {
    await git.runAction('stash');   // a ação allowlist de sempre (push --include-untracked)
    await git.loadStash();
  }
</script>

<div class="sl">
  <button class="sl-push" disabled={!!git.busy} onclick={guardarTudo}
    title="guarda TODAS as mudanças, inclusive untracked">guardar tudo (stash push)</button>
  {#each git.stashList as s (s.ref)}
    <div class="sl-row">
      <div class="sl-info">
        <span class="sl-ref">{s.ref}</span>
        <span class="sl-when">{s.rel}</span>
        <span class="sl-sub">{s.subject}</span>
      </div>
      <div class="sl-acts">
        <button class="git-mini" disabled={!!git.busy} onclick={() => onShowDiff(s.ref)}>ver diff</button>
        <button class="git-mini" disabled={!!git.busy} onclick={() => git.stashApply(s.ref)}
          title="aplica e mantém na lista">aplicar</button>
        <button class="git-mini" disabled={!!git.busy} onclick={() => git.stashPop(s.ref)}
          title="aplica e remove da lista">pop</button>
        {#if confirmDrop === s.ref}
          <button class="git-mini danger" disabled={!!git.busy}
            onclick={async () => { if (await git.stashDrop(s.ref)) confirmDrop = ''; }}>confirmar drop</button>
          <button class="git-mini" onclick={() => (confirmDrop = '')}>não</button>
        {:else}
          <button class="git-mini danger" disabled={!!git.busy} onclick={() => (confirmDrop = s.ref)}
            title="descarta o stash (não tem volta)">drop</button>
        {/if}
      </div>
    </div>
  {:else}
    <p class="git-muted">nenhum stash</p>
  {/each}
</div>

<style>
  .sl { display: flex; flex-direction: column; gap: var(--space-2); }
  .sl-push { padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .sl-push:disabled { opacity: 0.5; cursor: default; }
  .sl-row { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2);
    border-radius: var(--radius-md); border: 1px solid var(--border-subtle); }
  .sl-info { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  .sl-ref { flex: 0 0 auto; font-family: var(--font-mono); font-size: var(--text-xs); color: var(--accent); }
  .sl-when { flex: 0 0 auto; font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; }
  .sl-sub { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: var(--text-sm); color: var(--text-primary); }
  .sl-acts { display: flex; gap: var(--space-2); flex-wrap: wrap; }

  /* Replicas locais dos padroes (Svelte escopa CSS por componente) — ver ChangedFiles. */
  .git-mini { flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer; }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
```

- [ ] **Step 4: GitToolbar — sai o pop cego**

Props e botões: remove os botões `stash` (runAction) e `pop` (runAction stash-pop); entra um `stash` que abre a view. `stash-pop` permanece na allowlist do backend, sem botão (o pop seguro é por entrada, na view):

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    onLog: () => void;
    onStash: () => void;
  }
  let { git, onLog, onStash }: Props = $props();
</script>

<div class="git-actions">
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('status')}>status</button>
  <button class="git-act" disabled={!!git.busy} onclick={onLog} title="últimos commits (git log)">log</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('fetch')}>fetch</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('pull')}>pull</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.doPush()} title="envia os commits (git push)">push</button>
  <button class="git-act" disabled={!!git.busy} onclick={onStash} title="lista, revisa e aplica stashes">stash</button>
</div>
```

(O `<style>` fica idêntico. Se o plano 2 já rodou, o bloco `{#if git.pendingAbort}…{/if}` dele permanece ao final da `.git-actions`.)

- [ ] **Step 5: GitSheet (mobile) — view `stash` + diff do stash**

Enum: `type GitView = 'list' | 'log' | 'diff' | 'commit' | 'commitbox' | 'stash';` (se o plano 3 já rodou, o enum terá também `'blame' | 'filelog'` — acrescentar `'stash'` ao que existir).

State e funções novas:

```typescript
  let stashDiffOpen = $state(false);   // o diff aberto é de um stash (pro voltar cair na view 'stash')
```

```typescript
  import StashList from './git/StashList.svelte';
  import { getFileDiff, getCommitFileDiff, getStashDiff, type GitCommit } from '../lib/api';

  function openStash() {
    view = 'stash';
    git.loadStash();
  }

  // Diff de UM stash (revisar antes de aplicar) — reusa a view 'diff' e o highlight Shiki.
  async function openStashDiff(ref: string) {
    if (git.busy) return;
    diffSha = '';
    stashDiffOpen = true;
    diffPath = ref;
    diffRows = [];
    diffLoading = true;
    git.error = '';
    git.busy = ref;
    view = 'diff';
    try {
      const { diff } = await getStashDiff(sessionName, ref);
      const { highlightDiff } = await import('../lib/highlight');
      diffRows = await highlightDiff(diff, ref);
    } catch (e) {
      git.error = cleanErr(e);
      diffPath = '';
      stashDiffOpen = false;
      view = 'stash';
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }
```

`openDiff` (working tree) e `openCommitFileDiff` ganham `stashDiffOpen = false;` no início (limpam a marca). O voltar da view `diff` vira:

```svelte
<button class="git-back" onclick={() => (view = diffSha ? 'commit' : stashDiffOpen ? 'stash' : 'list')} aria-label="Voltar">‹ voltar</button>
```

Bloco de render novo (antes do `{:else}` final):

```svelte
  {:else if view === 'stash'}
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = 'list')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">stash</span>
      </div>
      <StashList {git} onShowDiff={openStashDiff} />
    </div>
```

E o `<GitToolbar>` da view `list` passa `onStash={openStash}`.

- [ ] **Step 6: GitPanel (desktop) — stash na zona direita**

State novo: `let stashOpen = $state(false);` + os do diff do stash:

```typescript
  let stashDiffRows = $state<DiffRow[]>([]);
  let stashDiffLoading = $state(false);
  let stashDiffPath = $state('');
```

```typescript
  import StashList from './git/StashList.svelte';
  import { getStashDiff } from '../lib/api';

  function openStash() {
    stashOpen = true;
    git.loadStash();
  }

  async function openStashDiff(ref: string) {
    if (git.busy) return;
    stashDiffPath = ref;
    stashDiffRows = [];
    stashDiffLoading = true;
    git.busy = ref;
    git.error = '';
    try {
      const { diff } = await getStashDiff(git.sessionName, ref);
      const { highlightDiff } = await import('../lib/highlight');
      stashDiffRows = await highlightDiff(diff, ref);
    } catch (e) {
      git.error = cleanErr(e);
      stashDiffPath = '';
    } finally {
      stashDiffLoading = false;
      git.busy = '';
    }
  }
```

`openWtDiff`, `openCommitDiff` e o `onSelect` do `<CommitList>` ganham `stashOpen = false;` no início (selecionar outra coisa fecha a view de stash). Zona direita — ramo novo imediatamente ANTES do branch `selected === null` (no arquivo atual ele é `{#if selected === null}` e vira `{:else if}`; se o plano 3 já rodou, ele já é `{:else if selected === null}` e o ramo novo entra como `{:else if stashOpen}`):

```svelte
      {#if stashOpen}
        {#if stashDiffPath}
          <button class="git-back" onclick={() => (stashDiffPath = '')} aria-label="Voltar">‹ voltar</button>
          <DiffView path={stashDiffPath} rows={stashDiffRows} loading={stashDiffLoading} />
        {:else}
          <button class="git-back" onclick={() => (stashOpen = false)} aria-label="Voltar">‹ voltar</button>
          <StashList {git} onShowDiff={openStashDiff} />
        {/if}
      {:else if selected === null}
        <!-- …resto do encadeamento IDÊNTICO ao atual… -->
```

`<GitToolbar {git} onLog={git.openLog} onStash={openStash} />`. Réplica local de `.git-back` no `<style>` do GitPanel (copiar de `GitSheet.svelte`).

**Higiene cruzada (aplicar só se o plano 3 rodou antes deste):** um flag de view da zona direita sempre desliga os outros, nos DOIS sentidos — `openStash` ganha `blameData = null; fileLogData = null;` no início, e os handlers `openBlame`/`openFileLog` do plano 3 ganham `stashOpen = false;`. (Standalone não tem com quem conflitar; a nota é o contrato de coabitação.)

- [ ] **Step 7: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix frontend run check`
Expected: 0 erros

Manual, mobile E desktop, num repo de brinquedo:
1. "guardar tudo" 2× com mudanças distintas → 2 entradas na lista (stash@{0}, stash@{1}).
2. "ver diff" de uma entrada → diff Shiki; voltar cai na lista de stash.
3. "aplicar" → mudança volta pra tree e a entrada FICA; "pop" na outra → volta e SAI da lista.
4. "drop" com confirm de 2 passos → entrada some.
5. Toolbar NÃO tem mais o botão `pop`; o `stash` dela abre a view.
6. Apply que conflita (aplicar 2× seguidas sem limpar) → erro do git aparece e a entrada fica na lista.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/git/StashList.svelte frontend/src/components/git/GitToolbar.svelte frontend/src/components/GitSheet.svelte frontend/src/components/GitPanel.svelte frontend/src/lib/api.ts frontend/src/lib/gitStore.svelte.ts
git commit -m "feat(git): StashList com diff/apply/pop/drop — fim do pop cego"
```

---

### Task 3: Gate final + docs

**Files:**
- Modify: `docs/USAGE.md` (seção `### Git`)

- [ ] **Step 1: Suíte completa backend**

Run: `cd backend && uv run pytest -v`
Expected: PASS

- [ ] **Step 2: Gate front completo**

Run: `npm --prefix frontend run check && npm --prefix frontend run build`
Expected: 0 erros + build ok

- [ ] **Step 3: Docs**

Em `docs/USAGE.md`, na seção `### Git`, acrescentar:

```markdown
- **Stash:** o botão **stash** abre o gerenciador: **guardar tudo** (inclui untracked), lista os
  stashes com assunto e data, **ver diff** antes de aplicar, **aplicar** (mantém na lista), **pop**
  (aplica e remove) e **drop** (descarta, com confirmação dupla). Não existe mais "pop cego" na
  barra de ações — o pop é sempre de uma entrada escolhida e revisada.
```

- [ ] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: gerenciador de stash"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** list/show/apply/pop/drop (Task 1) + view com guardar-tudo, diff pré-aplicar e drop confirmado (Task 2) + aposentadoria do pop cego (Task 2, Steps 4-6). Cada item verificado manualmente nas duas views.
- **Consistência de tipos:** `stash_list/stash_show/stash_op(cwd, ...)` ↔ `StashEntry`/`getStashList`/`getStashDiff`/`gitStashOp`; `_STASH_REF_RE` aceita só `stash@{N}` — mesmo formato que `stash_list` emite em `ref` (round-trip garantido: o front só manda refs que vieram do backend).
- **Sem placeholders:** backend/testes/store/componente com código completo; "IDÊNTICO ao atual" só no resto do encadeamento da zona direita (markup existente que se mantém).
- **Correções em relação ao plano monolítico:** (1) testes nos helpers reais (`_repo_with_file`), com apply/pop separados (o teste antigo do monolítico fazia apply→pop no MESMO stash, que conflita — apply deixa a tree suja); (2) fiação desktop explícita (zona direita com `stashOpen` + diff do stash na mesma zona); (3) voltar do diff de stash definido (`stashDiffOpen`), o plano antigo não dizia pra onde ia.
- **Decisões registradas:** ação allowlist `stash-pop` permanece no backend sem botão (pop seguro é por entrada); diff do stash cobre só tracked (limitação do `stash show -p`, documentada); drop é o único confirm de 2 passos (apply/pop falham limpo em conflito, sem risco).

## Loop-readiness

- `check_cmd` por fase: Task 1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 2 → `npm --prefix frontend run check`; Task 3 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
