# Git commit dialog estilo TortoiseGit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transformar o CommitBox do painel git no dialog de commit do TortoiseGit: select all/none, mensagens recentes, amend (reword + dobrar arquivos) e commit direto numa branch nova.

**Architecture:** Backend: `commit()` de `backend/app/git_ops.py` ganha `amend` e `new_branch` (+ helper `_validate_new_ref` e função `last_commit_message`), expostos nas rotas git de `api.py`. Front: `commitFiles`/`doCommit` ganham `opts`, e `CommitBox.svelte` ganha as 4 features de UI. Mensagens recentes ficam em `localStorage` (conveniência de UI, zero backend). Este plano é a fatia 1 de 5 do antigo plano monolítico `2026-07-29-git-tortoise-completo.md` (removido); os outros: log hub, blame/histórico, stash, branch/tag.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repos git temporários; Svelte 5 (runes) + TypeScript.

## Pré-requisitos

Nenhum — é o plano 1 da série. Ele **produz** `_validate_new_ref` (git_ops), consumida pelos planos 2 (log hub) e 5 (branch/tag): executar este antes deles.

## Referências

**TortoiseGit (UX a replicar):**
- Commit dialog (inventário completo): https://tortoisegit.org/docs/tortoisegit/tgit-dug-commit.html

**Git (flags usadas):**
- git-commit(1) `--amend --only` (reword sem vazar staged — verificado em sandbox), git-check-ref-format(1), git-switch(1) `-c` — https://git-scm.com/docs

**Internas (código existente a estender — LER antes de codar cada task):**
- `backend/app/git_ops.py` — `commit` (linha ~394), `changed_files`, `_run`, `GitError`, `_scrub`; padrão argv list + `LC_ALL=C` + `--` antes de paths
- `backend/tests/test_git_ops.py` — helpers `_repo(tmp_path)` / `_repo_with_file(tmp_path)` (NÃO existem fixtures `init_repo`/`head_sha`; o estilo é helper de módulo + `git_ops._run` inline)
- `backend/app/api.py:1842-1844` — `GitCommitBody` (com `Field(min_length=1)` nos dois campos — ver Task 1); rotas git em `api.py:1855-1937`; import de git_ops em `api.py:41-42`
- `frontend/src/lib/api.ts:818-828` — `commitFiles`, `gitPush`
- `frontend/src/lib/gitStore.svelte.ts` — `doCommit`, `refresh`, `openLog`
- `frontend/src/components/git/CommitBox.svelte` — componente inteiro a reescrever

## Global Constraints

