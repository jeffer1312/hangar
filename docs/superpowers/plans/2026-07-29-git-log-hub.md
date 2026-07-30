# Git log como hub de ações por commit (estilo TortoiseGit) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transformar o log do painel git no hub do TortoiseGit: menu de contexto em cada commit com diff unificado do commit inteiro, copiar hash/mensagem, criar branch/tag naquele commit, cherry-pick, revert e reset (soft/mixed/hard) — mais os 4 itens levantados na leitura da referência (2026-07-30): comparar o commit com a working tree, copiar os detalhes completos, ver as branches que contêm o commit, e buscar no log por texto da mensagem.

**Architecture:** Backend: funções novas em `git_ops.py` (`commit_diff`, `revert_commit`, `cherry_pick`, `reset_to`, `create_branch_at`, `create_tag`, `diff_vs_worktree`, `branches_containing`) + `git_log` ganha `grep` + 2 ações de abort na allowlist `_ACTIONS`, expostas como rotas `def` em `api.py`. Front: clients em `api.ts`, métodos no `gitStore`, um componente novo `CommitMenu.svelte` (overlay backdrop+card, usado pelas DUAS views) aberto a partir de um botão `⋯` por commit no `CommitList` e no `CommitDetail`, e um campo de busca na `GitToolbar`. Este plano é a fatia 2 de 5 do antigo plano monolítico (removido); os outros: commit dialog, blame/histórico, stash, branch/tag.

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
- Modify: `backend/app/api.py` (import linha 41-42, `GitActionBody` linha ~1834, novas rotas após `git_commit_diff` — que está em **`api.py:1971`**, não 1930; conferir com grep antes de inserir, o arquivo andou depois do plano 1)
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
# Teto do diff do commit inteiro. O diff POR ARQUIVO e seguro por construcao; o do commit inteiro
# nao: um commit que toca 500 arquivos vira megabytes que atravessam a rede e ainda passam pelo
# Shiki num bloco so, travando o celular. 200KB ja mostra qualquer commit humano por completo.
_DIFF_MAX = 200_000


def _cap(diff: str) -> tuple[str, bool]:
    if len(diff) <= _DIFF_MAX:
        return diff, False
    return diff[:_DIFF_MAX], True


def commit_diff(cwd: str, sha: str) -> dict:
    """Unified diff do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do
    Tortoise. Mesmas flags -m --first-parent de commit_files (merge = diff vs o 1o parent)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "show", "--format=", "-m", "--first-parent", sha)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git show falhou").strip() or "git show falhou")
    diff, truncated = _cap(p.stdout)
    return {"sha": sha, "diff": diff, "truncated": truncated}


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

### Task 3: Backend — diff vs working tree + branches que contêm + busca no log

Os 3 itens de backend levantados na leitura da referência do Tortoise (2026-07-30). O 4º item
(copiar detalhes completos) é só front e entra na Task 4.

**Files:**
- Modify: `backend/app/git_ops.py` (após as funções da Task 2; `git_log` linha ~216)
- Modify: `backend/app/api.py` (import + rotas, após as da Task 2; rota `git_log_route` existente)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_SHA_RE`, `_run`, `GitError`, `_LOG_FMT` (existentes)
- Produces:
  - `diff_vs_worktree(cwd: str, sha: str) -> dict` → `{"sha": str, "diff": str}` (o "Compare with working tree")
  - `branches_containing(cwd: str, sha: str) -> dict` → `{"local": [str], "remote": [str]}`
  - `git_log(cwd, n=50, grep: str | None = None)` — filtro por texto da mensagem
  - Rotas: `GET /git/commit/{sha}/diff-worktree`, `GET /git/commit/{sha}/branches`;
    `GET /git/log` ganha o query param `q`

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_diff_vs_worktree(tmp_path):
    d, f = _repo_with_file(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("linha original\nlinha nova nao commitada\n")
    diff = git_ops.diff_vs_worktree(d, sha)["diff"]
    assert "+linha nova nao commitada" in diff       # a mudanca do DISCO vs o commit
    assert git_ops.commit_diff(d, sha)["diff"] != diff   # difere do diff do commit em si


def test_diff_vs_worktree_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.diff_vs_worktree(_repo(tmp_path), "HEAD")   # nao casa _SHA_RE
    assert e.value.status == 400


def test_branches_containing(tmp_path):
    d = _repo(tmp_path)                                  # _repo cria "main" + "feature" no mesmo commit
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "switch", "-q", "-c", "so-nesta")
    (tmp_path / "z.txt").write_text("Z\n")
    git_ops.commit(d, "so na nova", ["z.txt"])
    novo = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    assert set(git_ops.branches_containing(d, base)["local"]) == {"main", "feature", "so-nesta"}
    assert git_ops.branches_containing(d, novo)["local"] == ["so-nesta"]


def test_branches_containing_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.branches_containing(_repo(tmp_path), "--all")
    assert e.value.status == 400


def test_git_log_grep(tmp_path):
    d, _ = _repo_with_file(tmp_path)                     # commits: "init", "add tracked"
    (tmp_path / "y.txt").write_text("Y\n")
    git_ops.commit(d, "agulha no palheiro", ["y.txt"])
    assuntos = [c["subject"] for c in git_ops.git_log(d, grep="agulha")]
    assert assuntos == ["agulha no palheiro"]
    assert git_ops.git_log(d, grep="nao existe nada assim") == []
    assert len(git_ops.git_log(d)) == 3                  # sem grep, tudo


def test_git_log_grep_nao_vira_flag(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    # Texto flag-like vai como VALOR de --grep= (nunca argv separado) -> zero resultado, sem erro.
    assert git_ops.git_log(d, grep="--all") == []


def test_git_log_grep_e_literal_nao_regex(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "w.txt").write_text("W\n")
    git_ops.commit(d, "corrige c++ (de novo)", ["w.txt"])
    # Sem -F o git responderia "Invalid regular expression" (409) nestes dois:
    assert [c["subject"] for c in git_ops.git_log(d, grep="c++")] == ["corrige c++ (de novo)"]
    assert [c["subject"] for c in git_ops.git_log(d, grep="(de novo)")] == ["corrige c++ (de novo)"]
    # E o ponto e ponto, nao curinga:
    assert git_ops.git_log(d, grep="c.+") == []
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "worktree or containing or grep" -v`
Expected: FAIL (`AttributeError: module 'app.git_ops' has no attribute 'diff_vs_worktree'` etc.)

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`, após `create_tag`:

```python
def diff_vs_worktree(cwd: str, sha: str) -> dict:
    """Unified diff do commit ate o DISCO agora — o "Compare with working tree" do Tortoise.
    `git diff <sha>` = arvore de trabalho vs aquele commit (inclui o que nao esta staged)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "diff", sha)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    diff, truncated = _cap(p.stdout)      # mesmo teto do commit_diff
    return {"sha": sha, "diff": diff, "truncated": truncated}


