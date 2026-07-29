# Git blame + histórico por arquivo (estilo TortoiseBlame) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Levar ao painel git o TortoiseBlame e o "Show log" por arquivo: blame linha a linha com gutter colorido por commit, e log de um arquivo seguindo renames (`--follow`).

**Architecture:** Backend: extrair o parser de log de `git_log` pra `_parse_log_records` (reuso), e criar `blame` (`--line-porcelain`), `file_log` (`--follow`) e o guard `_validate_tracked` em `git_ops.py`, expostos como rotas `def` em `api.py`. Front: clients em `api.ts`, componente novo `BlameView.svelte`, entradas "histórico"/"blame" por arquivo no `ChangedFiles` e "histórico" no `CommitDetail`, e as views `blame`/`filelog` nas DUAS views (push-view no `GitSheet`, zona direita no `GitPanel`). Este plano é a fatia 3 de 5 do antigo plano monolítico (removido); os outros: commit dialog, log hub, stash, branch/tag.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repos git temporários; Svelte 5 (runes) + TypeScript.

## Pré-requisitos

Nenhum — plano autocontido (não usa nada dos planos 1-2). Pode rodar em qualquer ordem da série. Se o plano 2 (log hub) já rodou, as mudanças em `CommitList`/`CommitDetail`/`GitSheet`/`GitPanel` deste plano são ADITIVAS e coexistem com as dele.

## Referências

**TortoiseGit (UX a replicar):**
- Blame: https://tortoisegit.org/docs/tortoisegit/tgit-dug-blame.html
- Log dialog (log por arquivo): https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html

**Git (flags usadas):**
- git-blame(1) `--line-porcelain`, git-log(1) `--follow`, git-ls-files(1) `--error-unmatch` — https://git-scm.com/docs

**Internas (código existente a estender — LER antes de codar cada task):**
- `backend/app/git_ops.py` — `_LOG_FMT` (linha 213) e o parser inline dentro de `git_log` (216-243, vai virar `_parse_log_records`)
- `backend/tests/test_git_ops.py` — helpers `_repo(tmp_path)` / `_repo_with_file(tmp_path)` (NÃO existem fixtures `init_repo`/`head_sha`)
- `backend/app/api.py` — rotas git (1855-1937), import de git_ops (41-42), `_session_cwd` (1847)
- `frontend/src/lib/api.ts` — `GitCommit` (795-806), `getCommitFileDiff` (padrão de URL com query, 787-791)
- `frontend/src/components/git/DiffView.svelte` — padrão de header `.git-diff-head` + conteúdo (o `BlameView` espelha)
- `frontend/src/components/git/ChangedFiles.svelte` — padrão `confirmDiscard` (confirm inline) e `.git-file-row`/`.git-mini`
- `frontend/src/components/git/CommitDetail.svelte` — lista de arquivos do commit
- `frontend/src/components/GitSheet.svelte` — enum `GitView`, `selectCommit`, views push-view
- `frontend/src/components/GitPanel.svelte` — encadeamento da zona direita (`selected`/`diffPath`)

## Global Constraints