- Backend git: **argv list sempre, shell string nunca**; input do usuário validado contra lista real ou regex estrita antes de ir pro argv; `--` separando paths de flags; `LC_ALL=C`/`LANGUAGE=C` em todo comando cuja saída é parseada.
- **NUNCA `--force`** em nada. Amend existe, mas a UI esconde "Commit & Push" quando amend está marcado (push de commit amendado exigiria `--force`).
- Rotas FastAPI de git são `def` (não `async def`) → threadpool; `Depends(require_auth)` em toda rota nova; body com `_StrictBody`.
- Falha aparece, não some: erro do git volta como `GitError` com o stderr (traduzido pra `409`/`400`), nunca `ok: false` calado.
- **Duas views SEMPRE**: toda feature de UI entra no `GitSheet` (mobile) E no `GitPanel` (desktop) — aqui as duas já compartilham o `CommitBox`, então basta verificar as duas no manual. Elas driftam fácil — é o gotcha documentado do projeto.
- UI em pt-BR; código/comentários/identificadores seguem o estilo do arquivo (comentários em pt-BR, identificadores em inglês). Match de indentação/estilo do arquivo vizinho — sem formatter.
- Gate de tipos do front: `npm --prefix frontend run check` (o `build` NÃO checa tipos). Gate do backend: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py` (self-check).
- Commits frequentes, conventional commits (`feat:`, `test:`), stage por path explícito (nunca `git add -A`).

## O que já existe (não recriar)

`commit(cwd, message, paths)` com checkbox por arquivo, rename atômico e anti-vazamento de staged (`--only`); `push` com `-u` no 1º push; `CommitBox` com seleção default "todos marcados", Commit / Commit & Push.

## Non-goals

Stage por hunk ("commit só parte do arquivo" do Tortoise via restore-after-commit) — candidato a plano futuro. Assinatura GPG, template de mensagem, spellcheck da mensagem.

---

### Task 1: Backend — `commit` com `amend` e `new_branch` + `last_commit_message`

**Files:**
- Modify: `backend/app/git_ops.py` (função `commit`, linha ~394; acrescentar `_validate_new_ref` e `last_commit_message` antes dela)
- Modify: `backend/app/api.py` (import linha 41-42, `GitCommitBody` linha 1842-1844, rota `git_commit` linha ~1911, nova rota `git_last_message`)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim do arquivo)

**Interfaces:**
- Consumes: `changed_files`, `_run`, `GitError` (existentes)
- Produces:
  - `commit(cwd: str, message: str, paths: list[str], amend: bool = False, new_branch: str | None = None) -> dict`
  - `last_commit_message(cwd: str) -> dict` → `{"message": str}` (corpo inteiro, `%B`)
  - `_validate_new_ref(cwd: str, kind: str, name: str) -> None` — `kind ∈ {"heads","tags"}`; **os planos 2 e 5 consomem esta função**
  - `GitCommitBody` ganha `amend: bool = False`, `new_branch: str | None = None` e **perde** o `min_length=1` de `paths`
  - Nova rota: `GET /api/sessions/{name}/git/last-message`

- [x] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py` (seguindo o estilo do arquivo: helpers de módulo, `git_ops.` prefixado, `tmp_path`):

```python
def test_commit_amend_reescreve_e_dobra(tmp_path):
    d, f = _repo_with_file(tmp_path)          # commits: "init" (vazio) + "add tracked"
    (tmp_path / "novo.txt").write_text("N\n")
    r = git_ops.commit(d, "mensagem corrigida", ["novo.txt"], amend=True)
    assert r["ok"]
    out = git_ops._run(d, "log", "--pretty=%s").stdout.splitlines()
    assert out == ["mensagem corrigida", "init"]        # 2 commits, não 3
    names = git_ops._run(d, "show", "--name-only", "--format=", "HEAD").stdout.split()
    assert "tracked.txt" in names and "novo.txt" in names   # dobrou sem perder o original


def test_commit_amend_sem_paths_e_so_reword(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    # Mudança staged por FORA do app NÃO pode vazar pra dentro de um reword:
    (tmp_path / "staged.txt").write_text("S\n")
    git_ops._run(d, "add", "staged.txt")
    r = git_ops.commit(d, "so renomeia a mensagem", [], amend=True)   # paths vazio SÓ vale com amend
    assert r["ok"]
    assert git_ops._run(d, "log", "-1", "--pretty=%B").stdout.strip() == "so renomeia a mensagem"
    names = git_ops._run(d, "show", "--name-only", "--format=", "HEAD").stdout.split()
    assert "staged.txt" not in names                      # --amend --only: staged não vaza
    assert "A  staged.txt" in git_ops._run(d, "status", "--porcelain").stdout   # continua staged


def test_commit_amend_sem_head_falha(tmp_path):
    d = str(tmp_path)
    for args in (["init", "-q", "-b", "main"], ["config", "user.email", "t@t"], ["config", "user.name", "t"]):
        git_ops._run(d, *args)                          # repo SEM nenhum commit
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", [], amend=True)
    assert e.value.status == 409


def test_commit_new_branch(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "x.txt").write_text("X\n")
    r = git_ops.commit(d, "na branch nova", ["x.txt"], new_branch="feat-x")
    assert r["ok"]
    assert git_ops._run(d, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() == "feat-x"


@pytest.mark.parametrize("bad", ["feature", "nome com espaço", "-D", "../x"])
def test_commit_new_branch_invalida_ou_existente(tmp_path, bad):
    d = _repo(tmp_path)                                 # _repo já cria a branch "feature"
    (tmp_path / "x.txt").write_text("X\n")
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", ["x.txt"], new_branch=bad)
    assert e.value.status == 400


def test_commit_paths_vazio_sem_amend_segue_falhando(tmp_path):
    d = _repo(tmp_path)
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", [])                      # regra antiga intacta
    assert e.value.status == 400


def test_last_commit_message(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    git_ops._run(d, "commit", "-q", "--amend", "-m", "assunto\n\ncorpo da mensagem")
    assert git_ops.last_commit_message(d)["message"] == "assunto\n\ncorpo da mensagem"
```

