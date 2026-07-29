# Git log como hub de ações por commit (estilo TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transformar o log do painel git no hub do TortoiseGit: menu de contexto em cada commit com diff unificado do commit inteiro, copiar hash/mensagem, criar branch/tag naquele commit, cherry-pick, revert e reset (soft/mixed/hard).

**Architecture:** Backend: funções novas em `git_ops.py` (`commit_diff`, `revert_commit`, `cherry_pick`, `reset_to`, `create_branch_at`, `create_tag`) + 2 ações de abort na allowlist `_ACTIONS`, expostas como rotas `def` em `api.py`. Front: clients em `api.ts`, métodos no `gitStore`, e um componente novo `CommitMenu.svelte` (overlay backdrop+card, usado pelas DUAS views) aberto a partir de um botão `⋯` por commit no `CommitList` e no `CommitDetail`. Este plano é a fatia 2 de 5 do antigo plano monolítico (removido); os outros: commit dialog, blame/histórico, stash, branch/tag.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repos git temporários; Svelte 5 (runes) + TypeScript; diff renderizado com o pipeline Shiki já existente (`lib/highlight.ts`, import dinâmico).

## Pré-requisitos

**Plano 1 (`2026-07-29-git-commit-dialog.md`) executado** — este plano consome `_validate_new_ref(cwd, kind, name)` de `git_ops.py`, criada lá. Se o plano 1 ainda não rodou, pare e rode ele antes.

## Referências

**TortoiseGit (UX a replicar):**
- Log dialog / menus de contexto por commit: https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html
- Branch/Tag: https://tortoisegit.org/docs/tortoisegit/tgit-dug-branchtag.html

**Git (flags usadas):**
- git-show(1) `-m --first-parent`, git-revert(1), git-cherry-pick(1), git-reset(1), git-tag(1), git-branch(1) — https://git-scm.com/docs

**Internas (código existente a estender — LER antes de codar cada task):**
- `backend/app/git_ops.py` — `_ACTIONS` (linha 189-198), `_SHA_RE` (354), `commit_files`/`commit_file_diff` (o par `-m --first-parent` que faz merge não vir vazio), `_validate_new_ref` (do plano 1)
- `backend/tests/test_git_ops.py` — helpers `_repo(tmp_path)` / `_repo_with_file(tmp_path)` (NÃO existem fixtures `init_repo`/`head_sha`)
- `backend/app/api.py` — `GitActionBody` com `Literal` (1833-1835), `_StrictBody` (449), rotas git (1855-1937), import de git_ops (41-42)
- `frontend/src/lib/api.ts` — `GitAction` (758), `getCommitFileDiff` (787-791, padrão de URL com query)
- `frontend/src/lib/gitStore.svelte.ts` — padrão `busy`/`error`/`refresh()`/`openLog()` dos métodos
- `frontend/src/components/GitSheet.svelte` — views `diff`/`commit`: `diffPath`/`diffSha`/`diffRows`/`commitSel`, `openCommitFileDiff`
- `frontend/src/components/GitPanel.svelte` — zona direita: `{#if diffPath && diffSha}<DiffView/>` dentro do branch `selected`
- `frontend/src/components/git/CommitList.svelte` — linha = `<button>` inteira (vai virar row com ⋯, padrão `git-file-row` de `ChangedFiles.svelte`)
- `frontend/src/components/git/CommitDetail.svelte` — detalhe do commit (ganha botão "⋯ ações")
- `frontend/src/components/git/GitToolbar.svelte` — botões de ação (ganha botão de abort condicional)
- `frontend/src/components/BottomSheet.svelte:259` — o sheet usa `z-index: 100` → o overlay do menu usa 110/120

## Global Constraints