- Backend git: **argv list sempre, shell string nunca**; path validado como arquivo TRACKED (`_validate_tracked`) antes de virar argv; `--` antes de paths; `LC_ALL=C`/`LANGUAGE=C` em saída parseada.
- Rotas FastAPI de git são `def` (não `async def`) → threadpool; `Depends(require_auth)` em toda rota nova.
- Falha aparece, não some: erro do git volta como `GitError` com o stderr (`409`/`400`).
- **Duas views SEMPRE**: as entradas e as views novas entram no `GitSheet` (mobile) E no `GitPanel` (desktop), e a verificação manual testa as duas. Mobile = push-view por enum `GitView`; desktop = 3 zonas por seleção (sem enum) — a fiação é diferente nas duas, por desenho.
- UI em pt-BR; código/comentários/identificadores seguem o estilo do arquivo. Match de indentação/estilo — sem formatter.
- Gate de tipos do front: `npm --prefix frontend run check`. Gate do backend: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`.
- Commits frequentes, conventional commits, stage por path explícito (nunca `git add -A`).

## O que já existe (não recriar)

`git_log` estruturado (parser por `\x1f`/`\x1e`); `CommitList` (tolera commits sem `col`/`edges` — usa `?? 0`, log linear); `DiffView` (padrão visual de viewer com header); `ChangedFiles` com `.git-file-row` + `.git-mini` + confirm inline.

## Non-goals

Blame com highlight de sintaxe (Shiki por arquivo no blame — melhoria posterior); blame NUMA revisão (o backend faz blame da working tree/HEAD — por isso o CommitDetail histórico só oferece "histórico", nunca "blame": culpar a versão atual a partir de um commit antigo seria mentira); log de arquivo com grafo (sempre linear).

---

### Task 1: Backend — `_parse_log_records` + blame + file_log

**Files:**
- Modify: `backend/app/git_ops.py` (extrair parser de `git_log`, linha 216-243; novas funções após `push`, antes do `__main__`)
- Modify: `backend/app/api.py` (import linha 41-42; rotas novas após as de git existentes)
- Test: `backend/tests/test_git_ops.py` (acrescentar ao fim)

**Interfaces:**
- Consumes: `_LOG_FMT`, `_run`, `GitError` (existentes)
- Produces:
  - `_parse_log_records(out: str) -> list[dict]` — o parser que hoje é o corpo de `git_log` (shape idêntico: `hash/short/parents/refs/author/ts/rel/subject`)
  - `_validate_tracked(cwd: str, path: str) -> None`
  - `blame(cwd: str, path: str) -> dict` → `{"path": str, "lines": [{"sha": str, "short": str, "author": str, "ts": int, "lineno": int, "content": str}]}`
  - `file_log(cwd: str, path: str, n: int = 50) -> list[dict]` (mesmo shape de `git_log`)
  - Rotas: `GET /git/blame?path=`, `GET /git/file-log?path=&n=`

- [ ] **Step 1: Escrever os testes que falham**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def test_blame_linhas(tmp_path):
    d, f = _repo_with_file(tmp_path)                 # tracked.txt: 1 commit com "linha original"
    f.write_text("linha original\nlinha 2\n")
    git_ops.commit(d, "c2", ["tracked.txt"])
    b = git_ops.blame(d, "tracked.txt")
    assert [l["content"] for l in b["lines"]] == ["linha original", "linha 2"]
    assert [l["lineno"] for l in b["lines"]] == [1, 2]
    assert b["lines"][0]["sha"] != b["lines"][1]["sha"]    # cada linha no seu commit
    assert all(l["author"] and l["short"] and l["ts"] > 0 for l in b["lines"])


def test_blame_path_nao_tracked(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "solto.txt").write_text("x\n")       # untracked
    with pytest.raises(GitError) as e:
        git_ops.blame(d, "solto.txt")
    assert e.value.status == 400
    with pytest.raises(GitError):
        git_ops.blame(d, "../fora.txt")


def test_file_log_follow(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    git_ops._run(d, "mv", "tracked.txt", "renamed.txt")
    git_ops._run(d, "commit", "-q", "-m", "rename")
    log = git_ops.file_log(d, "renamed.txt")
    subjects = [c["subject"] for c in log]
    assert "rename" in subjects and "add tracked" in subjects   # --follow atravessa o rename
    assert all(c["hash"] and c["author"] for c in log)          # mesmo shape do git_log


def test_file_log_path_nao_tracked(tmp_path):
    d = _repo(tmp_path)
    with pytest.raises(GitError) as e:
        git_ops.file_log(d, "nada.txt")
    assert e.value.status == 400


def test_git_log_intacto_apos_extracao(tmp_path):
    d = _repo(tmp_path)                              # a extração do parser não pode mudar o git_log
    commits = git_ops.git_log(d)
    assert commits and commits[0]["subject"] == "init"
    assert commits[0]["parents"] == []
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd backend && uv run pytest tests/test_git_ops.py -k "blame or file_log or intacto" -v`
Expected: FAIL (`AttributeError: module 'app.git_ops' has no attribute 'blame'`)

- [ ] **Step 3: Implementar**

Em `backend/app/git_ops.py`:

1. Extrair o parser de `git_log` (sem mudar NENHUMA linha do parsing — só mover):