def branches_containing(cwd: str, sha: str) -> dict:
    """Branches locais e remotas que contem o commit — o "Shows branches this commit is on"."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "branch", "-a", "--contains", sha, "--format=%(refname:short)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git branch --contains falhou").strip())
    remotes = _remote_names(cwd)          # UMA vez, fora do laco: era um subprocesso por branch
    local, remote = [], []
    for line in p.stdout.splitlines():
        name = line.strip()
        if not name or name.startswith("("):      # "(HEAD detached at ...)" nao e branch
            continue
        if "/" in name and name.split("/", 1)[0] in remotes:
            if name.endswith("/HEAD"):            # 'origin/HEAD' e ref simbolico, nao branch
                continue                          # (list_branches:159-163 filtra igual)
            remote.append(name)
        else:
            local.append(name)
    return {"local": local, "remote": remote}


def _remote_names(cwd: str) -> set[str]:
    """Nomes dos remotes ('origin', ...). Sem isto, uma branch local chamada 'feat/x' seria
    classificada como remota so por ter barra. Falha do git remote -> GitError, nunca um set vazio
    calado (que jogaria TODA branch remota pra coluna 'local')."""
    p = _run(cwd, "remote")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git remote falhou").strip())
    return {l.strip() for l in p.stdout.splitlines() if l.strip()}
```

E `git_log` ganha o filtro (assinatura retrocompatível — `grep=None` mantém todo caller atual):

```python
def git_log(cwd: str, n: int = 50, grep: str | None = None) -> list[dict]:
    """Ultimos n commits, estruturados. --topo-order (nao por data) pro grafo nao intercalar branches.
    grep filtra por texto da mensagem. Tres cuidados: (1) o texto vai GRUDADO na flag
    (`--grep=<txt>`), nunca argv separado — assim um texto que comeca com '-' e valor, nao flag;
    (2) `-F` porque --grep e REGEX por padrao: sem isso, buscar 'c++' ou '(fix)' devolve
    "Invalid regular expression" na cara do usuario, e um '.' casaria qualquer caractere calado;
    (3) `-i` pra busca no celular nao depender de maiuscula."""
    argv = ["log", "--topo-order", "-n", str(n), f"--pretty=format:{_LOG_FMT}"]
    if grep:
        argv += [f"--grep={grep}", "-F", "-i"]
    p = _run(cwd, *argv)
```

(o corpo restante de `git_log` fica IDÊNTICO ao atual — só a montagem do argv muda.)

Em `backend/app/api.py`:

1. Import — acrescentar `diff_vs_worktree`, `branches_containing`.
2. A rota de log existente ganha o query param, repassando pro `git_ops`. **Com `q`, NÃO chamar
   `assign_lanes`**: o grep tira commits do meio e o grafo desenharia arestas pra parents que não
   estão na lista. Busca ativa = lista sem grafo (decisão registrada nas notas).

```python
@app.get("/api/sessions/{name}/git/log", dependencies=[Depends(require_auth)])
def git_log_route(name: str, q: str | None = None):
    try:
        commits = git_log(_session_cwd(name), grep=q)
        return {"commits": commits if q else assign_lanes(commits)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

(Sem campo `filtered` no payload: o front já sabe se buscou — é a `logQuery` dele. Um terceiro lugar
guardando o mesmo booleano é um lugar a mais pra discordar dos outros dois.)

(Se a rota atual tiver outra forma/nome, **manter** a forma existente e só acrescentar o `q` — ler
o arquivo antes de reescrever.)

3. Rotas novas, após as da Task 2:

```python
@app.get("/api/sessions/{name}/git/commit/{sha}/diff-worktree", dependencies=[Depends(require_auth)])
def git_commit_diff_worktree(name: str, sha: str):
    try:
        return diff_vs_worktree(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/branches", dependencies=[Depends(require_auth)])
def git_commit_branches(name: str, sha: str):
    try:
        return branches_containing(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** — suíte git inteira + self-check

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): diff vs working tree, branches que contem o commit e busca no log"
```

---

### Task 4: Front — CommitMenu (o hub do Tortoise) + diff unificado

**Files:**
- Create: `frontend/src/components/git/CommitMenu.svelte`
- Create: `frontend/src/components/git/LogSearch.svelte` (busca do log — mora junto da lista, não na toolbar)
- Create: `frontend/src/lib/portal.ts` (action de teleporte pro `<body>`, hoje duplicada em 2 componentes)
- Modify: `frontend/src/lib/api.ts` (clients + `GitAction` estendido + `getGitLog(name, q?)`)
- Modify: `frontend/src/lib/gitStore.svelte.ts` (métodos novos, `pendingAbort`, `logQuery`, `runActionResult`, `cleanErr` vira export)
- Modify: `frontend/src/components/git/CommitList.svelte` (⋯ por linha + `noGraph`)
- Modify: `frontend/src/components/git/CommitDetail.svelte` (botão "⋯ ações")
- Modify: `frontend/src/components/git/GitToolbar.svelte` (abort condicional com confirm)
- Modify: `frontend/src/components/GitSheet.svelte` (menu + diffs + LogSearch/abort na view `log` — ver o cuidado do Step 5)
- Modify: `frontend/src/components/GitPanel.svelte` (menu + diffs + LogSearch na zona do centro)

**Interfaces:**
- Consumes: rotas das Tasks 1-3
- Produces:
  - api.ts: `getCommitDiff(name, sha)`, `gitRevert(name, sha)`, `gitCherryPick(name, sha)`, `gitReset(name, sha, mode)`, `gitCreateBranch(name, opts)`, `gitCreateTag(name, opts)`, `getCommitDiffVsWorktree(name, sha)`, `getCommitBranches(name, sha)`; `getGitLog(name, q?)` ganha o filtro; `GitAction` ganha `'revert-abort' | 'cherry-pick-abort'`; tipo `GitResetMode = 'soft' | 'mixed' | 'hard'`
  - gitStore: `revert(sha)`, `cherryPick(sha)`, `resetTo(sha, mode)`, `createBranch(name, sha?)`, `createTag(name, sha?, message?)`, `abortOp()`, `searchLog(q)`, estados `pendingAbort: string`, `logQuery: string`, `logFiltered: boolean`
  - `CommitMenu.svelte` props: `{ commit: GitCommit, git: GitStore, onClose: () => void, onShowDiff: (c: GitCommit) => void, onShowWorktreeDiff: (c: GitCommit) => void }`
  - `CommitList` ganha prop OPCIONAL `onMenu?: (c: GitCommit) => void`; `CommitDetail` ganha `onMenu: (c: GitCommit) => void`
  - `GitToolbar` ganha o campo de busca do log (o "Search log messages" do Tortoise)

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

// Commit vs o DISCO agora — o "Compare with working tree" do Tortoise.
export function getCommitDiffVsWorktree(name: string, sha: string): Promise<{ sha: string; diff: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-worktree`);
}

// Branches (locais e remotas) que contêm o commit.
export function getCommitBranches(name: string, sha: string): Promise<{ local: string[]; remote: string[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/branches`);
}
```

E `getGitLog` ganha o filtro (parâmetro OPCIONAL — todo caller atual segue chamando com 1 argumento):

```typescript
export function getGitLog(name: string, q?: string): Promise<{ commits: GitCommit[]; filtered?: boolean }> {
  const qs = q ? `?q=${encodeURIComponent(q)}` : '';
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/log${qs}`);
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
  // Helper interno pras acoes de commit: busy/error/output + refresh/openLog.
  // Devolve 'ok' | 'conflito' | 'erro' | 'ocupado' — nao um booleano: quem chama precisa distinguir
  // "o git entrou em sequencer" (oferece abort) de "nem comecou" (tree suja, sha invalido, rede).
  async function _repoOp(kind: string, fn: () => Promise<{ ok: boolean; output: string }>) {
    if (busy) return 'ocupado';
    busy = kind; error = ''; output = '';
    try {
      const r = await fn(); output = r.output || 'ok'; await openLog(); return 'ok';
    } catch (e) {
      error = cleanErr(e);
      // 409 = o git rodou e recusou (conflito de revert/cherry-pick deixa sequencer em andamento).
      // 400/404/rede = nao mexeu no repo.
      return String(e).includes('409') ? 'conflito' : 'erro';
    } finally {
      // refresh SEMPRE: um cherry-pick conflitado muda a lista de arquivos (conflitos aparecem).
      // Sem isto a tela segue mostrando o repo de antes do erro.
      busy = ''; await refresh();
    }
  }
  async function revert(sha: string) {
    const r = await _repoOp('revert', () => gitRevert(sessionName, sha));
    if (r === 'conflito') pendingAbort = 'revert-abort';   // so aqui ha sequencer pra abortar
    return r === 'ok';
  }
  async function cherryPick(sha: string) {
    const r = await _repoOp('cherry-pick', () => gitCherryPick(sessionName, sha));
    if (r === 'conflito') pendingAbort = 'cherry-pick-abort';
    return r === 'ok';
  }
  async function resetTo(sha: string, mode: GitResetMode) {
    return (await _repoOp(`reset-${mode}`, () => gitReset(sessionName, sha, mode))) === 'ok';
  }
  async function createBranch(name: string, sha?: string) {
    return (await _repoOp(name, () => gitCreateBranch(sessionName, { name, ...(sha ? { sha } : {}) }))) === 'ok';
  }
  async function createTag(name: string, sha?: string, message?: string) {
    return (await _repoOp(name, () => gitCreateTag(sessionName, { name, ...(sha ? { sha } : {}), ...(message ? { message } : {}) }))) === 'ok';
  }
  // git_action NAO levanta em returncode != 0 — devolve {ok:false} (git_ops.py:201-206). Olhar so o
  // `error` faria um abort recusado ("no revert in progress") sumir o botao calado.
  async function abortOp() {
    if (!pendingAbort || busy) return false;
    const r = await runActionResult(pendingAbort);
    if (r?.ok) { pendingAbort = ''; await openLog(); return true; }
    if (r && !r.ok) error = r.output || 'abort recusado pelo git';
    return false;
  }
```

O `runAction` atual engole o `ok` (grava em `output` e volta `void`). Em vez de mudar o contrato dele
— tem outros callers —, acrescentar ao lado um `runActionResult(action)` que faz o mesmo e
**retorna** `{ ok, output }`, e reescrever `runAction` como `await runActionResult(a)` descartando o
retorno. Assim nenhum caller existente muda.

Busca no log (o "Search log messages"). O `openLog()` existente passa a levar a query corrente, pra
que qualquer `refresh`/re-open depois de um revert/cherry-pick **não perca o filtro**:

```typescript
  // Busca no log. UM estado só: `logQuery` vazia = lista completa com grafo. Nao existe
  // `logFiltered` separado nem `filtered` no payload — seria a mesma informacao em 3 lugares,
  // com chance de discordarem.
  let logQuery = $state('');

  async function searchLog(q: string) {
    logQuery = q;
    await openLog();
  }
```

E dentro do `openLog()` existente, a chamada vira `getGitLog(sessionName, logQuery || undefined)`
(o resto do método fica idêntico — inclusive o controle de `logLoading`, que é o que dá o "carregando"
também pra busca).

No `return` do store, expor `pendingAbort` (getter), `logQuery`, `revert`, `cherryPick`, `resetTo`,
`createBranch`, `createTag`, `abortOp`, `searchLog`. `load()` também zera: acrescentar
`pendingAbort = ''; logQuery = '';` em `load()`.

E atualizar o import de tipos: `type GitResetMode` junto de `./api`.

- [ ] **Step 3: CommitMenu.svelte (novo)**

Overlay backdrop + card, **teleportado pro `<body>` com a action `portal`** — não basta z-index. O
`.sheet` do `BottomSheet` tem `animation` com transform persistente, `backdrop-filter` (Chromium) e
`overflow-y: auto`: cada um desses cria containing block pra `position: fixed`, então um
`.cm-back { position: fixed; inset: 0 }` declarado lá dentro cobriria só a caixa da sheet, e o card
desktop (`top: 30%`) seria cortado pelo scroll. É o mesmo bug já documentado em
`BottomSheet.svelte:170-178`, com a mesma solução.

A action existe **duplicada** em `BottomSheet.svelte:175` e `ModalDialog.svelte:35` (3 linhas cada).
Criar `frontend/src/lib/portal.ts` com ela e importar no `CommitMenu`; **não** mexer nos dois
componentes existentes (fora do escopo deste plano — deixá-los apontando pro módulo é limpeza de
outro dia).

```typescript
// frontend/src/lib/portal.ts
// Teleporta o nó pro <body>: ancestral com transform/filter/backdrop-filter cria containing block
// pra position:fixed, e o overlay ficaria preso na caixa do pai. Ver BottomSheet.svelte:170-178.
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return { destroy() { node.remove(); } };
}
```

Com o portal, os z-index seguem 110/120 acima do `z-index: 100` do `.sheet`
(`BottomSheet.svelte:259`) — agora no mesmo contexto de empilhamento, que é o que faz isso valer.

Confirms inline no padrão `confirmDiscard`:

```svelte
<script lang="ts">
  import type { GitCommit } from '../../lib/api';
  import { getCommitFiles, getCommitBranches } from '../../lib/api';
  import { portal } from '../../lib/portal';
  // cleanErr hoje e um const LOCAL dentro de createGitStore (gitStore.svelte.ts:21-23), nao um
  // export. Promove-lo a export nomeado do modulo (mover pra cima do createGitStore e prefixar
  // `export`) — o uso interno continua igual, nenhum caller muda.
  import { cleanErr, type GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    commit: GitCommit;
    git: GitStore;
    onClose: () => void;
    onShowDiff: (c: GitCommit) => void;
    onShowWorktreeDiff: (c: GitCommit) => void;
  }
  let { commit, git, onClose, onShowDiff, onShowWorktreeDiff }: Props = $props();

  let mode = $state<'menu' | 'branch' | 'tag' | 'reset' | 'branches'>('menu');
  let name = $state('');            // nome da branch/tag nova
  let tagMsg = $state('');          // mensagem opcional da tag (anotada)
  let confirmAct = $state('');      // 'cherry-pick' | 'revert' | 'hard' aguardando confirm
  let contains = $state<{ local: string[]; remote: string[] } | null>(null);   // branches que contem

  // Fecha so no sucesso: no erro o git.error aparece no pe do menu e ele fica aberto (falha aparece).
  async function run(fn: () => Promise<boolean>) {
    if (await fn()) onClose();
  }
  async function copy(text: string) {
    // navigator.clipboard exige contexto seguro: PWA em http://IP-LAN pode nao ter -> falha aparece.
    try { await navigator.clipboard.writeText(text); onClose(); }
    catch { git.error = 'clipboard indisponível neste navegador/contexto'; }
  }

  // "Copy to clipboard" do Tortoise: hash + autor + data + mensagem + arquivos. A lista de arquivos
  // vem de getCommitFiles (rota que ja existe) — assim o texto e o MESMO abrindo o menu da lista ou
  // do detalhe; sem isso, dependeria de o detalhe ja ter sido carregado.
  async function copyDetails() {
    let files: string[] | null = null;
    try { files = (await getCommitFiles(git.sessionName, commit.hash)).files.map((f) => f.path); }
    catch { files = null; }   // falha nao impede copiar o resto — mas o texto DIZ que faltou
    await copy([
      `commit ${commit.hash}`,
      `Autor:  ${commit.author}`,
      `Data:   ${new Date(commit.ts * 1000).toLocaleString()}`,
      '',
      commit.subject,
      '',
      'Arquivos:',
      ...(files === null ? ['  (lista indisponível — falha ao ler o commit)']
                         : files.map((p) => `  ${p}`)),
    ].join('\n'));
  }

  async function loadBranches() {
    mode = 'branches';
    contains = null;
    // cleanErr tira o "409: " da frente; String(e) mostraria o prefixo cru.
    try { contains = await getCommitBranches(git.sessionName, commit.hash); }
    catch (e) { git.error = cleanErr(e); }
  }
</script>

<!-- Escape fecha o MENU, nao a sheet inteira. Tem que ser na fase de CAPTURA: o BottomSheet escuta
     keydown no window na fase de bolha e chama stopImmediatePropagation (BottomSheet.svelte:130-138),
     entao um listener normal registrado depois nunca rodaria. Captura no window roda antes de todos. -->
<svelte:window onkeydowncapture={(e) => {
  if (e.key === 'Escape') { e.stopImmediatePropagation(); e.preventDefault(); onClose(); }
}} />
<div use:portal class="cm-back" onclick={onClose} role="presentation"></div>
<div use:portal class="cm" role="menu" aria-label="ações do commit {commit.short}">
  {#if mode === 'menu'}
    <p class="cm-title">commit {commit.short} — {commit.subject}</p>
    <button class="cm-item" onclick={() => { onShowDiff(commit); onClose(); }}>Ver diff completo</button>
    <button class="cm-item" onclick={() => { onShowWorktreeDiff(commit); onClose(); }}>Comparar com a working tree</button>
    <button class="cm-item" onclick={() => copy(commit.hash)}>Copiar hash</button>
    <button class="cm-item" onclick={() => copy(commit.subject)}>Copiar mensagem</button>
    <button class="cm-item" onclick={copyDetails}>Copiar detalhes completos</button>
    <button class="cm-item" onclick={loadBranches}>Branches que contêm este commit ▸</button>
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
  {:else if mode === 'branches'}
    <p class="cm-title">branches com {commit.short}</p>
    {#if contains === null}
      <p class="cm-muted">carregando…</p>
    {:else if !contains.local.length && !contains.remote.length}
      <p class="cm-muted">nenhuma branch contém este commit</p>
    {:else}
      <ul class="cm-list">
        {#each contains.local as b (b)}<li>{b}</li>{/each}
        {#each contains.remote as b (b)}<li class="cm-remote">{b}</li>{/each}
      </ul>
    {/if}
    <button class="cm-item" onclick={() => { mode = 'menu'; contains = null; }}>voltar</button>
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
<!-- O GitSheet/GitPanel tambem imprimem git.error no rodape (GitSheet:206, GitPanel:102). Com o menu
     aberto o mesmo texto apareceria duas vezes: os dois callers passam a esconder o rodape enquanto
     `menuCommit` existe ({#if git.error && !menuCommit}), porque o menu fica por cima. -->

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
  .cm-muted { margin: 0; padding: var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .cm-list { margin: 0; padding: var(--space-1) var(--space-2); max-height: 40vh; overflow-y: auto;
    list-style: none; font-family: var(--font-mono); font-size: var(--text-sm);
    color: var(--text-secondary); }
  .cm-list li { padding: 2px 0; }
  .cm-remote { color: var(--text-muted); }
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

`CommitDetail.svelte`: prop nova `onMenu?: (c: GitCommit) => void` — **opcional pelo mesmo motivo do
`CommitList`**: se o plano 3 reusar o detalhe na view de histórico de arquivo, prop obrigatória
quebra o typecheck dele. O botão só aparece com a prop presente.

```svelte
interface Props {
  commit: GitCommit;
  sessionName: string;
  onOpenFile: (p: string) => void;
  onMenu?: (c: GitCommit) => void;   // opcional: reusos sem menu omitem (idem CommitList)
}
```

```svelte
  <div class="git-cd-head">
    <p class="git-cd-subject">{commit.subject}</p>
    {#if onMenu}
      <button class="git-mini" onclick={() => onMenu(commit)} aria-label="ações do commit">⋯ ações</button>
    {/if}
  </div>
```

CSS: `.git-cd-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-2); }` + réplica `.git-mini`.

- [ ] **Step 5: LogSearch (novo) + abort — nos DOIS lugares certos**

**Cuidado medido:** no mobile a `<GitToolbar>` só existe no ramo `{:else}` do `GitSheet`
(`GitSheet.svelte:176-208`, view `list`). Quem está navegando commits está na view `log` — onde a
toolbar **não é renderizada**. Pôr a busca e o abort só na toolbar entregaria os dois invisíveis
justamente onde se usa (no desktop funcionaria, porque `GitPanel:76` sempre renderiza a toolbar:
é o drift de duas views que o CLAUDE.md avisa).

Por isso a busca vira um componente próprio, `frontend/src/components/git/LogSearch.svelte`, montado
**junto da lista de commits** — no `GitSheet` dentro do bloco da view `log`, no `GitPanel` no topo da
zona do centro:

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  interface Props { git: GitStore }
  let { git }: Props = $props();

  // Espelha a query do store: quando load() zera logQuery (reabrir a sheet), o campo acompanha em
  // vez de mostrar a busca velha sobre uma lista completa.
  let q = $state('');
  $effect(() => { q = git.logQuery; });
</script>

<form class="git-search" onsubmit={(e) => { e.preventDefault(); git.searchLog(q.trim()); }}>
  <input class="git-search-input" bind:value={q} placeholder="buscar na mensagem dos commits…"
    autocapitalize="off" autocorrect="off" spellcheck="false" />
  <button type="submit" class="git-mini" disabled={!!git.busy}>buscar</button>
  {#if git.logQuery}
    <button type="button" class="git-mini" onclick={() => git.searchLog('')}>limpar</button>
  {/if}
</form>
{#if git.logQuery}
  <p class="git-muted">resultados de "{git.logQuery}" — o grafo fica oculto enquanto a busca está ativa</p>
{/if}
```

Busca no **submit**, não a cada tecla: cada busca é um `git log` num subprocesso, e digitar no
celular dispararia um por caractere. CSS: `.git-search { display: flex; gap: var(--space-2); }`,
`.git-search-input { flex: 1; }`, mais as réplicas locais de `.git-mini`/`.git-muted` (Svelte escopa
CSS por componente).

**Abort** — mesmo problema, mesma solução: o botão precisa aparecer nas duas views. Vai na
`GitToolbar` (desktop e view `list` do mobile) **e** ao lado do `LogSearch` na view `log`. Como é
destrutivo (joga fora a resolução de conflito em andamento), leva confirm em 2 passos, igual ao
`reset --hard` e ao `confirmDiscard` de `ChangedFiles`:

```svelte
  {#if git.pendingAbort}
    {#if confirmAbort}
      <button class="git-act git-abort" disabled={!!git.busy} onclick={() => git.abortOp()}>confirmar abort</button>
      <button class="git-act" onclick={() => (confirmAbort = false)}>não</button>
    {:else}
      <button class="git-act git-abort" disabled={!!git.busy}
        onclick={() => (confirmAbort = true)} title="desiste da operação em conflito">abort…</button>
    {/if}
  {/if}
```

CSS: `.git-abort { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }`.

`CommitList.svelte` esconde o grafo quando a lista veio de uma busca: prop OPCIONAL
`noGraph?: boolean` (default `false`, então o reuso do plano 3 não muda) — o `<svg>` da lane só
renderiza com `{#if !noGraph}`. E a linha sintética "Working tree changes" (`CommitList:26-32`)
também some na busca: os callers passam `noGraph={!!git.logQuery}` e
`wtCount={git.logQuery ? 0 : git.files.length}` (ela não é resultado de busca nenhuma).

- [ ] **Step 6: GitSheet (mobile) — menu + diff unificado**

State novo (junto dos demais):

```typescript
  let menuCommit = $state<GitCommit | null>(null);   // commit com o menu de contexto aberto
```

E o `$effect` de abertura da sheet (`GitSheet.svelte:59-64`), que já reseta `view`/`diffPath`, passa
a resetar `menuCommit = null` também — senão, fechando a sheet com o menu aberto, ele reaparece
sobre a lista na próxima abertura. (No `GitPanel`, mesmo reset no ponto onde ele limpa a seleção.)

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

E a gêmea do "Comparar com a working tree" — **idêntica** a `openCommitFullDiff`, trocando só o
client e o título (que é o que o `highlightDiff` usa pra detectar linguagem):

```typescript
  // Commit vs o disco agora. Titulo diferente pro usuario saber qual dos dois diffs esta vendo.
  async function openCommitWorktreeDiff(c: GitCommit) {
    // ...corpo igual ao de openCommitFullDiff, com:
    //   diffPath = `commit ${c.short} ↔ working tree`;
    //   const { diff } = await getCommitDiffVsWorktree(sessionName, c.hash);
  }
```

Wiring: `onMenu={(c) => (menuCommit = c)}` e `noGraph={git.logFiltered}` no `<CommitList>` (view
`log`), `onMenu` no `<CommitDetail>` (view `commit`); e, como ÚLTIMO filho do `<BottomSheet>`
(depois de todos os blocos `{#if view ...}`):

```svelte
  {#if menuCommit}
    <CommitMenu commit={menuCommit} {git} onClose={() => (menuCommit = null)}
      onShowDiff={openCommitFullDiff} onShowWorktreeDiff={openCommitWorktreeDiff} />
  {/if}
```

(Importar `CommitMenu` de `./git/CommitMenu.svelte` e `getCommitDiffVsWorktree` de `../lib/api`.)

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

Mesma gêmea do mobile — `openCommitWorktreeDiff(c)`, corpo igual ao de cima trocando o client por
`getCommitDiffVsWorktree` e o título por `commit ${c.short} ↔ working tree`.

Wiring: `onMenu={(c) => (menuCommit = c)}` e `noGraph={git.logFiltered}` no `<CommitList>` (zona
centro), `onMenu` no `<CommitDetail>` (zona direita); e como último filho da `.gp`:

```svelte
  {#if menuCommit}
    <CommitMenu commit={menuCommit} {git} onClose={() => (menuCommit = null)}
      onShowDiff={openCommitFullDiff} onShowWorktreeDiff={openCommitWorktreeDiff} />
  {/if}
```

(Importar `CommitMenu` de `./git/CommitMenu.svelte` e `getCommitDiffVsWorktree` de `../lib/api`.)

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
7. **Comparar com a working tree** num commit antigo, com arquivo modificado e NÃO commitado → o
   diff mostra a mudança do disco; o título diz `↔ working tree` (distingue do "Ver diff completo").
8. **Copiar detalhes completos** → o texto colado tem hash, autor, data, mensagem e a lista de
   arquivos. Abrir o menu pela LISTA e pelo DETALHE dá o mesmo texto.
9. **Branches que contêm este commit** → num commit antigo lista várias; num commit só da branch
   atual, lista uma. Repo sem remote não quebra a seção de remotas.
10. **Busca no log**: buscar um texto que existe → só os commits que casam, sem grafo e sem a linha
    "Working tree changes", com o aviso; "limpar" volta a lista completa COM grafo. Buscar algo
    inexistente → lista vazia, sem erro. Buscar `c++` ou `(fix)` → funciona (é `-F`, não regex).
    **No mobile, buscar de DENTRO da view `log`** — é onde o campo tem que estar.
11. **Esc com o menu aberto** fecha só o menu, não a sheet. Fechar a sheet com o menu aberto e
    reabrir → o menu NÃO reaparece.
12. **Abort**: com um cherry-pick conflitado, o botão aparece nas duas views (view `log` e view
    `list` no mobile, toolbar no desktop), pede confirm, e um abort recusado pelo git mostra o erro
    em vez de sumir calado. Depois de um cherry-pick que falhou, a lista de arquivos na tela reflete
    o conflito (o `refresh` roda no `finally`).
13. Erro do menu aparece UMA vez (no menu), não duplicado no rodapé do sheet/painel.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/git/CommitMenu.svelte frontend/src/components/git/LogSearch.svelte frontend/src/components/git/CommitList.svelte frontend/src/components/git/CommitDetail.svelte frontend/src/components/git/GitToolbar.svelte frontend/src/components/GitSheet.svelte frontend/src/components/GitPanel.svelte frontend/src/lib/api.ts frontend/src/lib/gitStore.svelte.ts frontend/src/lib/portal.ts
git commit -m "feat(git): menu de contexto por commit + diff unificado (hub estilo Tortoise)"
```

---

### Task 5: Gate final + docs

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
  diff completo num único texto, comparar o commit com a working tree, copiar hash/mensagem/detalhes
  completos, ver as branches que contêm aquele commit, criar branch ou tag naquele ponto,
  cherry-pick, revert (cria um commit novo desfazendo) e reset até ali (soft/mixed/hard — o hard
  pede confirmação dupla). Se um cherry-pick/revert der conflito, o erro do git aparece e um botão
  **abort** surge na barra de ações para desistir da operação.
- **Buscar no log:** o campo acima da lista filtra os commits pelo texto da mensagem (`git log
  --grep`, ignora maiúsculas). Enquanto o filtro está ativo o grafo fica oculto — os commits do meio
  saem da lista e as linhas do grafo não teriam onde ligar. **limpar** volta a lista completa.
```

- [ ] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: menu de contexto por commit (log hub estilo Tortoise)"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** diff unificado, comparar com a working tree, copiar hash/mensagem/detalhes, branches que contêm o commit, criar branch/tag, cherry-pick, revert, reset, e busca no log — o menu de contexto do log do Tortoise, com os aborts de sequenciador. Cada item tem task e verificação.
- **Adendo 2026-07-30 (leitura da referência):** o menu real do Tortoise tem 22 itens; a versão original deste plano cobria 8. Foram somados os 4 que valiam o custo (comparar com a working tree, copiar detalhes completos, branches que contêm, busca no log) — Task 3 nova no backend + steps na Task 4. Continuam **fora**, agora por decisão registrada e não por esquecimento: `Switch/Checkout to revision` (detached HEAD é armadilha no celular, e trocar de branch já existe na BranchList), `Browse repository`, `Export this version`, `Edit notes`, `Collapse/Expand revisions`, além dos que já estavam nos non-goals (rebase, bisect, format-patch).
- **Consistência de tipos:** backend `commit_diff/revert_commit/cherry_pick(cwd, sha)`, `reset_to(cwd, sha, mode ∈ {soft,mixed,hard})`, `create_branch_at(cwd, name, sha=None, switch_after=False)`, `create_tag(cwd, name, sha=None, message=None)`; front `gitRevert/gitCherryPick/gitReset/gitCreateBranch/gitCreateTag(name, ...)`, store `revert/cherryPick/resetTo/createBranch/createTag/abortOp`, `pendingAbort`. `GitShaBody` é compartilhado por revert e cherry-pick. `CommitMenu.onShowDiff` recebe o commit (não o sha) — casa com `openCommitFullDiff(c: GitCommit)` nas duas views.
- **Sem placeholders:** todo o backend/testes/clientes/store/componente com código real; os dois trechos marcados "IDÊNTICO ao atual" (conteúdo da linha do CommitList) referem-se a markup que JÁ EXISTE no arquivo e se mantém byte a byte — o executor copia do próprio arquivo, não inventa.
- **Correções em relação ao plano monolítico:** (1) testes nos helpers reais (`_repo`/`_repo_with_file`); (2) desktop NÃO usa push-view — o diff unificado entra pela zona direita (o plano antigo dizia só "mesma entrada"); (3) `pendingAbort` no store em vez de "botões que aparecem quando o erro menciona" (mágica frágil); (4) clipboard com catch explícito (PWA em http LAN pode não ter `navigator.clipboard`); (5) overlay com z-index 110/120 medido contra o `BottomSheet` (100).
- **Decisões registradas:** reset hard existe mas com confirm em 2 passos (a regra "nunca reset --hard" é de workflow do operador; a ferramenta oferece com guarda); menu aberto da lista seta `commitSel` pro "voltar" do diff cair no detalhe; create_branch_at não troca de branch por padrão (Tortoise idem); **busca ativa esconde o grafo** — o `--grep` tira commits do meio e `assign_lanes` desenharia arestas pra parents ausentes, então o backend nem manda lanes quando há `q` e o `CommitList` recebe `noGraph`; **a busca dispara no submit**, não a cada tecla (cada busca é um subprocesso `git log`); **`--grep=<txt>` grudado na flag**, nunca argv separado, pra que texto começando com `-` seja valor e não flag; **`branches_containing` separa local/remota pela lista real de remotes** (`git remote`), não pela presença de barra — `feat/x` é branch local com barra.

## Loop-readiness

- `check_cmd` por fase: Tasks 1-3 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 4 → `npm --prefix frontend run check`; Task 5 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
