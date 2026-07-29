# Git branch/tag manager (estilo TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fechar a gestão de refs do painel git: deletar branch local (com `-d` e o `-D` confirmado quando o git recusa), listar/criar/deletar tags e dar push de tag — o "Branch/Tag" do TortoiseGit.

**Architecture:** Backend: `delete_branch` (valida contra a lista real, rejeita a atual), `list_tags`, `push_tag` (output scrubado) e `delete_tag` em `git_ops.py`, expostos como rotas `def` em `api.py`. Front: clients em `api.ts`, estado+métodos no `gitStore`, menu `⋯` por branch no `BranchList` (com a opção `-D` surgindo só quando o git recusa o `-d`), e componente novo `TagBox.svelte` aberto pela toolbar nas DUAS views (push-view `tags` no `GitSheet`; zona direita no `GitPanel`). Este plano é a fatia 5 de 5 do antigo plano monolítico (removido); os outros: commit dialog, log hub, blame/histórico, stash.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repos git temporários (incluindo repo com remote); Svelte 5 (runes) + TypeScript.

## Pré-requisitos

**Plano 2 (`2026-07-29-git-log-hub.md`) executado** — o TagBox cria tags via `create_tag` (backend) e `git.createTag` (store), ambos produzidos lá (que por sua vez usa `_validate_new_ref` do plano 1). Se o plano 2 ainda não rodou, pare e rode ele antes.

## Referências

**TortoiseGit (UX a replicar):**
- Branch/Tag: https://tortoisegit.org/docs/tortoisegit/tgit-dug-branchtag.html

**Git (flags usadas):**
- git-branch(1) `-d`/`-D`, git-tag(1) `--sort=-creatordate` / `-d`, git-push(1) `origin <tag>` — https://git-scm.com/docs

**Internas (código existente a estender — LER antes de codar cada task):**
- `backend/app/git_ops.py` — `list_branches` (linha 137), `push` (426, padrão `_scrub` + checagem de `origin`), `create_tag` (do plano 2)
- `backend/tests/test_git_ops.py` — helpers `_repo(tmp_path)` e `_with_remote(tmp_path)` (repo local + 'origin'; NÃO existem fixtures `init_repo`/`head_sha`)
- `backend/app/api.py` — rotas git (1855-1937), import de git_ops (41-42), `_StrictBody` (449)
- `frontend/src/lib/api.ts` — `gitPush` (826-828, padrão de client POST sem body)
- `frontend/src/lib/gitStore.svelte.ts` — `pick` (padrão busy/error), `createTag` (do plano 2)
- `frontend/src/components/git/BranchList.svelte` — linhas de branch (`git-branch`), a atual marcada
- `frontend/src/components/git/GitToolbar.svelte` — botões (o `tags` entra após o `push`)
- `frontend/src/components/GitSheet.svelte` — enum `GitView`; `frontend/src/components/GitPanel.svelte` — zona direita
- `frontend/src/components/git/ChangedFiles.svelte` — padrão `.git-mini` + confirm inline

## Global Constraints