- [x] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "amend or new_branch or last_commit or paths_vazio" -v`
Expected: FAIL (`TypeError: commit() got an unexpected keyword argument 'amend'` / `AttributeError: module 'app.git_ops' has no attribute 'last_commit_message'`)

- [x] **Step 3: Implementar**

Em `backend/app/git_ops.py`, logo antes de `def commit` (linha ~394):

```python
_BRANCH_REF_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$")
_REF_KIND_LABEL = {"heads": "branch", "tags": "tag"}


def _validate_new_ref(cwd: str, kind: str, name: str) -> None:
    """kind = 'heads' | 'tags'. Rejeita nome inválido pro git (check-ref-format) ou já existente.
    O regex prévio bara flag-like ('-D', '../') antes do subprocesso; o check-ref-format pega o resto
    (espaço, '..', '~', '^', ':', barra final...)."""
    label = _REF_KIND_LABEL[kind]
    if not _BRANCH_REF_RE.match(name) or ".." in name:
        raise GitError(400, f"nome de {label} inválido")
    p = _run(cwd, "check-ref-format", f"refs/{kind}/{name}")
    if p.returncode != 0:
        raise GitError(400, f"nome de {label} inválido")
    lst = _run(cwd, "branch" if kind == "heads" else "tag", "--format=%(refname:short)")
    if name in {l.strip() for l in lst.stdout.splitlines()}:
        raise GitError(400, f"{label} já existe: {name}")


def last_commit_message(cwd: str) -> dict:
    """Mensagem completa (%B) do HEAD — pra pré-preencher o amend. Repo sem commit -> 409."""
    p = _run(cwd, "log", "-1", "--pretty=%B")
    if p.returncode != 0:
        raise GitError(409, "sem commits pra amend")
    return {"message": p.stdout.rstrip("\n")}