```python
def _parse_log_records(out: str) -> list[dict]:
    """Parser do formato _LOG_FMT (campos %x1f, registros %x1e) — compartilhado por git_log e file_log."""
    out_commits = []
    for rec in out.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        f = rec.split("\x1f")
        if len(f) < 8:
            continue
        full, short, parents, refs, author, ts, rel, subject = f[:8]
        out_commits.append({
            "hash": full,
            "short": short,
            "parents": parents.split() if parents else [],
            "refs": refs.strip(),
            "author": author,
            "ts": int(ts) if ts.isdigit() else 0,
            "rel": rel,
            "subject": subject,
        })
    return out_commits
```

`git_log` passa a terminar em `return _parse_log_records(p.stdout)` (o corpo do `for` sai dele; o tratamento de `returncode != 0` fica em `git_log`, que é quem conhece o caso "repo sem commits").

2. Novas funções após `push` (antes do `if __name__`):

```python
def _validate_tracked(cwd: str, path: str) -> None:
    """Path precisa ser um arquivo TRACKED do repo. --error-unmatch sai !=0 pra untracked/fora;
    o path vai após '--' (nunca vira flag)."""
    if not path or path.startswith("/") or ".." in path.split("/"):
        raise GitError(400, "path invalido")
    p = _run(cwd, "ls-files", "--error-unmatch", "--", path)
    if p.returncode != 0:
        raise GitError(400, "arquivo nao é tracked nesse repo")


def blame(cwd: str, path: str) -> dict:
    """git blame --line-porcelain: um bloco por linha, atributos repetidos (mais verboso que
    --porcelain, mas o parse é linear e não precisa de cache de sha). Bloco: '<sha> <orig> <final>'
    + linhas chave-valor + linha de conteúdo começando com TAB."""
    _validate_tracked(cwd, path)
    p = _run(cwd, "blame", "--line-porcelain", "--", path)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "blame falhou").strip())
    lines: list[dict] = []
    cur: dict | None = None
    for raw in p.stdout.splitlines():
        if cur is None:
            sha = raw.split(" ", 1)[0]
            cur = {"sha": sha, "short": sha[:7]}
        elif raw.startswith("\t"):
            cur["content"] = raw[1:]
            cur["lineno"] = len(lines) + 1
            lines.append(cur)
            cur = None
        elif raw.startswith("author "):
            cur["author"] = raw[7:]
        elif raw.startswith("author-time "):
            cur["ts"] = int(raw[12:])
    return {"path": path, "lines": lines}


def file_log(cwd: str, path: str, n: int = 50) -> list[dict]:
    """Log de UM arquivo, seguindo rename (--follow só vale pra path único). Sem assign_lanes:
    o front desenha linear (CommitList usa col ?? 0)."""
    _validate_tracked(cwd, path)
    p = _run(cwd, "log", "--follow", "--topo-order", "-n", str(n),
             f"--pretty=format:{_LOG_FMT}", "--", path)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git log falhou").strip())
    return _parse_log_records(p.stdout)
```

Em `backend/app/api.py`:

1. Import (linha 41-42) — acrescentar `blame`, `file_log`.
2. Rotas novas (após as rotas git existentes):

```python
@app.get("/api/sessions/{name}/git/blame", dependencies=[Depends(require_auth)])
def git_blame(name: str, path: str):
    try:
        return blame(_session_cwd(name), path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/file-log", dependencies=[Depends(require_auth)])
def git_file_log(name: str, path: str, n: int = 50):
    try:
        return {"commits": file_log(_session_cwd(name), path, n)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)
```

- [ ] **Step 4: Rodar e ver passar** (+ suíte inteira do git e self-check — o self-check cobre `git_log`, então prova que a extração não quebrou nada)

Run: `cd backend && uv run pytest tests/test_git_ops.py -v && uv run python app/git_ops.py`
Expected: PASS + `git_ops self-check OK`

- [ ] **Step 5: Commit**

```bash
git add backend/app/git_ops.py backend/app/api.py backend/tests/test_git_ops.py
git commit -m "feat(git): blame por linha + log por arquivo com --follow"
```

---

### Task 2: Front — BlameView + entradas "histórico"/"blame"

**Files:**
- Create: `frontend/src/components/git/BlameView.svelte`
- Modify: `frontend/src/lib/api.ts` (`BlameLine`, `getBlame`, `getFileLog`)
- Modify: `frontend/src/components/git/ChangedFiles.svelte` (⋯ por arquivo: histórico/blame)
- Modify: `frontend/src/components/git/CommitDetail.svelte` (⋯ por arquivo: histórico)
- Modify: `frontend/src/components/GitSheet.svelte` (views `blame`/`filelog` + `openBlame`/`openFileLog` mobile)
- Modify: `frontend/src/components/GitPanel.svelte` (mesma fiação na zona direita)