- Backend git: **argv list sempre, shell string nunca**; nomes de branch/tag validados contra as listas REAIS (`list_branches`/`list_tags`) antes de virar argv; output de push passa por `_scrub` (pode conter URL do remote com credencial).
- **NUNCA `--force`** em push. `branch -D` só dispara por clique explícito do usuário DEPOIS que o `-d` falhou (o git é a primeira guarda; o clique é a segunda).
- Rotas FastAPI de git são `def` (não `async def`) → threadpool; `Depends(require_auth)` em toda rota nova; body com `_StrictBody`.
- Falha aparece, não some: erro do git volta como `GitError` com o stderr (`409`/`400`).
- **Duas views SEMPRE**: o menu de branch e o TagBox entram no `GitSheet` (mobile) E no `GitPanel` (desktop), e a verificação manual testa as duas. Mobile = push-view por enum `GitView`; desktop = 3 zonas por seleção.
- UI em pt-BR; código/comentários/identificadores seguem o estilo do arquivo. Match de indentação/estilo — sem formatter.
- Gate de tipos do front: `npm --prefix frontend run check`. Gate do backend: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`.
- Commits frequentes, conventional commits, stage por path explícito (nunca `git add -A`).

## O que já existe (não recriar)

`list_branches`/`switch_branch` (com DWIM de remota); `BranchList` com a atual no topo; `create_tag` + `git.createTag` (plano 2 — TagBox consome); padrão `_scrub` no `push`.

## Non-goals

Deletar branch REMOTA, deletar tag remota (`push origin :tag`), renomear branch, merge/rebase pela UI, ver tags remotas. A tag deletada é só a LOCAL (a remota fica — decisão separada, fora do app).

---

### Task 1: Backend — delete_branch + list_tags + push_tag + delete_tag

**Files:**
- Modify: `backend/app/git_ops.py` (novas funções após `push`, antes do `__main__`)
- Modify: `backend/app/api.py` (import linha 41-42; rotas novas após as de git existentes)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `list_branches`, `_scrub`, `_run`, `GitError` (existentes); `create_tag` (**do plano 2**, só nos testes)
- Produces:
  - `delete_branch(cwd: str, name: str, force: bool = False) -> dict`
  - `list_tags(cwd: str) -> list[str]`
  - `push_tag(cwd: str, name: str) -> dict`
  - `delete_tag(cwd: str, name: str) -> dict`
  - Rotas: `POST /git/branch/delete`, `GET /git/tags`, `POST /git/tag/push`, `POST /git/tag/delete`

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_delete_branch_mergeada_e_force(tmp_path):
    d = _repo(tmp_path)                          # "feature" aponta pro mesmo commit de main -> mergeada
    assert git_ops.delete_branch(d, "feature")["ok"]
    assert "feature" not in git_ops.list_branches(d)["branches"]
    # branch NÃO mergeada: -d falha (409), -D passa
    git_ops._run(d, "switch", "-q", "-c", "wip")
    (tmp_path / "w.txt").write_text("W\n")
    git_ops.commit(d, "wip", ["w.txt"])
    git_ops._run(d, "switch", "-q", "main")
    with pytest.raises(GitError) as e:
        git_ops.delete_branch(d, "wip")
    assert e.value.status == 409
    assert git_ops.delete_branch(d, "wip", force=True)["ok"]


def test_delete_branch_atual_rejeitada(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.delete_branch(_repo(tmp_path), "main")
    assert e.value.status == 400


def test_delete_branch_inexistente(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.delete_branch(_repo(tmp_path), "nope; rm -rf /")
    assert e.value.status == 400


def test_push_tag(tmp_path):
    d = _with_remote(tmp_path)                   # repo local + 'origin' em tmp_path/"remote"
    rd = str(tmp_path / "remote")
    git_ops.create_tag(d, "v9", None)            # create_tag vem do plano 2
    r = git_ops.push_tag(d, "v9")
    assert r["ok"]
    assert "v9" in git_ops._run(rd, "tag").stdout


def test_push_tag_inexistente(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.push_tag(_with_remote(tmp_path), "v0")
    assert e.value.status == 400


def test_list_e_delete_tag(tmp_path):
    d = _repo(tmp_path)
    git_ops.create_tag(d, "v1", None, message="anotada")
    git_ops.create_tag(d, "v2", None)
    assert set(git_ops.list_tags(d)) == {"v1", "v2"}
    assert git_ops.delete_tag(d, "v1")["ok"]
    assert git_ops.list_tags(d) == ["v2"]
    with pytest.raises(GitError) as e:
        git_ops.delete_tag(d, "v1")              # já não existe -> 400
    assert e.value.status == 400
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "delete_branch or push_tag or tag" -v`
Expected: FAIL (`AttributeError` nas funções novas)

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, após `push` (antes do `if __name__`):

