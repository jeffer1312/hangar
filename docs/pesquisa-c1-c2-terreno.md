# Terreno real do C1 (gerenciador de arquivos) e C2 (diff do arquivo inteiro)

Levantamento read-only feito em 2026-08-15, antes de escrever o plano. Serve para corrigir duas
afirmações de `docs/pesquisa-referencias-2026-08-13.md` que **não se sustentam no código**:

- C1 (linha 820): *"Metade do backend já existe: `fs.py` tem `scan_dir` e `getRoots`"*.
- C2 (linha 845): *"Boa notícia: o backend já faz."*

As duas são otimistas. O detalhe está nas seções 1 e 3.

---

## 1. `backend/app/fs.py` — lista pastas, nunca arquivos

| Função | Onde | O que devolve |
|---|---|---|
| `list_roots() -> list[FsRoot]` | `backend/app/fs.py:29` | só as raízes da allowlist; `FsRoot{name, path}` (`backend/app/models.py:221`) |
| `scan_dir(root, path=None) -> FsScanResult` | `backend/app/fs.py:34` | `FsScanResult{entries, error}` (`backend/app/models.py:236`); `FsEntry{name, path, is_git, has_claude_md, mtime}` (`backend/app/models.py:227`) |

**`scan_dir` descarta tudo que não é diretório** (`fs.py:76-77`: `if not e.is_dir(): continue`) e
esconde dot-dirs, `.git` incluído (`fs.py:73-74`). É um seletor de projeto, não um listador de repo.

**Não existe leitura de conteúdo de arquivo em lugar nenhum do backend.** Sem `read_file`, sem
streaming, sem limite de tamanho, sem detecção de binário — porque nada disso é necessário para
listar pastas. Para o C1 isso é **zero de reaproveitamento**, não metade.

O que de fato se reaproveita é a **trava de raiz**, e ela é boa: `_real()` resolve
`expanduser+realpath` antes de comparar (`fs.py:18-21`); `root` tem que casar exatamente uma raiz da
allowlist (`fs.py:46-49`, 403 `root not allowed`); `path` tem que estar contido nela via `_within`
(`fs.py:24-26`, `fs.py:52-54`, 400 `path escapes its root`); symlink que aponta para fora nunca
aparece (`fs.py:79-80`). Allowlist em `resolve_scan_roots` (`backend/app/config.py:180-196`), var
`CP_SCAN_ROOTS`, default `~/pessoal,~/sistemas` (`config.py:90,104`).

`scan_dir` é **não-recursivo e sem paginação**: um nível por chamada HTTP. Hoje isso não incomoda
porque o uso é escolher uma pasta; numa árvore de repositório inteiro vira N chamadas por nível.

### Rotas de `fs` hoje

- `GET /api/fs/roots` — `backend/app/api.py:3810`, `Depends(require_auth)`.
- `GET /api/fs/scan?root=&path=` — `backend/app/api.py:3815`; traduz `FsError` (`fs.py:8-15`) em
  `HTTPException`.
- Auth: `require_auth` (`backend/app/auth.py:86-111`) — Bearer, `?token=` ou cookie `cp_token`.

## 2. Frontend: não há árvore nem virtualização

- `FolderScanner.svelte` (`frontend/src/components/FolderScanner.svelte:3,30,59`) chama
  `getRoots()`/`scanDir()` e desenha **drill-in por coluna** — chips de raiz, busca, breadcrumb,
  lista de subpastas — não uma árvore com vários nós abertos ao mesmo tempo. Único consumidor:
  `components/CreateSessionSheet.svelte`.