**Interfaces:**
- Consumes: rotas da Task 1
- Produces:
  - api.ts: `BlameLine` (`{sha, short, author, ts, lineno, content}`), `getBlame(name, path)`, `getFileLog(name, path)`
  - `BlameView.svelte` props: `{ path: string, lines: BlameLine[] }` (o botão voltar fica no HOST, padrão DiffView)
  - `ChangedFiles` ganha props `onBlame: (path: string) => void`, `onFileLog: (path: string) => void`
  - `CommitDetail` ganha prop `onFileLog: (path: string) => void` (sem blame — ver Non-goals)

- [ ] **Step 1: Clients (api.ts)**

Acrescentar após `getGitLog`:

```typescript
// Uma linha do blame (git blame --line-porcelain): quem commitou cada linha do arquivo.
export interface BlameLine {
  sha: string;
  short: string;
  author: string;
  ts: number;
  lineno: number;
  content: string;
}

export function getBlame(name: string, path: string): Promise<{ path: string; lines: BlameLine[] }> {
  const q = new URLSearchParams({ path });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/blame?${q}`);
}

// Log de UM arquivo (segue rename via --follow). Commits SEM lanes (o CommitList desenha linear).
export function getFileLog(name: string, path: string): Promise<{ commits: GitCommit[] }> {
  const q = new URLSearchParams({ path });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/file-log?${q}`);
}
```

- [ ] **Step 2: BlameView.svelte (novo)**

Estilo TortoiseBlame: gutter à esquerda com cor estável por commit (linhas do MESMO commit consecutivo só mostram o gutter na primeira — é o que torna blame legível), código mono à direita:

```svelte
<script lang="ts">
  import type { BlameLine } from '../../lib/api';

  interface Props {
    path: string;
    lines: BlameLine[];
  }
  let { path, lines }: Props = $props();

  // Cor estavel por commit (hash -> HSL), estilo TortoiseBlame: linhas do MESMO commit dividem cor.
  const gutterColor = (sha: string) => `hsl(${parseInt(sha.slice(0, 6), 16) % 360} 40% 55%)`;
  const shortDate = (ts: number) => new Date(ts * 1000).toLocaleDateString();
</script>

<div class="git-diff-head">
  <span class="git-diff-name">blame: {path}</span>
</div>
<div class="blame">
  {#each lines as l, i (l.lineno)}
    {@const newBlock = i === 0 || lines[i - 1].sha !== l.sha}
    <div class="blame-row">
      <span class="blame-gutter" style:border-left-color={gutterColor(l.sha)}>
        {#if newBlock}<span class="blame-meta">{l.short} · {l.author} · {shortDate(l.ts)}</span>{/if}
      </span>
      <span class="blame-code">{l.content}</span>
    </div>
  {/each}
</div>

<style>
  .git-diff-head {
    display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0;
    padding-bottom: var(--space-2); border-bottom: 1px solid var(--border-subtle);
  }
  .git-diff-name {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .blame { overflow: auto; max-height: 62vh; font-size: var(--text-xs); }
  .blame-row { display: flex; align-items: stretch; min-width: max-content; }
  .blame-gutter {
    flex: 0 0 9.5rem; border-left: 3px solid transparent; padding: 0 var(--space-2);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .blame-meta { color: var(--text-muted); font-size: 10px; }
  .blame-code { flex: 1; font-family: var(--font-mono); white-space: pre; color: var(--text-secondary); }
</style>
```

- [ ] **Step 3: ChangedFiles — ⋯ por arquivo**

Props novas (obrigatórias — os dois callers são atualizados nesta task):

```svelte
interface Props {
  git: GitStore;
  onOpenDiff: (path: string) => void;
  onCommit: () => void;
  onBlame: (path: string) => void;
  onFileLog: (path: string) => void;
}
```

State novo: `let fileMenu = $state('');   // path com o mini-menu (histórico/blame) aberto`

Na row do arquivo, o bloco `{#if confirmDiscard === f.path}…{:else}…{/if}` vira 3 ramos (untracked `??` NÃO tem ⋯ — não é tracked, o backend rejeita):

```svelte
        {#if confirmDiscard === f.path}
          <!-- …2 botões de confirm de descarte, IDÊNTICOS ao atual… -->
        {:else if fileMenu === f.path}
          <button class="git-mini" disabled={!!git.busy} onclick={() => { fileMenu = ''; onFileLog(f.path); }}>histórico</button>
          <button class="git-mini" disabled={!!git.busy} onclick={() => { fileMenu = ''; onBlame(f.path); }}>blame</button>
          <button class="git-mini" onclick={() => (fileMenu = '')} aria-label="fechar">×</button>
        {:else}
          <button class="git-mini" disabled={!!git.busy} onclick={() => (confirmDiscard = f.path)} aria-label="descartar mudanças" title="descartar mudanças">⟲</button>
          {#if f.code !== '??'}
            <button class="git-mini" aria-label="histórico e blame" title="histórico e blame" onclick={() => (fileMenu = f.path)}>⋯</button>
          {/if}
        {/if}
```

- [ ] **Step 4: CommitDetail — ⋯ por arquivo (só histórico)**

Prop nova `onFileLog: (path: string) => void` e state `let fileMenu = $state('');`. A lista de arquivos vira rows com ⋯ (arquivo de commit histórico NÃO oferece blame: o backend culpa a working tree ATUAL, não a daquele commit — ver Non-goals):

```svelte
  <div class="git-cd-files">
    {#each files as f (f.path)}
      <div class="git-file-row">
        <button class="git-file" onclick={() => onOpenFile(f.path)} title="ver diff">
          <span class="git-file-tag">{f.code}</span><span class="git-path-base">{f.path}</span>
        </button>
        {#if fileMenu === f.path}
          <button class="git-mini" onclick={() => { fileMenu = ''; onFileLog(f.path); }}>histórico</button>
          <button class="git-mini" onclick={() => (fileMenu = '')} aria-label="fechar">×</button>
        {:else}
          <button class="git-mini" aria-label="histórico do arquivo" title="histórico do arquivo" onclick={() => (fileMenu = f.path)}>⋯</button>
        {/if}
      </div>
    {:else}
      <p class="git-muted">nenhum arquivo alterado</p>
    {/each}
  </div>
```

CSS: `.git-file-row { display: flex; align-items: center; gap: var(--space-2); }`, `.git-file` ganha `flex: 1; min-width: 0;` (mantém o resto), réplica `.git-mini` (padrão do projeto).

- [ ] **Step 5: GitSheet (mobile) — views `blame` e `filelog`**

Enum e states novos:

```typescript
  type GitView = 'list' | 'log' | 'diff' | 'commit' | 'commitbox' | 'blame' | 'filelog';
```

```typescript
  let blameLines = $state<BlameLine[]>([]);
  let blamePath = $state('');
  let blameLoading = $state(false);
  let blameBack = $state<'list' | 'commit'>('list');     // de onde o blame foi aberto
  let fileLogCommits = $state<GitCommit[]>([]);
  let fileLogPath = $state('');
  let fileLogLoading = $state(false);
  let fileLogBack = $state<'list' | 'commit'>('list');
  let commitBack = $state<'log' | 'filelog'>('log');     // voltar do detalhe respeita a origem
```

Importar `getBlame, getFileLog, type BlameLine` de `../lib/api` e `BlameView` de `./git/BlameView.svelte`. Funções novas (espelham `openDiff` no busy/error):

```typescript
  async function openBlame(path: string) {
    if (git.busy) return;
    blameBack = view === 'commit' ? 'commit' : 'list';
    blamePath = path; blameLines = []; blameLoading = true;
    git.error = ''; git.busy = path;
    view = 'blame';
    try { blameLines = (await getBlame(sessionName, path)).lines; }
    catch (e) { git.error = cleanErr(e); view = blameBack; }
    finally { blameLoading = false; git.busy = ''; }
  }

  async function openFileLog(path: string) {
    if (git.busy) return;
    fileLogBack = view === 'commit' ? 'commit' : 'list';
    fileLogPath = path; fileLogCommits = []; fileLogLoading = true;
    git.error = ''; git.busy = path;
    view = 'filelog';
    try { fileLogCommits = (await getFileLog(sessionName, path)).commits; }
    catch (e) { git.error = cleanErr(e); view = fileLogBack; }
    finally { fileLogLoading = false; git.busy = ''; }
  }
```

`selectCommit` passa a lembrar a origem (pro voltar do detalhe cair na view certa quando o commit foi aberto do filelog):

```typescript
  function selectCommit(c: GitCommit | null) {
    if (c) { commitBack = view === 'filelog' ? 'filelog' : 'log'; commitSel = c; view = 'commit'; }
    else { view = 'list'; }
  }
```

Blocos de render novos no `<BottomSheet>` (antes do bloco `{:else}` final), e o voltar da view `commit` passa a usar `commitBack`:

```svelte
  {:else if view === 'blame'}
    <div class="git">
      <button class="git-back" onclick={() => (view = blameBack)} aria-label="Voltar">‹ voltar</button>
      {#if blameLoading}
        <p class="git-muted">carregando blame…</p>
      {:else}
        <BlameView path={blamePath} lines={blameLines} />
      {/if}
    </div>
  {:else if view === 'filelog'}
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = fileLogBack)} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">histórico: {fileLogPath}</span>
      </div>
      {#if fileLogLoading}
        <p class="git-muted">carregando…</p>
      {:else}
        <CommitList commits={fileLogCommits} onSelect={selectCommit} />
      {/if}
    </div>
```

No bloco da view `commit`, o botão voltar vira `onclick={() => (view = commitBack)}`.

Wiring: `<ChangedFiles {git} onOpenDiff={openDiff} onCommit={() => (view = 'commitbox')} onBlame={openBlame} onFileLog={openFileLog} />` e `<CommitDetail commit={commitSel} {sessionName} onOpenFile={openCommitFileDiff} onFileLog={openFileLog} />`.

Nota: se o plano 2 já rodou, `CommitList` tem a prop OPCIONAL `onMenu` — aqui ela simplesmente se omite (o ⋯ não aparece no filelog; o menu fica disponível no detalhe do commit). Nada a fazer.

- [ ] **Step 6: GitPanel (desktop) — mesma fiação na zona direita**

States novos:

```typescript
  let blameData = $state<{ path: string; lines: BlameLine[] } | null>(null);
  let fileLogData = $state<{ path: string; commits: GitCommit[] } | null>(null);
```

Importar `getBlame, getFileLog, type BlameLine` e `BlameView`. Funções novas:

```typescript
  async function openBlame(path: string) {
    if (git.busy) return;
    blameData = null; fileLogData = null;
    git.busy = path; git.error = '';
    try { blameData = await getBlame(git.sessionName, path); }
    catch (e) { git.error = cleanErr(e); }
    finally { git.busy = ''; }
  }

  async function openFileLog(path: string) {
    if (git.busy) return;
    blameData = null; fileLogData = null;
    git.busy = path; git.error = '';
    try { fileLogData = { path, commits: (await getFileLog(git.sessionName, path)).commits }; }
    catch (e) { git.error = cleanErr(e); }
    finally { git.busy = ''; }
  }
```

Limpar os dois states quando outra coisa ocupa a zona: acrescentar `blameData = null; fileLogData = null;` no início de `openWtDiff`, de `openCommitDiff` e do handler `onSelect` do `<CommitList>` da zona central.

Zona direita — dois ramos novos ANTES do `{#if selected === null}`:

```svelte
      {#if blameData}
        <button class="git-back" onclick={() => (blameData = null)} aria-label="Voltar">‹ voltar</button>
        <BlameView path={blameData.path} lines={blameData.lines} />
      {:else if fileLogData}
        <button class="git-back" onclick={() => (fileLogData = null)} aria-label="Voltar">‹ voltar</button>
        <p class="git-muted">histórico: {fileLogData.path}</p>
        <CommitList commits={fileLogData.commits}
          onSelect={(c) => { if (c) { fileLogData = null; selected = c; diffPath = ''; diffSha = ''; } }} />
      {:else if selected === null}
        <!-- …resto do encadeamento IDÊNTICO ao atual… -->
```

Wiring: `<ChangedFiles … onBlame={openBlame} onFileLog={openFileLog} />` na `.gp-left` e `onFileLog={openFileLog}` no `<CommitDetail>`. Réplica local de `.git-back` no `<style>` do GitPanel (copiar de `GitSheet.svelte`: `.git-back { align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default); background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }`).

**Higiene cruzada (aplicar só se os planos 4 e/ou 5 rodaram antes deste):** um flag de view da zona direita sempre desliga os outros, nos DOIS sentidos — `openBlame`/`openFileLog` ganham no início `stashOpen = false;` (plano 4) e/ou `tagsOpen = false;` (plano 5); e os handlers `openStash`/`openTags` deles ganham `blameData = null; fileLogData = null;`. Se já houver ramos deles no encadeamento, os novos entram como `{:else if blameData}`/`{:else if fileLogData}` imediatamente antes do branch `selected === null`. (Standalone não tem com quem conflitar; a nota é o contrato de coabitação.)

- [ ] **Step 7: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix frontend run check`
Expected: 0 erros

Manual, mobile E desktop:
1. Blame de um arquivo com 3+ commits distintos: cores distintas por commit, gutter só na 1ª linha de cada bloco; scroll horizontal mantém gutter fixo à esquerda do bloco de código.
2. ⋯ num arquivo untracked NÃO aparece; ⋯ num tracked abre histórico/blame; erro de blame em path inválido aparece (não some).
3. Histórico de um arquivo renomeado: o log atravessa o rename (commits anteriores ao rename aparecem); tocar num commit abre o detalhe e o voltar retorna ao histórico.
4. ⋯ num arquivo do CommitDetail abre o histórico; voltar retorna ao detalhe.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/git/BlameView.svelte frontend/src/components/git/ChangedFiles.svelte frontend/src/components/git/CommitDetail.svelte frontend/src/components/GitSheet.svelte frontend/src/components/GitPanel.svelte frontend/src/lib/api.ts
git commit -m "feat(git): BlameView + histórico por arquivo (TortoiseBlame)"
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
- **Blame e histórico por arquivo:** o **⋯** ao lado de um arquivo alterado abre **histórico**
  (log só daquele arquivo, seguindo renomeações) e **blame** (quem commitou cada linha, com uma cor
  por commit, estilo TortoiseBlame). No detalhe de um commit, o ⋯ de cada arquivo abre o histórico
  dele.
```

- [ ] **Step 4: Commit**

```bash
git add docs/USAGE.md
git commit -m "docs: blame + histórico por arquivo"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** blame linha a linha (Task 1 backend + Task 2 BlameView) e log por arquivo com `--follow` (Task 1 backend + Task 2 entradas/views). Cada um com verificação manual nas duas views.
- **Consistência de tipos:** `blame(cwd, path) -> {"path", "lines":[{sha, short, author, ts, lineno, content}]}` casa com `BlameLine` do TS; `file_log(cwd, path, n=50) -> list[dict]` = shape de `git_log`/`GitCommit` (rota embrulha em `{"commits": ...}`, igual `git_log_route`); `_parse_log_records` preserva byte a byte o parsing que hoje vive em `git_log` (e `test_git_log_intacto_apos_extracao` trava a regressão).
- **Sem placeholders:** backend/testes/componente novo com código completo; os trechos "IDÊNTICO ao atual" são markup existente do próprio arquivo que se mantém (confirm de descarte em ChangedFiles, resto do encadeamento da zona direita no GitPanel).
- **Correções em relação ao plano monolítico:** (1) testes nos helpers reais (`_repo`/`_repo_with_file`); (2) fiação desktop explícita na zona direita (o plano antigo dizia "mesma entrada" sem dizer onde); (3) navegação de retorno com origem (`blameBack`/`fileLogBack`/`commitBack`) — o plano antigo não dizia pra onde o "voltar" ia; (4) CommitDetail histórico NÃO oferece blame (o backend culpa a working tree atual, não a revisão — oferecer seria incorreto).
- **Decisões registradas:** blame renderizado em texto plano mono (Shiki por arquivo fica pra depois); gutter colorido por hash de commit com bloco só na 1ª linha consecutiva; ⋯ escondido em untracked (`??` não é tracked → 400 do backend).

## Loop-readiness

- `check_cmd` por fase: Task 1 → `cd backend && uv run pytest tests/test_git_ops.py -q`; Task 2 → `npm --prefix frontend run check`; Task 3 → `cd backend && uv run pytest -q && npm --prefix frontend run check`.
- Regra da casa: plano superpowers executa SEMPRE via superpowers — a sessão que rodar o loop deve carregar `superpowers:executing-plans` e iterar as tasks, com o loop fornecendo o re-prompt a cada idle.