```python
def delete_branch(cwd: str, name: str, force: bool = False) -> dict:
    """Apaga branch local. -d (default) so apaga se mergeada; force=True usa -D (DESTRUTIVO —
    a UI só oferece DEPOIS que o -d falhou, com confirm explícito). A branch ATUAL é rejeitada
    (400). Nome validado contra a lista real (sem injeção/flag-like)."""
    info = list_branches(cwd)
    if name == info["current"]:
        raise GitError(400, "nao da pra apagar a branch atual")
    if name not in info["branches"]:
        raise GitError(400, "branch inexistente")
    p = _run(cwd, "branch", "-D" if force else "-d", name)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "apagar branch falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def list_tags(cwd: str) -> list[str]:
    """Tags locais, mais recentes primeiro (creatordate)."""
    p = _run(cwd, "tag", "--sort=-creatordate", "--format=%(refname:short)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git tag falhou").strip())
    return [t.strip() for t in p.stdout.splitlines() if t.strip()]


def push_tag(cwd: str, name: str) -> dict:
    """git push origin <tag>. Output SEMPRE scrubado (pode conter URL do remote com credencial).
    Sem 'origin' -> 409 claro (mesmo padrão do push de branch)."""
    if name not in list_tags(cwd):
        raise GitError(400, "tag inexistente")
    rem = _run(cwd, "remote")
    if "origin" not in rem.stdout.split():
        raise GitError(409, "sem remote 'origin' — configure um remote antes")
    p = _run(cwd, "push", "origin", name)
    if p.returncode != 0:
        raise GitError(409, _scrub((p.stderr or p.stdout or "push da tag falhou").strip()))
    return {"ok": True, "output": _scrub((p.stdout + p.stderr).strip())}


def delete_tag(cwd: str, name: str) -> dict:
    """Apaga a tag LOCAL (a remota fica — apagar remoto é outra decisão, fora do app)."""
    if name not in list_tags(cwd):
        raise GitError(400, "tag inexistente")
    p = _run(cwd, "tag", "-d", name)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "apagar tag falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}
```

Em `backend/app/api.py`:

1. Import (linha 41-42) — acrescentar `delete_branch`, `list_tags`, `push_tag`, `delete_tag`.
2. Rotas novas (após as rotas git existentes):

```python
class GitBranchDeleteBody(_StrictBody):
    name: str
    force: bool = False


class GitTagNameBody(_StrictBody):
    name: str   # validado em git_ops contra a lista real de tags


@app.post("/api/sessions/{name}/git/branch/delete", dependencies=[Depends(require_auth)])
def git_branch_delete(name: str, body: GitBranchDeleteBody):
    try:
        return delete_branch(_session_cwd(name), body.name, body.force)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/tags", dependencies=[Depends(require_auth)])
def git_tags(name: str):
    try:
        return {"tags": list_tags(_session_cwd(name))}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/tag/push", dependencies=[Depends(require_auth)])
def git_tag_push(name: str, body: GitTagNameBody):
    try:
        return push_tag(_session_cwd(name), body.name)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/tag/delete", dependencies=[Depends(require_auth)])
def git_tag_delete(name: str, body: GitTagNameBody):
    try:
        return delete_tag(_session_cwd(name), body.name)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** (+ suíte inteira do git e self-check)

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): delete branch (com -D confirmado) + list/push/delete tag"
```

---

### Task 2: Front — menu ⋯ por branch + TagBox

**Files:**
- Create: `frontend/src/components/git/TagBox.svelte`
- Modify: `frontend/src/lib/api.ts` (`gitDeleteBranch`, `getTags`, `gitPushTag`, `gitDeleteTag`)
- Modify: `frontend/src/lib/gitStore.svelte.ts` (`tags`, `loadTags`, `deleteBranch`, `pushTag`, `deleteTag`)
- Modify: `frontend/src/components/git/BranchList.svelte` (⋯ por branch não-atual)
- Modify: `frontend/src/components/git/GitToolbar.svelte` (botão `tags`)
- Modify: `frontend/src/components/GitSheet.svelte` (view `tags`)
- Modify: `frontend/src/components/GitPanel.svelte` (tags na zona direita)

**Interfaces:**
- Consumes: rotas da Task 1 + `createTag` do store (**do plano 2**)
- Produces:
  - api.ts: `gitDeleteBranch(name, branch, force?)`, `getTags(name)`, `gitPushTag(name, tag)`, `gitDeleteTag(name, tag)`
  - gitStore: estado `tags: string[]`; `loadTags()`, `deleteBranch(name, force?)`, `pushTag(name)`, `deleteTag(name)`
  - `TagBox.svelte` props: `{ git: GitStore }`
  - `GitToolbar` ganha prop `onTags: () => void`

- [ ] **Step 1: Clients (api.ts)**

Acrescentar após `gitPush`:

```typescript
export function gitDeleteBranch(name: string, branch: string, force = false): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/branch/delete`, {
    method: 'POST',
    body: JSON.stringify({ name: branch, force }),
  });
}

export function getTags(name: string): Promise<{ tags: string[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/tags`);
}

export function gitPushTag(name: string, tag: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/tag/push`, {
    method: 'POST',
    body: JSON.stringify({ name: tag }),
  });
}

// Apaga só a tag LOCAL (a remota fica).
export function gitDeleteTag(name: string, tag: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/tag/delete`, {
    method: 'POST',
    body: JSON.stringify({ name: tag }),
  });
}
```

- [ ] **Step 2: Store (gitStore.svelte.ts)**

Importar `gitDeleteBranch, getTags, gitPushTag, gitDeleteTag` de `./api`. Estado e métodos:

```typescript
  let tags = $state<string[]>([]);
```

```typescript
  async function loadTags() {
    try { tags = (await getTags(sessionName)).tags; }
    catch (e) { error = cleanErr(e); }
  }
  async function deleteBranch(name: string, force = false) {
    if (busy) return false;
    busy = name; error = ''; output = '';
    try { const r = await gitDeleteBranch(sessionName, name, force); output = r.output || 'branch apagada'; await refresh(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function pushTag(name: string) {
    if (busy) return false;
    busy = name; error = ''; output = '';
    try { const r = await gitPushTag(sessionName, name); output = r.output || 'tag enviada'; return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function deleteTag(name: string) {
    if (busy) return false;
    busy = name; error = ''; output = '';
    try { const r = await gitDeleteTag(sessionName, name); output = r.output || 'tag apagada'; await loadTags(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
```

No `return` do store, expor `tags` (getter), `loadTags`, `deleteBranch`, `pushTag`, `deleteTag`.

- [ ] **Step 3: BranchList — ⋯ por branch**

States novos:

```typescript
  let branchMenu = $state('');     // branch com o mini-menu aberto
  let forceDelete = $state('');    // branch que o git recusou apagar (-d) e espera decisão do -D

  async function doDelete(b: string, force: boolean) {
    const ok = await git.deleteBranch(b, force);
    if (ok) { branchMenu = ''; forceDelete = ''; }
    else if (!force) forceDelete = b;   // 409 (não mergeada) -> oferece o -D com texto claro
  }
```

A linha de cada branch LOCAL (não vale pra remota nem pra atual) vira row com ⋯:

```svelte
  {#each localList as b (b)}
    <div class="git-branch-row">
      <button class="git-branch" class:current={b === git.current} disabled={!!git.busy} onclick={() => git.pick(b)}>
        <!-- …conteúdo da linha IDÊNTICO ao atual (dot + name + spin)… -->
      </button>
      {#if b !== git.current}
        {#if branchMenu === b}
          <button class="git-mini danger" disabled={!!git.busy} onclick={() => doDelete(b, false)}>deletar</button>
          <button class="git-mini" onclick={() => { branchMenu = ''; forceDelete = ''; }}>não</button>
        {:else}
          <button class="git-mini" aria-label="ações da branch" title="ações da branch" onclick={() => (branchMenu = b)}>⋯</button>
        {/if}
      {/if}
    </div>
    {#if forceDelete === b}
      <div class="git-branch-force">
        <span class="git-branch-force-txt">não mergeada na atual — deletar mesmo assim?</span>
        <button class="git-mini danger" disabled={!!git.busy} onclick={() => doDelete(b, true)}>deletar (-D)</button>
        <button class="git-mini" onclick={() => (forceDelete = '')}>não</button>
      </div>
    {/if}
  {/each}
```

CSS: `.git-branch-row { display: flex; align-items: center; gap: var(--space-1); }`, `.git-branch` ganha `flex: 1; min-width: 0;` (perde o `width: 100%`), `.git-branch-force { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2); }`, `.git-branch-force-txt { flex: 1; font-size: var(--text-xs); color: var(--error); }` + réplica `.git-mini`/`.git-mini.danger` (padrão do projeto, ver ChangedFiles).

Nota de UX: o primeiro clique em "deletar" usa `-d` — seguro por construção (o git só apaga mergeada), então não pede confirm; o `-D` é o único que ganha texto de confirmação explícito.

- [ ] **Step 4: TagBox.svelte (novo)**

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props { git: GitStore; }
  let { git }: Props = $props();

  let newTag = $state('');
  let newTagMsg = $state('');
  let confirmDelete = $state('');   // tag aguardando confirmacao de delete

  async function create() {
    // createTag(name, sha?, message?) — sha omitido = HEAD (vem do plano 2).
    if (await git.createTag(newTag.trim(), undefined, newTagMsg.trim() || undefined)) {
      newTag = ''; newTagMsg = '';
      await git.loadTags();
    }
  }
</script>

<div class="tb">
  <p class="git-section">nova tag (no HEAD)</p>
  <input class="tb-input" bind:value={newTag} placeholder="nome da tag"
    autocapitalize="off" autocorrect="off" spellcheck="false" />
  <input class="tb-input" bind:value={newTagMsg} placeholder="mensagem (opcional — vira tag anotada)"
    autocapitalize="off" autocorrect="off" spellcheck="false" />
  <button class="tb-create" disabled={!newTag.trim() || !!git.busy} onclick={create}>criar tag</button>

  <p class="git-section">tags</p>
  {#each git.tags as t (t)}
    <div class="tb-row">
      <span class="tb-name">{t}</span>
      <button class="git-mini" disabled={!!git.busy} onclick={() => git.pushTag(t)} title="envia pro origin">push</button>
      {#if confirmDelete === t}
        <button class="git-mini danger" disabled={!!git.busy}
          onclick={async () => { if (await git.deleteTag(t)) confirmDelete = ''; }}>confirmar</button>
        <button class="git-mini" onclick={() => (confirmDelete = '')}>não</button>
      {:else}
        <button class="git-mini danger" disabled={!!git.busy} onclick={() => (confirmDelete = t)}
          title="apaga a tag local (a remota fica)">deletar</button>
      {/if}
    </div>
  {:else}
    <p class="git-muted">nenhuma tag</p>
  {/each}
</div>

<style>
  .tb { display: flex; flex-direction: column; gap: var(--space-2); }
  .tb-input { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .tb-create { padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .tb-create:disabled { opacity: 0.5; cursor: default; }
  .tb-row { display: flex; align-items: center; gap: var(--space-2); }
  .tb-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-secondary); }

  /* Replicas locais dos padroes (Svelte escopa CSS por componente) — ver ChangedFiles. */
  .git-section { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em; }
  .git-mini { flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer; }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
```

- [ ] **Step 5: GitToolbar + hosts (mobile E desktop)**

`GitToolbar.svelte`: prop nova `onTags: () => void` na interface, e o botão imediatamente APÓS o botão `push` (âncora estável — existe com ou sem os planos 3/4):

```svelte
  <button class="git-act" disabled={!!git.busy} onclick={onTags} title="listar, criar e enviar tags">tags</button>
```

`GitSheet.svelte` (mobile): enum ganha `'tags'` (acrescentar ao que existir — `'list' | 'log' | 'diff' | 'commit' | 'commitbox'`, mais `'stash'` se o plano 4 rodou, mais `'blame' | 'filelog'` se o 3 rodou). Função + bloco:

```typescript
  import TagBox from './git/TagBox.svelte';

  function openTags() {
    view = 'tags';
    git.loadTags();
  }
```

```svelte
  {:else if view === 'tags'}
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = 'list')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">tags</span>
      </div>
      <TagBox {git} />
    </div>
```

`<GitToolbar {git} onLog={openLog} onTags={openTags} … />` (manter as props que já existirem, ex.: `onStash` do plano 4).

`GitPanel.svelte` (desktop): state `let tagsOpen = $state(false);`, função `openTags` (`tagsOpen = true; git.loadTags();`), prop `onTags={openTags}` no `<GitToolbar>`, e ramo novo na zona direita imediatamente ANTES do `{:else if selected === null}` (âncora estável em todas as combinações de planos):

```svelte
      {#if tagsOpen}
        <button class="git-back" onclick={() => (tagsOpen = false)} aria-label="Voltar">‹ voltar</button>
        <TagBox {git} />
      {:else if selected === null}
        <!-- …resto do encadeamento IDÊNTICO ao atual… -->
```

`openWtDiff`, `openCommitDiff` e o `onSelect` do `<CommitList>` ganham `tagsOpen = false;` no início. Réplica local de `.git-back` no `<style>` do GitPanel se ainda não existir (copiar de `GitSheet.svelte`).

**Higiene cruzada (aplicar só se os planos 3 e/ou 4 rodaram antes deste):** um flag de view da zona direita sempre desliga os outros, nos DOIS sentidos — `openTags` ganha no início `blameData = null; fileLogData = null;` (plano 3) e/ou `stashOpen = false;` (plano 4); e os handlers `openBlame`/`openFileLog`/`openStash` deles ganham `tagsOpen = false;`. Se já houver ramos de outros planos no encadeamento, o ramo novo entra como `{:else if tagsOpen}`. (Standalone não tem com quem conflitar; a nota é o contrato de coabitação.)

- [ ] **Step 6: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix frontend run check`
Expected: 0 erros

Manual, mobile E desktop, num repo de brinquedo com remote:
1. ⋯ numa branch mergeada → "deletar" apaga direto; ⋯ numa branch com commit não-mergeado → o 409 vira a linha "não mergeada — deletar mesmo assim?" → "deletar (-D)" apaga; ⋯ na branch ATUAL não aparece.
2. tags: criar tag leve e anotada no HEAD → aparecem na lista (e no `refs` do grafo do log); push de uma → chega no remote; deletar com confirm → some.
3. Push de tag sem remote configurado → 409 claro aparece (testar num repo sem origin).
4. Toolbar tem `tags` em mobile e desktop; voltar da view fecha certo nas duas.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/git/TagBox.svelte frontend/src/components/git/BranchList.svelte frontend/src/components/git/GitToolbar.svelte frontend/src/components/GitSheet.svelte frontend/src/components/GitPanel.svelte frontend/src/lib/api.ts frontend/src/lib/gitStore.svelte.ts
git commit -m "feat(git): ações de branch (delete com -D confirmado) + TagBox"
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
- **Branches:** o **⋯** ao lado de uma branch (que não seja a atual) permite deletá-la. Se o git
  recusar por ela não estar mergeada, aparece a opção explícita **deletar (-D)**.
- **Tags:** o botão **tags** lista as tags locais e permite criar (leve ou anotada, sempre no HEAD —
  para tagar um commit antigo, use o menu ⋯ do commit no log), dar **push** de uma tag pro origin e
  **deletar** a tag local (com confirmação; a remota é preservada).
```

- [ ] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: gestão de branches e tags"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** delete branch com -d/-D (Task 1 + Task 2 Step 3), list/create/push/delete tag (Task 1 + TagBox, create via `create_tag` do plano 2). Cada item com verificação manual nas duas views.
- **Consistência de tipos:** `delete_branch(cwd, name, force=False)` ↔ `gitDeleteBranch(name, branch, force?)` ↔ `deleteBranch(name, force?)`; `list_tags(cwd) -> list[str]` ↔ `getTags(name) -> {tags: string[]}`; `push_tag/delete_tag(cwd, name)` ↔ `gitPushTag/gitDeleteTag(name, tag)`. `GitTagNameBody` é compartilhado por push e delete. `createTag(name, sha?, message?)` (plano 2) é chamado com `sha=undefined` → HEAD.
- **Sem placeholders:** backend/testes/store/componentes com código completo; "IDÊNTICO ao atual" só no conteúdo da linha de branch e no resto do encadeamento da zona direita (markup existente que se mantém).
- **Correções em relação ao plano monolítico:** (1) testes nos helpers reais (`_repo`/`_with_remote`); (2) `list_tags` validado por conjunto (a ordem de `--sort=-creatordate` com mesma data não é estável pra assert exato); (3) fiação desktop explícita (zona direita com `tagsOpen`); (4) higiene cruzada dos flags de view documentada como passo explícito (o monolítico ignorava a convivência das views na zona direita).
- **Decisões registradas:** `-d` sem confirm (o git é a guarda — só apaga mergeada), `-D` só aparece DEPOIS da recusa do git, com texto claro; delete de tag é só local (remota fica); push de tag scrubado como todo push; tag nova pelo TagBox é sempre no HEAD (tagar commit antigo = menu ⋯ do log, do plano 2).

## Loop-readiness

- `check_cmd` por fase: Task 1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 2 → `npm --prefix frontend run check`; Task 3 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