```

Reescrever `commit` (manter o bloco de renames idêntico ao existente — só movido):

```python
def commit(cwd: str, message: str, paths: list[str], amend: bool = False,
           new_branch: str | None = None) -> dict:
    """Commita SO os paths marcados (checkbox estilo Tortoise). Valida cada path contra a lista real
    de alterados (anti-traversal/flag-like); mensagem nao pode ser vazia. `-m` recebe a mensagem como
    argv (nunca shell) -> sem injecao.
    amend=True permite paths=[] (so reword) e dobra os paths marcados no commit anterior. Sempre
    `--amend --only`: sem isso um reword puro (`--amend -m`) dobraria mudancas staged por fora do app
    (verificado: --only sem paths = reword, index intocado; com paths = dobra so eles).
    new_branch cria a branch (switch -c) ANTES do commit — se o commit falhar, o usuário fica na
    branch nova com as mudanças intactas (o erro do git é reportado, falha aparece)."""
    if not message.strip():
        raise GitError(400, "mensagem vazia")
    if not paths and not amend:
        raise GitError(400, "nenhum arquivo selecionado")
    # TODAS as validacoes (sem efeito colateral) antes do switch/add:
    valid = {f["path"] for f in changed_files(cwd)}
    for p in paths:
        if p not in valid:
            raise GitError(400, f"arquivo nao esta na lista de alterados: {p}")
    if amend:
        last_commit_message(cwd)   # falha 409 se não há HEAD pra amend
    if new_branch is not None:
        _validate_new_ref(cwd, "heads", new_branch)
    # Efeitos colaterais:
    if new_branch is not None:
        s = _run(cwd, "switch", "-c", new_branch)
        if s.returncode != 0:
            raise GitError(409, (s.stderr or "criar branch falhou").strip())
    # Renomeados: o git colapsa "R old -> new" e changed_files so expoe `new`. Committar so `new`
    # transformaria o rename num "add" e deixaria a delecao de `old` staged e orfã. Detecta o par pelo
    # status e inclui `old` no pathspec do commit pra manter o rename atômico. (`old` vem do git, nao do
    # usuario -> nao precisa da validacao anti-traversal que os `paths` ja passaram.)
    renames: dict[str, str] = {}
    for line in _run(cwd, "status", "--porcelain").stdout.splitlines():
        if line[:1] == "R" and " -> " in line[3:]:
            old, new = line[3:].split(" -> ", 1)
            renames[new] = old
    extra = [renames[p] for p in paths if p in renames]
    # `add` primeiro: --only sozinho falha em arquivo untracked ("pathspec did not match").
    # `commit --only -- <paths>` grava SO esses paths, mesmo que outros estejam staged no indice.
    if paths:
        _run(cwd, "add", "--", *paths)
        argv = ["commit", "--only", "-m", message, "--", *paths, *extra]
        if amend:
            argv.insert(1, "--amend")
    else:
        argv = ["commit", "--amend", "--only", "-m", message]   # reword puro (amend garantido acima)
    r = _run(cwd, *argv)
    if r.returncode != 0:
        raise GitError(409, (r.stderr or r.stdout or "commit falhou").strip() or "commit falhou")
    return {"ok": True, "output": (r.stdout + r.stderr).strip()}
```

Em `backend/app/api.py`:

1. Import (linha 41-42) — acrescentar `last_commit_message` à lista importada de `app.git_ops`.
2. `GitCommitBody` (linha 1842-1844) — o `min_length=1` de `paths` rejeitaria o amend-só-reword na camada pydantic ANTES do git_ops; a regra "paths vazio exige amend" fica no git_ops (400 igual):

```python
class GitCommitBody(_StrictBody):
    message: str = Field(min_length=1)
    paths: list[str] = []        # sem min_length: amend=True aceita [] (reword); git_ops barra [] sem amend
    amend: bool = False
    new_branch: str | None = None
```

3. Rota `git_commit` (linha ~1911) — repassar os campos novos:

```python
@app.post("/api/sessions/{name}/git/commit", dependencies=[Depends(require_auth)])
def git_commit(name: str, body: GitCommitBody):
    try:
        return commit(_session_cwd(name), body.message, body.paths, body.amend, body.new_branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

4. Nova rota, logo após `git_commit`:

```python
@app.get("/api/sessions/{name}/git/last-message", dependencies=[Depends(require_auth)])
def git_last_message(name: str):
    try:
        return last_commit_message(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [x] **Step 4: Rodar e ver passar** (+ suíte inteira do git e self-check)

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS em tudo (inclusive os testes antigos — a assinatura nova é retrocompatível) + `git_ops self-check OK`

- [x] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): commit com amend (--amend --only) e new-branch + last_commit_message"
```

---

### Task 2: Front — CommitBox Tortoise (select all/none, mensagens recentes, amend, branch nova)

**Files:**
- Modify: `frontend/src/lib/api.ts` (`commitFiles` linha ~818 + `getLastCommitMessage` nova)
- Modify: `frontend/src/lib/gitStore.svelte.ts` (`doCommit` estendido)
- Modify: `frontend/src/components/git/CommitBox.svelte` (reescrever — arquivo pequeno, substituição completa é mais segura que patch)

**Interfaces:**
- Consumes: rotas da Task 1 (`POST /git/commit` com `amend`/`new_branch`, `GET /git/last-message`)
- Produces:
  - `commitFiles(name, message, paths, opts?: { amend?: boolean; newBranch?: string })` em `api.ts`
  - `getLastCommitMessage(name): Promise<{ message: string }>` em `api.ts`
  - `doCommit(message, paths, opts?)` no gitStore — mesmo contrato, usado só pelo CommitBox

- [x] **Step 1: Clients e store**

Em `frontend/src/lib/api.ts`, substituir `commitFiles` e acrescentar `getLastCommitMessage`:

```typescript
export function commitFiles(name: string, message: string, paths: string[],
                            opts?: { amend?: boolean; newBranch?: string }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit`, {
    method: 'POST',
    body: JSON.stringify({ message, paths, amend: opts?.amend ?? false, new_branch: opts?.newBranch ?? null }),
  });
}