- Backend git: **argv list sempre, shell string nunca**; sha validado por `_SHA_RE`; nomes de branch/tag por `_validate_new_ref`; modo de reset por ENUM (nunca string livre do usuário).
- **NUNCA `--force`** em nada. Operação destrutiva (reset hard) exige confirm inline em 2 passos na UI (padrão `confirmDiscard` de `ChangedFiles.svelte`).
- Comando git arbitrário NÃO é exposto — só as operações deste plano.
- Rotas FastAPI de git são `def` (não `async def`) → threadpool; `Depends(require_auth)` em toda rota nova; body com `_StrictBody`.
- Falha aparece, não some: revert/cherry-pick que conflitam voltam 409 com o stderr do git e a UI oferece o `*-abort`; nunca `ok: false` calado.
- **Duas views SEMPRE**: o menu e o diff unificado entram no `GitSheet` (mobile) E no `GitPanel` (desktop), e a verificação manual testa as duas. Atenção à diferença de arquitetura: mobile = push-view por enum `GitView`; desktop = 3 zonas por seleção (sem enum).
- UI em pt-BR; código/comentários/identificadores seguem o estilo do arquivo. Match de indentação/estilo — sem formatter.
- Diff grande: highlight via `import('../lib/highlight')` dinâmico (Shiki fora do bundle inicial).
- Gate de tipos do front: `npm --prefix frontend run check`. Gate do backend: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`.
- Commits frequentes, conventional commits, stage por path explícito (nunca `git add -A`).

## O que já existe (não recriar)

`git_log` estruturado + `assign_lanes` (grafo); `commit_files`/`commit_file_diff` (diff por arquivo dentro de um commit); `CommitList` (grafo + seleção), `CommitDetail` (arquivos do commit), `DiffView` (Shiki, recebe `path` como título e `rows` tokenizadas); ações allowlist `status/pull/fetch/stash/stash-pop/log`.

## Non-goals

Rebase interativo, resolução de conflitos com merge tool (conflito de cherry-pick/revert se resolve na sessão ou com abort), bisect, format-patch, estatísticas do log.

---

### Task 1: Backend — diff unificado do commit inteiro + revert + cherry-pick (+ aborts)

**Files:**
- Modify: `backend/app/git_ops.py` (novas funções após `commit_file_diff`, linha ~391; `_ACTIONS` linha 189)
- Modify: `backend/app/api.py` (import linha 41-42, `GitActionBody` linha 1833, novas rotas após `git_commit_diff` linha ~1930)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_SHA_RE`, `_run`, `GitError` (existentes)
- Produces:
  - `commit_diff(cwd: str, sha: str) -> dict` → `{"sha": str, "diff": str}` (unified diff do commit INTEIRO)
  - `revert_commit(cwd: str, sha: str) -> dict` → `{"ok": bool, "output": str}`
  - `cherry_pick(cwd: str, sha: str) -> dict` → idem
  - `_ACTIONS` ganha `"revert-abort"` e `"cherry-pick-abort"` (zero input → entram na allowlist)
  - Body: `GitShaBody(_StrictBody)` com `sha: str`
  - Rotas: `GET /git/commit/{sha}/diff-full`, `POST /git/revert`, `POST /git/cherry-pick`
  - `GitActionBody.action` aceita também `"revert-abort" | "cherry-pick-abort"`

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_commit_diff_inteiro(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha original\nlinha 2\n")
    (tmp_path / "g.txt").write_text("G\n")
    git_ops.commit(d, "mexe tracked e add g", ["tracked.txt", "g.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    diff = git_ops.commit_diff(d, sha)["diff"]
    assert "+linha 2" in diff and "g.txt" in diff          # os DOIS arquivos no mesmo diff


def test_commit_diff_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.commit_diff(_repo(tmp_path), "nope; rm -rf /")
    assert e.value.status == 400


def test_revert_commit(tmp_path):
    d, f = _repo_with_file(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    r = git_ops.revert_commit(d, sha)
    assert r["ok"]
    assert not f.exists()                                  # revert desfez o add, commitado
    assert "Revert" in git_ops._run(d, "log", "-1", "--pretty=%s").stdout


def test_revert_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.revert_commit(_repo(tmp_path), "--no-edit")
    assert e.value.status == 400


def test_cherry_pick(tmp_path):
    d = _repo(tmp_path)
    git_ops._run(d, "switch", "-q", "-c", "feat")
    (tmp_path / "feat.txt").write_text("F\n")
    git_ops.commit(d, "na feat", ["feat.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "switch", "-q", "main")
    r = git_ops.cherry_pick(d, sha)
    assert r["ok"] and (tmp_path / "feat.txt").exists()
    assert "na feat" in git_ops._run(d, "log", "-1", "--pretty=%s").stdout


def test_cherry_pick_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.cherry_pick(_repo(tmp_path), "HEAD; echo pwned")
    assert e.value.status == 400


def test_abort_actions_na_allowlist(tmp_path):
    d, f = _repo_with_file(tmp_path)
    # Sem sequencer em andamento, o abort FALHA (git diz "no revert in progress") —
    # o que prova que a ação chegou ao git (e não foi rejeitada pela allowlist).
    r = git_ops.git_action(d, "revert-abort")
    assert r["ok"] is False and "revert" in r["output"]
    r = git_ops.git_action(d, "cherry-pick-abort")
    assert r["ok"] is False and "cherry-pick" in r["output"]
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "commit_diff or revert or cherry or abort" -v`
Expected: FAIL (`AttributeError: module 'app.git_ops' has no attribute 'commit_diff'` etc.)

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, dentro do literal `_ACTIONS` (linha 189-198), após a entrada `"log"`:

```python
    # Saida de emergencia de revert/cherry-pick que conflitaram (sequencer em andamento).
    "revert-abort": ["revert", "--abort"],
    "cherry-pick-abort": ["cherry-pick", "--abort"],
```

Novas funções após `commit_file_diff`:

```python
def commit_diff(cwd: str, sha: str) -> dict:
    """Unified diff do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do
    Tortoise. Mesmas flags -m --first-parent de commit_files (merge = diff vs o 1o parent)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "show", "--format=", "-m", "--first-parent", sha)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git show falhou").strip() or "git show falhou")
    return {"sha": sha, "diff": p.stdout}


def revert_commit(cwd: str, sha: str) -> dict:
    """git revert --no-edit <sha>: cria um NOVO commit desfazendo <sha>. Conflito -> returncode!=0:
    o repo fica em revert-in-progress; o stderr vai pro usuário e a saída é a ação 'revert-abort'."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "revert", "--no-edit", sha)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or p.stdout or "revert falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def cherry_pick(cwd: str, sha: str) -> dict:
    """git cherry-pick <sha>: reaplica o commit em cima do HEAD. Mesmo contrato de erro do revert
    (abort via ação 'cherry-pick-abort')."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "cherry-pick", sha)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or p.stdout or "cherry-pick falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}
```

Em `backend/app/api.py`:

1. Import (linha 41-42) — acrescentar `commit_diff`, `revert_commit`, `cherry_pick`.
2. `GitActionBody` (linha 1833-1835):

```python
class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop", "log",
                    "revert-abort", "cherry-pick-abort"]
```

3. Body + rotas novas, após `git_commit_diff` (linha ~1930):

```python
class GitShaBody(_StrictBody):
    sha: str   # validado em git_ops por _SHA_RE (hex 7-40) antes de virar argv


@app.get("/api/sessions/{name}/git/commit/{sha}/diff-full", dependencies=[Depends(require_auth)])
def git_commit_diff_full(name: str, sha: str):
    try:
        return commit_diff(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/revert", dependencies=[Depends(require_auth)])
def git_revert(name: str, body: GitShaBody):
    try:
        return revert_commit(_session_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/cherry-pick", dependencies=[Depends(require_auth)])
def git_cherry_pick(name: str, body: GitShaBody):
    try:
        return cherry_pick(_session_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** (+ suíte inteira do git e self-check)

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): diff unificado do commit + revert/cherry-pick com abort"
```

---

### Task 2: Backend — reset_to + create_branch_at + create_tag

**Files:**
- Modify: `backend/app/git_ops.py` (após as funções da Task 1)
- Modify: `backend/app/api.py` (import + rotas, após as da Task 1)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_SHA_RE`, `_validate_new_ref` (**do plano 1**), `list_branches`, `_run`, `GitError`
- Produces:
  - `reset_to(cwd: str, sha: str, mode: str) -> dict` — `mode ∈ {"soft","mixed","hard"}`
  - `create_branch_at(cwd: str, name: str, sha: str | None = None, switch_after: bool = False) -> dict`
  - `create_tag(cwd: str, name: str, sha: str | None = None, message: str | None = None) -> dict`
  - Rotas: `POST /git/reset`, `POST /git/branch`, `POST /git/tag` — **o plano 5 (branch/tag) consome `create_tag`**

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_reset_to_soft_mantem_staged(tmp_path):
    d, f = _repo_with_file(tmp_path)
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    (tmp_path / "b.txt").write_text("B\n")
    git_ops.commit(d, "c2", ["b.txt"])
    r = git_ops.reset_to(d, base, "soft")
    assert r["ok"]
    assert git_ops._run(d, "rev-parse", "HEAD").stdout.strip() == base
    st = git_ops._run(d, "status", "--porcelain").stdout
    assert "A  b.txt" in st                              # soft: b.txt ficou STAGED


def test_reset_to_hard_descarta(tmp_path):
    d, f = _repo_with_file(tmp_path)
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("estragado\n")
    git_ops.commit(d, "c2", ["tracked.txt"])
    git_ops.reset_to(d, base, "hard")
    assert f.read_text() == "linha original\n"


@pytest.mark.parametrize("bad_mode", ["--hard; rm", "HARD", ""])
def test_reset_modo_invalido(tmp_path, bad_mode):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    with pytest.raises(GitError) as e:
        git_ops.reset_to(d, sha, bad_mode)               # fora do enum -> 400 antes do git
    assert e.value.status == 400


def test_reset_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.reset_to(_repo(tmp_path), "HEAD~1", "soft")
    assert e.value.status == 400


def test_create_branch_at_sem_switch(tmp_path):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    r = git_ops.create_branch_at(d, "feat-nova", sha)
    assert r["ok"]
    info = git_ops.list_branches(d)
    assert "feat-nova" in info["branches"] and info["current"] == "main"   # não trocou


def test_create_branch_at_com_switch(tmp_path):
    d = _repo(tmp_path)
    git_ops.create_branch_at(d, "feat-vai", None, switch_after=True)
    assert git_ops.list_branches(d)["current"] == "feat-vai"


def test_create_branch_nome_invalido_ou_existente(tmp_path):
    d = _repo(tmp_path)                                  # _repo já cria "feature"
    for bad in ["feature", "nome com espaço", "-D"]:
        with pytest.raises(GitError) as e:
            git_ops.create_branch_at(d, bad, None)
        assert e.value.status == 400


def test_create_tag_anotada_e_leve(tmp_path):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops.create_tag(d, "v1.0", sha, message="release 1")
    git_ops.create_tag(d, "marcador", sha)
    out = git_ops._run(d, "tag", "--format=%(refname:short)%09%(objecttype)").stdout
    assert "v1.0\ttag" in out                            # anotada = objeto tag
    assert "marcador\tcommit" in out                     # leve = aponta pro commit


def test_create_tag_invalida_ou_duplicada(tmp_path):
    d = _repo(tmp_path)
    git_ops.create_tag(d, "v1", None)
    for bad in ["v1", "nome com espaço"]:
        with pytest.raises(GitError) as e:
            git_ops.create_tag(d, bad, None)
        assert e.value.status == 400
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "reset or create_branch or create_tag" -v`
Expected: FAIL (funções não definidas)

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, após `cherry_pick`:

```python
_RESET_MODES = {"soft", "mixed", "hard"}


def reset_to(cwd: str, sha: str, mode: str) -> dict:
    """git reset --<mode> <sha>. mode vem de ENUM (nunca do usuário como string livre). hard é
    destrutivo — a UI exige confirm em 2 passos; aqui a única guarda é o enum + sha validado."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    if mode not in _RESET_MODES:
        raise GitError(400, "modo invalido")
    p = _run(cwd, "reset", f"--{mode}", sha)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "reset falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def create_branch_at(cwd: str, name: str, sha: str | None = None, switch_after: bool = False) -> dict:
    """Cria branch em <sha> (ou HEAD). NÃO troca por padrão (Tortoise: 'Create branch from revision'
    não muda a working tree); switch_after=True faz o switch depois."""
    _validate_new_ref(cwd, "heads", name)
    if sha is not None and not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    argv = ["branch", name] + ([sha] if sha else [])
    p = _run(cwd, *argv)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "criar branch falhou").strip())
    if switch_after:
        s = _run(cwd, "switch", name)
        if s.returncode != 0:
            raise GitError(409, (s.stderr or "switch falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def create_tag(cwd: str, name: str, sha: str | None = None, message: str | None = None) -> dict:
    """Tag anotada (com message) ou leve. Nome validado igual branch (refs/tags/)."""
    _validate_new_ref(cwd, "tags", name)
    if sha is not None and not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    argv = ["tag"]
    if message and message.strip():
        argv += ["-a", "-m", message]
    argv += [name] + ([sha] if sha else [])
    p = _run(cwd, *argv)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "criar tag falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}
```

Em `backend/app/api.py`:

1. Import — acrescentar `reset_to`, `create_branch_at`, `create_tag`.
2. Rotas novas, após as da Task 1:

```python
class GitResetBody(_StrictBody):
    sha: str
    mode: Literal["soft", "mixed", "hard"]   # enum no schema E no git_ops


class GitBranchBody(_StrictBody):
    name: str
    sha: str | None = None
    switch_after: bool = False


class GitTagBody(_StrictBody):
    name: str
    sha: str | None = None
    message: str | None = None


@app.post("/api/sessions/{name}/git/reset", dependencies=[Depends(require_auth)])
def git_reset(name: str, body: GitResetBody):
    try:
        return reset_to(_session_cwd(name), body.sha, body.mode)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/branch", dependencies=[Depends(require_auth)])
def git_branch_create(name: str, body: GitBranchBody):
    try:
        return create_branch_at(_session_cwd(name), body.name, body.sha, body.switch_after)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/tag", dependencies=[Depends(require_auth)])
def git_tag_create(name: str, body: GitTagBody):
    try:
        return create_tag(_session_cwd(name), body.name, body.sha, body.message)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** — suíte git inteira + self-check

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): reset soft/mixed/hard + criar branch/tag num commit"
```

---

### Task 3: Front — CommitMenu (o hub do Tortoise) + diff unificado

**Files:**
- Create: `frontend/src/components/git/CommitMenu.svelte`
- Modify: `frontend/src/lib/api.ts` (clients + `GitAction` estendido)
- Modify: `frontend/src/lib/gitStore.svelte.ts` (métodos novos + `pendingAbort`)
- Modify: `frontend/src/components/git/CommitList.svelte` (⋯ por linha)
- Modify: `frontend/src/components/git/CommitDetail.svelte` (botão "⋯ ações")
- Modify: `frontend/src/components/git/GitToolbar.svelte` (botão de abort condicional)
- Modify: `frontend/src/components/GitSheet.svelte` (menu + `openCommitFullDiff` mobile)
- Modify: `frontend/src/components/GitPanel.svelte` (menu + `openCommitFullDiff` desktop)

**Interfaces:**
- Consumes: rotas das Tasks 1-2
- Produces:
  - api.ts: `getCommitDiff(name, sha)`, `gitRevert(name, sha)`, `gitCherryPick(name, sha)`, `gitReset(name, sha, mode)`, `gitCreateBranch(name, opts)`, `gitCreateTag(name, opts)`; `GitAction` ganha `'revert-abort' | 'cherry-pick-abort'`; tipo `GitResetMode = 'soft' | 'mixed' | 'hard'`
  - gitStore: `revert(sha)`, `cherryPick(sha)`, `resetTo(sha, mode)`, `createBranch(name, sha?)`, `createTag(name, sha?, message?)`, `abortOp()`, estado `pendingAbort: string`
  - `CommitMenu.svelte` props: `{ commit: GitCommit, git: GitStore, onClose: () => void, onShowDiff: (c: GitCommit) => void }`
  - `CommitList` ganha prop OPCIONAL `onMenu?: (c: GitCommit) => void`; `CommitDetail` ganha `onMenu: (c: GitCommit) => void`

- [ ] **Step 1: Clients (api.ts)**

`GitAction` vira:

```typescript
export type GitAction = 'status' | 'pull' | 'fetch' | 'stash' | 'stash-pop' | 'log'
  | 'revert-abort' | 'cherry-pick-abort';
```

Acrescentar após `getCommitFileDiff`:

```typescript
// Diff unificado do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do Tortoise.
export function getCommitDiff(name: string, sha: string): Promise<{ sha: string; diff: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-full`);
}

export function gitRevert(name: string, sha: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/revert`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export function gitCherryPick(name: string, sha: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/cherry-pick`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export type GitResetMode = 'soft' | 'mixed' | 'hard';

export function gitReset(name: string, sha: string, mode: GitResetMode): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/reset`, {
    method: 'POST', body: JSON.stringify({ sha, mode }),
  });
}

export function gitCreateBranch(name: string, opts: { name: string; sha?: string; switch_after?: boolean }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/branch`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}

export function gitCreateTag(name: string, opts: { name: string; sha?: string; message?: string }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/tag`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}
```

Importar esses clients no `gitStore.svelte.ts` (junto do import existente de `./api`).

- [ ] **Step 2: Store (gitStore.svelte.ts)**

Estado novo junto dos demais `let`:

```typescript
  // Sequenciador em andamento: revert/cherry-pick que conflitou -> toolbar mostra o abort.
  let pendingAbort = $state<'revert-abort' | 'cherry-pick-abort' | ''>('');
```

Métodos novos (mesmo padrão `busy`/`error` dos existentes; todos mudam o log → `refresh()` + `openLog()` no sucesso):

```typescript
  // Helper interno pras ações de commit: busy/error/output + refresh/openLog; retorna sucesso.
  async function _repoOp(kind: string, fn: () => Promise<{ ok: boolean; output: string }>): Promise<boolean> {
    if (busy) return false;
    busy = kind; error = ''; output = '';
    try { const r = await fn(); output = r.output || 'ok'; await refresh(); await openLog(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function revert(sha: string) {
    const ok = await _repoOp('revert', () => gitRevert(sessionName, sha));
    if (!ok && error) pendingAbort = 'revert-abort';   // falhou (ex.: conflito) -> oferece a saída
    return ok;
  }
  async function cherryPick(sha: string) {
    const ok = await _repoOp('cherry-pick', () => gitCherryPick(sessionName, sha));
    if (!ok && error) pendingAbort = 'cherry-pick-abort';
    return ok;
  }
  async function resetTo(sha: string, mode: GitResetMode) {
    return _repoOp(`reset-${mode}`, () => gitReset(sessionName, sha, mode));
  }
  async function createBranch(name: string, sha?: string) {
    return _repoOp(name, () => gitCreateBranch(sessionName, { name, ...(sha ? { sha } : {}) }));
  }
  async function createTag(name: string, sha?: string, message?: string) {
    return _repoOp(name, () => gitCreateTag(sessionName, { name, ...(sha ? { sha } : {}), ...(message ? { message } : {}) }));
  }
  async function abortOp() {
    if (!pendingAbort || busy) return;
    await runAction(pendingAbort);
    if (!error) { pendingAbort = ''; await openLog(); }
  }
```

No `return` do store, expor `pendingAbort` (getter), `revert`, `cherryPick`, `resetTo`, `createBranch`, `createTag`, `abortOp`. `load()` também zera: acrescentar `pendingAbort = '';` em `load()`.

E atualizar o import de tipos: `type GitResetMode` junto de `./api`.

- [ ] **Step 3: CommitMenu.svelte (novo)**

Overlay backdrop + card, renderizado DENTRO do sheet/painel (que usa `z-index: 100` — ver `BottomSheet.svelte:259`): backdrop 110, card 120. Confirms inline no padrão `confirmDiscard`:

```svelte
<script lang="ts">
  import type { GitCommit } from '../../lib/api';
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    commit: GitCommit;
    git: GitStore;
    onClose: () => void;
    onShowDiff: (c: GitCommit) => void;
  }
  let { commit, git, onClose, onShowDiff }: Props = $props();

  let mode = $state<'menu' | 'branch' | 'tag' | 'reset'>('menu');
  let name = $state('');            // nome da branch/tag nova
  let tagMsg = $state('');          // mensagem opcional da tag (anotada)
  let confirmAct = $state('');      // 'cherry-pick' | 'revert' | 'hard' aguardando confirm

  // Fecha so no sucesso: no erro o git.error aparece no pe do menu e ele fica aberto (falha aparece).
  async function run(fn: () => Promise<boolean>) {
    if (await fn()) onClose();
  }
  async function copy(text: string) {
    // navigator.clipboard exige contexto seguro: PWA em http://IP-LAN pode nao ter -> falha aparece.
    try { await navigator.clipboard.writeText(text); onClose(); }
    catch { git.error = 'clipboard indisponível neste navegador/contexto'; }
  }
</script>

<div class="cm-back" onclick={onClose} role="presentation"></div>
<div class="cm" role="menu" aria-label="ações do commit {commit.short}">
  {#if mode === 'menu'}
    <p class="cm-title">commit {commit.short} — {commit.subject}</p>
    <button class="cm-item" onclick={() => { onShowDiff(commit); onClose(); }}>Ver diff completo</button>
    <button class="cm-item" onclick={() => copy(commit.hash)}>Copiar hash</button>
    <button class="cm-item" onclick={() => copy(commit.subject)}>Copiar mensagem</button>
    <button class="cm-item" onclick={() => (mode = 'branch')}>Criar branch aqui…</button>
    <button class="cm-item" onclick={() => (mode = 'tag')}>Criar tag aqui…</button>
    {#if confirmAct === 'cherry-pick'}
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.cherryPick(commit.hash))}>confirmar cherry-pick</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item" onclick={() => (confirmAct = 'cherry-pick')}>Cherry-pick</button>
    {/if}
    {#if confirmAct === 'revert'}
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.revert(commit.hash))}>confirmar revert</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item" onclick={() => (confirmAct = 'revert')}>Revert este commit</button>
    {/if}
    <button class="cm-item" onclick={() => (mode = 'reset')}>Reset até aqui ▸</button>
  {:else if mode === 'branch'}
    <p class="cm-title">branch nova em {commit.short}</p>
    <input class="cm-input" bind:value={name} placeholder="nome da branch"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <div class="cm-row">
      <button class="cm-item primary" disabled={!name.trim() || !!git.busy}
        onclick={() => run(() => git.createBranch(name.trim(), commit.hash))}>criar</button>
      <button class="cm-item" onclick={() => { mode = 'menu'; name = ''; }}>voltar</button>
    </div>
  {:else if mode === 'tag'}
    <p class="cm-title">tag nova em {commit.short}</p>
    <input class="cm-input" bind:value={name} placeholder="nome da tag"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <input class="cm-input" bind:value={tagMsg} placeholder="mensagem (opcional — vira tag anotada)"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <div class="cm-row">
      <button class="cm-item primary" disabled={!name.trim() || !!git.busy}
        onclick={() => run(() => git.createTag(name.trim(), commit.hash, tagMsg.trim() || undefined))}>criar</button>
      <button class="cm-item" onclick={() => { mode = 'menu'; name = ''; tagMsg = ''; }}>voltar</button>
    </div>
  {:else}
    <p class="cm-title">reset até {commit.short}</p>
    <button class="cm-item" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'soft'))}>soft — mantém tudo staged</button>
    <button class="cm-item" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'mixed'))}>mixed — mantém na tree, fora do stage</button>
    {#if confirmAct === 'hard'}
      <p class="cm-warn">HARD apaga mudanças não commitadas. Tem certeza?</p>
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'hard'))}>sim, reset --hard</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item danger" onclick={() => (confirmAct = 'hard')}>hard — descarta mudanças…</button>
    {/if}
    <button class="cm-item" onclick={() => { mode = 'menu'; confirmAct = ''; }}>voltar</button>
  {/if}
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>