- **Nenhum componente de árvore expansível existe** no front (a varredura por *tree*/*árvore*/
  *expand*/*folder* só acha árvore de acessibilidade, "working tree" do git e a árvore em ASCII que
  o TUI desenha, `AssistantBubble.svelte:429-442` + `lib/format.ts:340-345`).
- **Nenhuma biblioteca de virtualização de lista** instalada — `frontend/package.json:26-32` tem só
  `@xterm/xterm`, `@xterm/addon-fit`, `qr-scanner`, `shiki`, `uplot`.

## 3. `backend/app/git_ops.py` — não existe diff cumulativo por arquivo

| Função | Onde | Granularidade | Cap |
|---|---|---|---|
| `changed_files(cwd)` | `git_ops.py:317` | `git status --porcelain` → `{path, code, staged}` | — |
| `file_diff(cwd, path)` | `git_ops.py:338` | **working tree vs HEAD**, um arquivo (`git diff HEAD -- path`; untracked via `--no-index`) | **nenhum** |
| `commit_files(cwd, sha)` | `git_ops.py:372` | arquivos de um commit | — |
| `commit_file_diff(cwd, sha, path)` | `git_ops.py:394` | um arquivo dentro de **um** commit | **nenhum** |
| `commit_diff(cwd, sha)` | `git_ops.py:421` | commit inteiro | 200 KB (`_DIFF_MAX`, `git_ops.py:412`) |
| `diff_vs_worktree(cwd, sha)` | `git_ops.py:511` | commit → disco agora, **repo inteiro, sem filtro de path** | 200 KB |
| `git_summary(cwd)` | `git_ops.py:101` | `{dirty, ahead, behind}` | cache 3s / 30s (`git_ops.py:93-99`) |

Três consequências diretas para o C2:

1. **Nenhuma função soma vários commits para um path.** Não há `git diff <base>..HEAD -- path`. As
   duas granularidades por arquivo são "disco vs HEAD" e "dentro de um commit".
   `file_diff` cobre o caso "o agente editou e ainda não commitou" — que é o caso comum — mas para
   de cobrir no instante em que alguém commita no meio da sessão.
2. **As duas rotas por arquivo não têm cap de tamanho.** O limite de 200 KB só existe em
   `commit_diff` e `diff_vs_worktree` — exatamente as que o C2 não usa.
3. **Nenhum diff tem cache** (só `git_summary` tem). Cada clique na árvore forka um `git`.

Também ausente: **qualquer flag de whitespace** (`-w`, `--ignore-space-change`,
`--ignore-all-space`) — o "esconder espaço em branco" do Paseo não tem base hoje.

E `EditDiff.svelte` (`frontend/src/components/EditDiff.svelte:11-15`) **também não agrega**: usa
`computeEditDiff` (`frontend/src/lib/editdiff.ts:112`) por edição isolada, um card por `tool_input`.

### Rotas de git

- `GET /api/sessions/{name}/git/files` → `api.py:2881`
- `POST /api/sessions/{name}/git/diff` (body `GitPathBody{path}`) → `api.py:2903`
- `GET …/git/commit/{sha}/files` → `api.py:2935` · `…/diff?path=` → `api.py:2943`
- `GET …/git/commit/{sha}/diff-full` → `api.py:2972` · `…/diff-worktree` → `api.py:3028`

**Toda rota de git passa por `_session_cwd(name)`** (`api.py:2776-2781`, 404 se não existe): não há
acesso a git por cwd arbitrário, sempre por sessão tmux viva. Wrappers no front em
`frontend/src/lib/api.ts`: `getRoots:496`, `scanDir:507`, `getChangedFiles:931`, `getFileDiff:935`,
`getCommitFiles:942`, `getCommitFileDiff:946`, `getCommitDiff:954`, `getCommitDiffVsWorktree:992`.

## 4. Peças de UI que o C2 reusa de verdade

- `DiffView.svelte` (`frontend/src/components/git/DiffView.svelte:4-10`) — props
  `{path, rows, loading, truncated?}`. **Puramente apresentacional**, não busca nada; o fetch é do
  `GitStore`. Já mostra `+N/−M` no cabeçalho e o aviso de "cortado em 200 KB" (`:35-36`). É reuso
  limpo. **Não tem** alternância unificado ↔ lado a lado.
- `GitChangesTab.svelte` (`…/git/GitChangesTab.svelte:9-17`) — props
  `{git, desktop, level, onPush, onPop}`; desktop em 3 colunas (`:102-115`), celular em drill-down
  por nível (`:116-124`). É o precedente mais próximo do que o C1+C2 querem ser.
- `lib/gitTabs.ts:1-44` — navegação por abas dentro do modal: `GitNav{tab, levels}`, com
  profundidade máxima **por aba** (`:11-15`: changes 1, history 2, branches 0).
  `GitTabs.svelte:74-88` despacha para a aba.

## 5. Desktop × celular, hoje

- Breakpoint único `matchMedia('(min-width: 820px)')` — `frontend/src/App.svelte:124-131`; decide
  entre `DesktopShell` (`App.svelte:435-451`) e `SessionList`/`Chat` (`:452-469`).
- **`Git.svelte` nunca é docado**: é sempre `BottomSheet` (`components/Git.svelte:27`), e a única
  diferença entre as duas telas é `wide={desktop} centered={desktop}` — modal centrado de
  `min(1100px, 92vw)` no desktop, folha subindo no celular. Aberto em `Sidebar.svelte:1305`,
  `screens/SessionList.svelte:1010` e `screens/Chat.svelte:1612`.
- `DesktopSessionContext.svelte` é o painel flutuante do contexto da sessão (breakpoint 1280px,
  `screens/Chat.svelte:1782-1784`), com botão que abre esse mesmo modal de git. **Não há árvore
  nele.**
- **Um painel docado permanente à direita do chat seria peça nova**, não uma variação de props do
  que existe.
- Rotas hash em `frontend/src/lib/route.ts:20-95`; `board` (`:14`) e `canvas` (`:17`) **só existem
  no desktop** (`:66-93`) — no celular a mesma URL cai na lista. Listener em `App.svelte:256`.

## 6. Tokens de superfície

`--surface-raised` e `--surface-inset` existem, definidos duas vezes de propósito: base em
`frontend/src/app.css:108-109` e a variante com alpha do papel de parede em `app.css:394-395`
(`color-mix(… var(--cp-surface-alpha, 0.87) …)`); uso em `app.css:770`.

## 7. Cobertura de teste existente

- `backend/tests/test_fs_scan.py` (209 linhas, 15 testes) — `resolve_scan_roots`, `list_roots`,
  `scan_dir` (ordem por mtime, drill, raiz não permitida, escape por `..`, escape por symlink,
  symlink filho escondido, 404, "not a directory", permissão negada) e as duas rotas.
- `backend/tests/test_git_ops.py` (574 linhas, ~24 testes) — branches, switch, `changed_files`,
  `file_diff` (rejeita path não listado, mostra edições, untracked mostra o arquivo inteiro),
  discard, stash, commit.
- `backend/tests/test_git_summary.py` (149 linhas, ~15 testes) — só `git_summary`.
- **Buraco:** `commit_diff`, `diff_vs_worktree` e `commit_file_diff` não têm teste em pytest — os
  cenários existem só no self-check `if __name__ == "__main__"` dentro de `git_ops.py:659-849`. O
  cap de 200 KB não é testado em lugar nenhum.