// Mensagem completa do HEAD (pra pré-preencher o amend). 409 se o repo não tem commit.
export function getLastCommitMessage(name: string): Promise<{ message: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/last-message`);
}
```

Em `frontend/src/lib/gitStore.svelte.ts`, trocar a assinatura de `doCommit` e a chamada a `commitFiles` (o resto do método fica idêntico — `refresh()` já relê branches+current quando `newBranch` troca a branch):

```typescript
  async function doCommit(message: string, paths: string[], opts?: { amend?: boolean; newBranch?: string }) {
    if (busy) return false;
    busy = 'commit'; error = ''; output = '';
    try { const r = await commitFiles(sessionName, message, paths, opts); output = r.output || 'commit ok'; await refresh(); await openLog(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
```

- [x] **Step 2: CommitBox — reescrita completa**

Substituir `frontend/src/components/git/CommitBox.svelte` por:

```svelte
<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  import { getLastCommitMessage } from '../../lib/api';

  interface Props { git: GitStore; onDone?: () => void; }
  let { git, onDone }: Props = $props();

  // Todos os arquivos alterados marcados por padrao (staged + unstaged + untracked).
  let sel = $state<Set<string>>(new Set());
  let selectionInitialized = $state(false);
  $effect(() => {
    if (!selectionInitialized && git.files.length) {
      sel = new Set(git.files.map((f) => f.path));
      selectionInitialized = true;
    }
  });
  let message = $state('');
  let amend = $state(false);
  let wantBranch = $state(false);
  let newBranch = $state('');

  const toggle = (p: string) => { sel.has(p) ? sel.delete(p) : sel.add(p); sel = new Set(sel); };
  const chosen = $derived(git.files.filter((f) => sel.has(f.path)).map((f) => f.path));
  // amend sem arquivos marcados = so reword (o backend faz --amend --only: staged nao vaza).
  const canCommit = $derived(
    !!message.trim() && (chosen.length > 0 || amend) && !git.busy && (!wantBranch || !!newBranch.trim()),
  );

  // Mensagens recentes: conveniencia de UI por sessao, sem backend (localStorage).
  const MSG_KEY = $derived(`cp_git_msgs::${git.sessionName}`);
  let recent = $state<string[]>([]);
  $effect(() => {
    try { recent = JSON.parse(localStorage.getItem(MSG_KEY) ?? '[]'); } catch { recent = []; }
  });
  function rememberMessage(msg: string) {
    const list = [msg, ...recent.filter((m) => m !== msg)].slice(0, 10);
    recent = list;
    try { localStorage.setItem(MSG_KEY, JSON.stringify(list)); } catch { /* modo privado/quota: nao trava o commit */ }
  }

  async function toggleAmend() {
    amend = !amend;
    if (amend && !message.trim()) {
      try { message = (await getLastCommitMessage(git.sessionName)).message; }
      catch { /* repo sem HEAD: o commit devolve o 409 do backend, falha aparece la */ }
    }
  }

  async function doCommit(thenPush: boolean) {
    if (!canCommit) return;
    const ok = await git.doCommit(message, chosen, {
      amend, newBranch: wantBranch ? newBranch.trim() : undefined,
    });
    // Push de amend exigiria --force (proibido) -> o botao Commit & Push some com amend marcado.
    if (ok && thenPush && !amend) { const pushOk = await git.doPush(); }
    if (ok) {
      rememberMessage(message.trim());
      message = ''; amend = false; wantBranch = false; newBranch = '';
      onDone?.();
    }
  }
</script>

<div class="cb">
  <div class="cb-sel-row">
    <button class="git-mini" onclick={() => (sel = new Set(git.files.map((f) => f.path)))}>todos</button>
    <button class="git-mini" onclick={() => (sel = new Set())}>nenhum</button>
  </div>
  <div class="cb-files">
    {#each git.files as f (f.path)}
      <label class="cb-file">
        <input type="checkbox" checked={sel.has(f.path)} onchange={() => toggle(f.path)} />
        <span class="cb-code">{f.code.trim() || '?'}</span>
        <span class="cb-path">{f.path}</span>
      </label>
    {/each}
    {#if !git.files.length}<p class="git-muted">nada pra commitar</p>{/if}
  </div>
  {#if recent.length}
    <select class="cb-recent" value="" onchange={(e) => { const v = e.currentTarget.value; if (v) message = v; e.currentTarget.value = ''; }}>
      <option value="">mensagens recentes…</option>
      {#each recent as r (r)}<option value={r}>{r.length > 72 ? r.slice(0, 72) + '…' : r}</option>{/each}
    </select>
  {/if}
  <textarea class="cb-msg" bind:value={message} placeholder="mensagem do commit…" rows="3"
    autocapitalize="off" spellcheck="false"></textarea>
  <div class="cb-opts">
    <label class="cb-opt"><input type="checkbox" checked={amend} onchange={toggleAmend} /> reescrever o último commit (amend)</label>
    <label class="cb-opt"><input type="checkbox" bind:checked={wantBranch} /> commitar numa branch nova</label>
  </div>
  {#if wantBranch}
    <input class="cb-branch" bind:value={newBranch} placeholder="nome da branch nova"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <p class="cb-hint">cria a branch a partir da atual antes de commitar</p>
  {/if}
  <div class="cb-actions">
    <button class="cb-btn" disabled={!canCommit} onclick={() => doCommit(false)}>Commit</button>
    {#if !amend}
      <button class="cb-btn primary" disabled={!canCommit} onclick={() => doCommit(true)}>Commit &amp; Push</button>
    {/if}
  </div>
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>

<style>
  .cb { display: flex; flex-direction: column; gap: var(--space-3); }
  .cb-sel-row { display: flex; gap: var(--space-2); justify-content: flex-end; }
  .cb-files { display: flex; flex-direction: column; gap: 2px; max-height: 40vh; overflow-y: auto; }
  .cb-file { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2);
    font-size: var(--text-sm); cursor: pointer; }
  .cb-code { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); min-width: 1.4rem; }
  .cb-path { font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cb-msg { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); resize: vertical; }
  .cb-recent { width: 100%; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-secondary);
    font-size: var(--text-sm); }
  .cb-opts { display: flex; flex-direction: column; gap: var(--space-1); }
  .cb-opt { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);
    color: var(--text-secondary); cursor: pointer; }
  .cb-branch { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .cb-hint { margin: calc(-1 * var(--space-2)) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  .cb-actions { display: flex; gap: var(--space-2); }
  .cb-btn { flex: 1; padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .cb-btn.primary { background: var(--accent); color: var(--bg-base); border-color: var(--accent); }
  .cb-btn:disabled { opacity: 0.5; cursor: default; }

  /* Svelte escopa CSS por componente — replicas locais dos padroes de ChangedFiles/CommitList. */
  .git-mini { flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer; }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-error { margin: 0; font-size: var(--text-sm); color: var(--error); white-space: pre-wrap; word-break: break-word; }
</style>
```

- [x] **Step 3: Gate de tipos**

Run: `npm --prefix frontend run check`
Expected: 0 erros

- [x] **Step 4: Verificação manual (mobile E desktop)**

Abrir o painel git de uma sessão com mudanças, num repo de brinquedo:
1. Marcar/desmarcar "todos"/"nenhum"; commitar normal → mensagem entra nas recentes; reusar uma recente pelo select.
2. Amend: marcar "reescrever", mensagem pré-preenche com a do HEAD; commitar sem marcar arquivo (reword) e depois com arquivo marcado (dobra). Conferir que "Commit & Push" some com amend marcado.
3. Branch nova: commitar com nome de branch → badge da sessão muda pra branch nova; nome inválido/existente mostra o 400 do git.
4. Repetir 1-3 no desktop (≥820px, CommitBox na zona direita do GitPanel).

- [x] **Step 5: Commit**

```bash
git add frontend/src/lib/api.ts frontend/src/lib/gitStore.svelte.ts frontend/src/components/git/CommitBox.svelte
git commit -m "feat(git): CommitBox estilo Tortoise — recentes, amend, branch nova, select all"
```

---

### Task 3: Gate final + docs

**Files:**
- Modify: `docs/USAGE.md` (seção `### Git`, linha ~192-201)

- [x] **Step 1: Suíte completa backend**

Run: `cd backend && uv run pytest -v`
Expected: PASS (não só a de git — o relaxamento do `GitCommitBody.paths` não pode ter quebrado outra rota)

- [x] **Step 2: Gate front completo**

Run: `npm --prefix frontend run check && npm --prefix frontend run build`
Expected: 0 erros + build ok

- [x] **Step 3: Docs**

Em `docs/USAGE.md`, na seção `### Git`, atualizar o bullet de Commit:

```markdown
- **Commit:** em "Working tree changes", marque os arquivos desejados (ou **todos**/**nenhum**),
  escreva a mensagem e confirme — só os arquivos marcados entram no commit (funciona igual em
  mobile e desktop). O select **mensagens recentes…** reaproveita as últimas 10 mensagens daquela
  sessão. Dá pra **reescrever o último commit (amend)**: a mensagem vem pré-preenchida e os
  arquivos marcados dobram nele (com amend, o botão Commit & Push some — push de amend exigiria
  `--force`). Também dá pra **commitar numa branch nova**, criada a partir da atual.
```

- [x] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: commit dialog estilo Tortoise (amend, branch nova, recentes)"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** select all/none, mensagens recentes, amend (reword + dobrar), branch nova — os 4 itens do dialog Tortoise que fazem sentido no app, cada um com task e verificação.
- **Consistência de tipos:** `commit(cwd, message, paths, amend=False, new_branch=None)`; `last_commit_message(cwd) -> {"message": str}`; `_validate_new_ref(cwd, kind ∈ {"heads","tags"}, name)`; `GitCommitBody{message, paths=[], amend, new_branch}`; front `commitFiles(name, message, paths, opts?)`, `getLastCommitMessage(name)`, `doCommit(message, paths, opts?)`. O body JSON usa `new_branch` (snake) e o client TS converte de `newBranch` (camel) — mesmo padrão do resto do `api.ts`.
- **Sem placeholders:** backend e testes com código completo e verificado em sandbox (`--amend --only` sem paths = reword sem vazar staged; com paths = dobra só eles); front é substituição completa do componente.
- **Correções em relação ao plano monolítico:** (1) testes usam os helpers REAIS do arquivo (`_repo`/`_repo_with_file` — `init_repo`/`head_sha` nunca existiram); (2) `GitCommitBody.paths` perde o `min_length=1` (rejeitaria amend-só-reword na borda); (3) amend usa `--amend --only` sempre (o `--amend -m` do plano antigo vazava staged pra dentro do reword); (4) validações sem efeito colateral rodam ANTES do `switch -c`.
- **Decisões registradas:** mensagens recentes em `localStorage` por sessão (não backend); amend esconde Commit & Push; falha do `switch -c` deixa o usuário na branch nova com mudanças intactas (reportado no erro).

## Loop-readiness

- `check_cmd` por fase: Task 1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 2 → `npm --prefix frontend run check`; Task 3 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