<style>
  /* Overlay DENTRO do sheet (BottomSheet usa z-index 100): backdrop 110 / card 120. */
  .cm-back { position: fixed; inset: 0; z-index: 110; background: color-mix(in srgb, var(--bg-base) 60%, transparent); }
  .cm {
    position: fixed; z-index: 120; left: var(--space-3); right: var(--space-3); bottom: var(--space-3);
    display: flex; flex-direction: column; gap: 2px; padding: var(--space-2);
    border-radius: var(--radius-lg); border: 1px solid var(--border-default);
    background: var(--bg-elevated); box-shadow: 0 8px 30px rgb(0 0 0 / 0.35);
    animation: view-in 200ms var(--ease-out) both;
  }
  @keyframes view-in { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
  @media (min-width: 820px) {
    .cm { left: 50%; right: auto; bottom: auto; top: 30%; transform: translateX(-50%);
      width: 340px; animation: none; }
  }
  .cm-title { margin: 0; padding: var(--space-1) var(--space-2); font-size: var(--text-xs);
    color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cm-item { display: block; width: 100%; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent; color: var(--text-secondary);
    font-size: var(--text-sm); text-align: left; cursor: pointer; }
  @media (hover: hover) { .cm-item:hover { background: var(--bg-hover); } }
  .cm-item:disabled { opacity: 0.5; cursor: default; }
  .cm-item.danger { color: var(--error); }
  .cm-item.primary { color: var(--accent); }
  .cm-row { display: flex; gap: var(--space-2); }
  .cm-input { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .cm-warn { margin: 0; padding: var(--space-2); font-size: var(--text-xs); color: var(--error); }
  .git-error { margin: 0; padding: var(--space-2); font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; }
</style>
```

- [ ] **Step 4: ⋯ no CommitList e no CommitDetail**

`CommitList.svelte`: prop nova `onMenu?: (c: GitCommit) => void` — OPCIONAL de propósito: o plano 3 (blame/histórico) reusará o `CommitList` na view de histórico de arquivo SEM menu, e prop obrigatória quebraria o typecheck dele. Com `onMenu` presente (os dois callers desta task), o ⋯ aparece; a linha "Working tree changes" NÃO tem ⋯:

```svelte
interface Props {
  commits: GitCommit[];
  onSelect: (c: GitCommit | null) => void;
  onMenu?: (c: GitCommit) => void;   // opcional: reusos sem menu (ex.: histórico de arquivo) omitem
  selectedHash?: string;
  wtCount?: number;
}
```

```svelte
  {#each commits as c (c.hash)}
    {@const cx = laneX(c.col ?? 0)}
    <div class="git-commit-row">
      <button class="git-commit" class:sel={selectedHash === c.hash} onclick={() => onSelect(c)} title={c.subject}>
        <!-- …conteúdo da linha IDÊNTICO ao atual (svg do grafo + hash + ref + subject + when)… -->
      </button>
      {#if onMenu}
        <button class="git-mini" aria-label="ações do commit" title="ações do commit"
          onclick={() => onMenu(c)}>⋯</button>
      {/if}
    </div>
  {/each}
```

CSS: `.git-commit-row { display: flex; align-items: center; gap: var(--space-1); }` e `.git-commit` perde o `width: 100%`, ganha `flex: 1; min-width: 0;`. Acrescentar a réplica local de `.git-mini` (padrão do projeto, ver ChangedFiles).

`CommitDetail.svelte`: prop nova `onMenu: (c: GitCommit) => void`; botão logo após o subject:

```svelte
interface Props {
  commit: GitCommit;
  sessionName: string;
  onOpenFile: (p: string) => void;
  onMenu: (c: GitCommit) => void;
}
```

```svelte
  <div class="git-cd-head">
    <p class="git-cd-subject">{commit.subject}</p>
    <button class="git-mini" onclick={() => onMenu(commit)} aria-label="ações do commit">⋯ ações</button>
  </div>
```

CSS: `.git-cd-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-2); }` + réplica `.git-mini`.

- [ ] **Step 5: GitToolbar — botão de abort condicional**

Acrescentar ao fim da `.git-actions`, só quando há sequenciador em andamento:

```svelte
  {#if git.pendingAbort}
    <button class="git-act git-abort" disabled={!!git.busy}
      onclick={() => git.abortOp()} title="desiste da operação em conflito">abort</button>
  {/if}
```

CSS: `.git-abort { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }`.

- [ ] **Step 6: GitSheet (mobile) — menu + diff unificado**

State novo (junto dos demais):

```typescript
  let menuCommit = $state<GitCommit | null>(null);   // commit com o menu de contexto aberto
```

Função nova (espelha `openCommitFileDiff`, mas busca o diff do commit INTEIRO). Ela sempre seta `commitSel`: assim o "voltar" da view `diff` (`diffSha ? 'commit' : 'list'`) cai no detalhe do commit, mesmo quando o menu foi aberto direto da lista:

```typescript
  import { getFileDiff, getCommitFileDiff, getCommitDiff, type GitCommit } from '../lib/api';

  // Diff unificado do commit INTEIRO (menu "Ver diff completo"). Título sintético — o highlightDiff
  // usa o path só pra detectar linguagem (sem extensão = texto plano, ok pra diff multi-arquivo).
  async function openCommitFullDiff(c: GitCommit) {
    if (git.busy) return;
    commitSel = c;
    diffSha = c.hash;
    diffPath = `commit ${c.short}`;
    diffRows = [];
    diffLoading = true;
    git.error = '';
    git.busy = c.hash;
    view = 'diff';
    try {
      const { diff } = await getCommitDiff(sessionName, c.hash);
      const { highlightDiff } = await import('../lib/highlight');
      diffRows = await highlightDiff(diff, diffPath);
    } catch (e) {
      git.error = cleanErr(e);
      diffPath = '';
      view = 'commit';   // falhou -> volta pro detalhe do commit
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }
```

Wiring: `onMenu={(c) => (menuCommit = c)}` no `<CommitList>` (view `log`) e no `<CommitDetail>` (view `commit`); e, como ÚLTIMO filho do `<BottomSheet>` (depois de todos os blocos `{#if view ...}`):

```svelte
  {#if menuCommit}
    <CommitMenu commit={menuCommit} {git} onClose={() => (menuCommit = null)} onShowDiff={openCommitFullDiff} />
  {/if}
```

(Importar `CommitMenu` de `./git/CommitMenu.svelte`.)

- [ ] **Step 7: GitPanel (desktop) — menu + diff unificado**

Mesmo state `menuCommit`. O diff unificado entra na zona direita pelo MESMO encadeamento do diff por arquivo (aparece abaixo do CommitDetail, que é o comportamento do diff por arquivo hoje):

```typescript
  import { getFileDiff, getCommitFileDiff, getCommitDiff } from '../lib/api';

  // Diff unificado do commit INTEIRO (menu "Ver diff completo") — mesma zona do diff por arquivo.
  async function openCommitFullDiff(c: GitCommit) {
    if (git.busy) return;
    selected = c;
    diffSha = c.hash;
    diffPath = `commit ${c.short}`;
    diffRows = [];
    diffLoading = true;
    git.busy = c.hash;
    git.error = '';
    try {
      const { diff } = await getCommitDiff(git.sessionName, c.hash);
      const { highlightDiff } = await import('../lib/highlight');
      diffRows = await highlightDiff(diff, diffPath);
    } catch (e) {
      git.error = cleanErr(e);
      diffPath = '';
      diffSha = '';
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }
```

Wiring: `onMenu={(c) => (menuCommit = c)}` no `<CommitList>` (zona centro) e no `<CommitDetail>` (zona direita); e como último filho da `.gp`:

```svelte
  {#if menuCommit}
    <CommitMenu commit={menuCommit} {git} onClose={() => (menuCommit = null)} onShowDiff={openCommitFullDiff} />
  {/if}
```

(Importar `CommitMenu` de `./git/CommitMenu.svelte`.)

- [ ] **Step 8: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix frontend run check`
Expected: 0 erros

Manual, num repo de brinquedo (com commits que dá pra perder!), mobile E desktop:
1. ⋯ num commit multi-arquivo → "Ver diff completo" mostra os arquivos todos num diff só; voltar cai no detalhe.
2. Copiar hash/mensagem (se o clipboard falhar no http, o erro aparece — comportamento esperado).
3. "Criar branch aqui…" e "Criar tag aqui…" num commit antigo → aparecem no grafo (`refs`) e na lista de branches.
4. Cherry-pick de um commit de outra branch; revert de um commit → log ganha commit "Revert …".
5. Cherry-pick que CONFLITA → erro do git aparece + botão "abort" surge na toolbar → abort limpa.
6. Reset mixed e hard (com o confirm de 2 passos) → log/tree mudam conforme o modo.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/git/CommitMenu.svelte frontend/src/components/git/CommitList.svelte frontend/src/components/git/CommitDetail.svelte frontend/src/components/git/GitToolbar.svelte frontend/src/components/GitSheet.svelte frontend/src/components/GitPanel.svelte frontend/src/lib/api.ts frontend/src/lib/gitStore.svelte.ts
git commit -m "feat(git): menu de contexto por commit + diff unificado (hub estilo Tortoise)"
```

---

### Task 4: Gate final + docs

**Files:**
- Modify: `docs/USAGE.md` (seção `### Git`)

- [ ] **Step 1: Suíte completa backend**

Run: `cd backend && uv run pytest -v`
Expected: PASS

- [ ] **Step 2: Gate front completo**

Run: `npm --prefix frontend run check && npm --prefix frontend run build`
Expected: 0 erros + build ok

- [ ] **Step 3: Docs**

Em `docs/USAGE.md`, na seção `### Git`, acrescentar após o bullet "Histórico":

```markdown
- **Ações por commit:** o botão **⋯** (na lista de commits ou no detalhe) abre o menu do commit:
  diff completo num único texto, copiar hash/mensagem, criar branch ou tag naquele ponto,
  cherry-pick, revert (cria um commit novo desfazendo) e reset até ali (soft/mixed/hard — o hard
  pede confirmação dupla). Se um cherry-pick/revert der conflito, o erro do git aparece e um botão
  **abort** surge na barra de ações para desistir da operação.
```

- [ ] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: menu de contexto por commit (log hub estilo Tortoise)"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** diff unificado, copiar hash/mensagem, criar branch/tag, cherry-pick, revert, reset — o menu de contexto do log do Tortoise, com os aborts de sequenciador. Cada item tem task e verificação.
- **Consistência de tipos:** backend `commit_diff/revert_commit/cherry_pick(cwd, sha)`, `reset_to(cwd, sha, mode ∈ {soft,mixed,hard})`, `create_branch_at(cwd, name, sha=None, switch_after=False)`, `create_tag(cwd, name, sha=None, message=None)`; front `gitRevert/gitCherryPick/gitReset/gitCreateBranch/gitCreateTag(name, ...)`, store `revert/cherryPick/resetTo/createBranch/createTag/abortOp`, `pendingAbort`. `GitShaBody` é compartilhado por revert e cherry-pick. `CommitMenu.onShowDiff` recebe o commit (não o sha) — casa com `openCommitFullDiff(c: GitCommit)` nas duas views.
- **Sem placeholders:** todo o backend/testes/clientes/store/componente com código real; os dois trechos marcados "IDÊNTICO ao atual" (conteúdo da linha do CommitList) referem-se a markup que JÁ EXISTE no arquivo e se mantém byte a byte — o executor copia do próprio arquivo, não inventa.
- **Correções em relação ao plano monolítico:** (1) testes nos helpers reais (`_repo`/`_repo_with_file`); (2) desktop NÃO usa push-view — o diff unificado entra pela zona direita (o plano antigo dizia só "mesma entrada"); (3) `pendingAbort` no store em vez de "botões que aparecem quando o erro menciona" (mágica frágil); (4) clipboard com catch explícito (PWA em http LAN pode não ter `navigator.clipboard`); (5) overlay com z-index 110/120 medido contra o `BottomSheet` (100).
- **Decisões registradas:** reset hard existe mas com confirm em 2 passos (a regra "nunca reset --hard" é de workflow do operador; a ferramenta oferece com guarda); menu aberto da lista seta `commitSel` pro "voltar" do diff cair no detalhe; create_branch_at não troca de branch por padrão (Tortoise idem).

## Loop-readiness

- `check_cmd` por fase: Tasks 1-2 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 3 → `npm --prefix frontend run check`; Task 4 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
