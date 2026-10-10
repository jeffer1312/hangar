"""Worktrees: onde a sessão está de verdade, a situação de cada worktree e a remoção segura."""
import filecmp
import json
import logging
import os
import re
import subprocess
import threading
import time
import queue
from dataclasses import dataclass
from pathlib import Path

from app import atomico
from app.git_ops import _FETCH_TIMEOUT, GitError, _run, _scrub, head_info, remove_worktree

_log = logging.getLogger("hangar.worktrees")

REMOVED_FILE = Path.home() / ".hangar" / "worktrees-removidas.json"
_TAIL = 256 * 1024


@dataclass
class Location:
    branch: str | None
    worktree: bool
    worktree_path: str | None
    worktree_gone: bool
    # Raiz do repositório onde o agente trabalha, quando não é a da pasta de abertura.
    git_cwd: str | None = None


def repo_root_of(path: str | None) -> str | None:
    """Sobe até achar `.git` (pasta ou arquivo), sem subprocesso: roda por sessão na listagem."""
    if not path:
        return None
    p = Path(path)
    for d in (p, *p.parents):
        if (d / ".git").exists():
            return str(d)
    return None


def main_repo_of(root: str) -> str:
    """A pasta principal: ela mesma, ou a dona da worktree (`gitdir: <repo>/.git/worktrees/<n>`)."""
    dot = Path(root, ".git")
    try:
        if dot.is_file():
            txt = dot.read_text(encoding="utf-8", errors="replace").strip()
            if txt.startswith("gitdir: "):
                # normpath: com `worktree.useRelativePaths` o ponteiro vem com `..`.
                gitdir = Path(os.path.normpath(os.path.join(root, txt[len("gitdir: "):])))
                if gitdir.parent.name == "worktrees":
                    return str(gitdir.parent.parent.parent)
    except OSError:
        pass
    return root


def worktree_paths(main: str) -> list[str]:
    """As worktrees ligadas ao repo, de `.git/worktrees/*/gitdir` (aponta pro `.git` delas)."""
    out: list[str] = []
    try:
        entries = sorted(Path(main, ".git", "worktrees").iterdir())
    except OSError:
        return out
    for e in entries:
        try:
            g = (e / "gitdir").read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            continue
        if g:
            out.append(os.path.dirname(os.path.normpath(os.path.join(e, g))))
    return out


_cache: dict[str, tuple[tuple[int, int], object]] = {}
_cache_lock = threading.Lock()


def _cached(path: str, read):
    try:
        st = os.stat(path)
    except OSError:
        return None
    key = (st.st_mtime_ns, st.st_size)
    with _cache_lock:
        hit = _cache.get(path)
        if hit and hit[0] == key:
            return hit[1]
    value = read(path)
    with _cache_lock:
        _cache[path] = (key, value)
    return value


def _tail_lines(path: str) -> list[bytes]:
    with open(path, "rb") as fh:
        fh.seek(0, os.SEEK_END)
        size = fh.tell()
        fh.seek(max(0, size - _TAIL))
        lines = fh.read().split(b"\n")
    return lines[1:] if size > _TAIL else lines   # a primeira veio cortada


_DEEP_TAIL = 8 * 1024 * 1024


def reversed_lines(path: str):
    """Linhas do fim para o começo, em blocos de `_TAIL`, até `_DEEP_TAIL` bytes. Imagem lida pela
    sessão entra no transcript em base64 e enche sozinha os últimos 256 KB."""
    with open(path, "rb") as fh:
        end = pos = fh.seek(0, os.SEEK_END)
        rest: list[bytes] = []   # pedaços da linha ainda cortada; juntar só no `\n` não recopia megas
        while pos > 0 and end - pos < _DEEP_TAIL:
            step = min(_TAIL, pos)
            pos -= step
            fh.seek(pos)
            block = fh.read(step)
            if b"\n" not in block:
                rest.insert(0, block)
                continue
            lines = (block + b"".join(rest)).split(b"\n")
            rest = [lines[0]]
            yield from reversed(lines[1:])
        if pos == 0:
            yield b"".join(rest)


# `cd X` no início de um comando, ou `git -C X`: o comando roda em X.
_SHELL_DIR_RE = re.compile(r'(?:(?:^|[\n;&|(])\s*cd|\bgit\s+-C)\s+(["\']?)([^\s;&|"\')]+)\1')
_EDIT_TOOLS = {"Edit": "file_path", "MultiEdit": "file_path", "Write": "file_path",
               "NotebookEdit": "notebook_path"}


def _tool_paths(block: dict) -> list[tuple[str, bool]]:
    """(caminho, é `cd`?) de uma chamada, do último citado para o primeiro."""
    args = block.get("input")
    name = block.get("name")
    if not isinstance(args, dict):
        return []
    if name == "Bash" and isinstance(args.get("command"), str):
        return [(m.group(2), True) for m in reversed(list(_SHELL_DIR_RE.finditer(args["command"])))]
    target = args.get(_EDIT_TOOLS[name]) if name in _EDIT_TOOLS else None
    return [(target, False)] if isinstance(target, str) else []


def _claude_tail(jsonl: str) -> tuple[str | None, list[tuple[str, bool, str | None]]]:
    """(último `cwd`, caminhos citados pelas ferramentas como (caminho, é `cd`?, `cwd` da linha)),
    do mais recente para o mais antigo. Uma leitura só: o cache é por arquivo."""
    def read(p: str):
        last: str | None = None
        hits: list[tuple[str, bool, str | None]] = []
        for raw in reversed_lines(p):
            has_tool = b'"tool_use"' in raw
            # Achado o último `cwd`, só interessa linha com chamada: o resto pode ser imagem de megas.
            if not has_tool and (last or b'"cwd"' not in raw):
                continue
            try:
                line = json.loads(raw)
            except ValueError:
                continue
            if not isinstance(line, dict):
                continue
            cwd = line.get("cwd")
            cwd = cwd if isinstance(cwd, str) and cwd else None
            last = last or cwd   # a varredura vem do fim: o primeiro achado é o mais recente
            content = (line.get("message") or {}).get("content") if has_tool else None
            for block in reversed(content) if isinstance(content, list) else ():
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    hits.extend((path, is_cd, cwd) for path, is_cd in _tool_paths(block))
            if len(hits) >= 20:
                break
        return last, hits
    return _cached(jsonl, read) or (None, [])


def claude_cwd(jsonl: str) -> str | None:
    """`cwd` da última linha que o tem: o Claude grava a pasta em cada linha, e ela muda no
    `EnterWorktree` e no `cd` dentro do projeto."""
    return _claude_tail(jsonl)[0]


def _repo_candidates(path: str | None) -> tuple[str, list[str], list[str]] | None:
    """(principal, worktrees removidas, todas as pastas do repo) de quem contém `path`."""
    root = repo_root_of(path)
    if not root:
        return None
    main = main_repo_of(root)
    gone = [k for k, v in removed().items() if v == main]
    return main, gone, [main, *worktree_paths(main), *gone]


def claude_worktree(cwd: str | None, jsonl: str) -> str | None:
    """A pasta do mesmo repo onde o Claude trabalha. Numa worktree irmã o `cwd` do transcript nunca
    sai da pasta de abertura: o sinal é `cd X`/`git -C X` e o arquivo editado. Nada na principal
    (`cd` ou edição) tira a sessão da worktree: consultar a principal é rotina e faria o rótulo
    alternar a cada comando."""
    last, hits = _claude_tail(jsonl)
    base = last or cwd
    repo = _repo_candidates(base)
    if not base or not repo:
        return last
    main, gone, candidates = repo
    home = _owner(base, candidates)
    for raw, is_cd, line_cwd in hits:
        if line_cwd and _owner(line_cwd, candidates) != home:
            break   # chamada anterior à última troca de `cwd` (EnterWorktree/ExitWorktree)
        p = os.path.normpath(os.path.join(line_cwd or base, os.path.expanduser(raw)))
        if os.path.exists(p):
            owner = _owner(p, candidates)
            if owner and owner != main:
                return owner
        elif is_cd and raw.startswith(("/", "~")) and _of_this_repo(p, main, gone):
            # Pasta absoluta que sumiu: a worktree foi removida. `cd -` e `cd $W` não dizem nada.
            return _owner(p, gone) or p
    return last


_WORKDIR_RE = re.compile(r'"?workdir"?\s*:\s*"(/[^"]+)"')
_CD_RE = re.compile(r'"?cmd"?\s*:\s*"\s*cd\s+(/[^\s&;"]+)')
_PATCH_RE = re.compile(r'\*\*\* (?:Add|Update|Delete) File: (/[^\s\\"]+)')


def _codex_paths(rollout: str) -> list[tuple[str, bool]]:
    """(caminho, é pasta?) citados pelos comandos do Codex, da chamada mais recente para a mais
    antiga. O Codex não troca de pasta: trabalha numa worktree por `workdir`, `cd X &&` ou patch
    com caminho absoluto. Dentro de uma chamada, `cd` vence `workdir` (o comando roda onde entrou),
    e os dois vencem o arquivo do patch."""
    def read(p: str) -> list[tuple[str, bool]]:
        out: list[tuple[str, bool]] = []
        for raw in reversed(_tail_lines(p)):
            if b"_call" not in raw:   # function_call e custom_tool_call
                continue
            try:
                line = json.loads(raw)
            except ValueError:
                continue
            payload = line.get("payload") if isinstance(line, dict) else None
            if not isinstance(payload, dict):
                continue
            if payload.get("type") not in ("function_call", "custom_tool_call"):
                continue
            text = payload.get("arguments") or payload.get("input") or ""
            if not isinstance(text, str):
                continue
            for rx, is_dir in ((_CD_RE, True), (_WORKDIR_RE, True), (_PATCH_RE, False)):
                out.extend((m.group(1), is_dir) for m in reversed(list(rx.finditer(text))))
            if len(out) >= 50:
                break
        return out
    return _cached(rollout, read) or []


def _owner(path: str, candidates: list[str]) -> str | None:
    best = None
    for c in candidates:
        if (path == c or path.startswith(c.rstrip("/") + "/")) and (best is None or len(c) > len(best)):
            best = c
    return best


def codex_cwd(cwd: str, rollout: str) -> str | None:
    """A worktree (ou a principal) do MESMO repo onde o último comando rodou; outro repo não conta."""
    repo = _repo_candidates(cwd)
    if not repo:
        return None
    main, gone, candidates = repo
    for p, is_dir in _codex_paths(rollout):
        if os.path.exists(p):
            owner = _owner(p, candidates)
            if owner:
                return owner
        elif is_dir and _of_this_repo(p, main, gone):
            # Pasta que sumiu: a worktree foi removida. Volta como está para o `locate` marcar
            # "apagada". Arquivo de patch inexistente não conta (um "Delete File" é legítimo).
            return _owner(p, gone) or p
    return None


def _of_this_repo(path: str, main: str, gone: list[str]) -> bool:
    """Pasta sumida só conta se for deste repo: dentro dele, de uma worktree removida dele ou de
    uma irmã no padrão do Hangar (`<repo>-<x>`). Pasta sumida de outro repo não diz nada."""
    if _owner(path, [main, *gone]):
        return True
    parent, name = os.path.split(main.rstrip("/"))
    prefix = parent.rstrip("/") + "/"
    if not path.startswith(prefix):
        return False
    return path[len(prefix):].split("/", 1)[0].startswith(name + "-")


def removed() -> dict[str, str]:
    try:
        data = json.loads(REMOVED_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def locate(provider: str, cwd: str | None, jsonl: str | None) -> Location:
    real = None
    # Sessão que nasceu numa worktree (opção "Nova worktree" da criação, ou o agente aberto nela)
    # fica nela: os sinais do transcript só valem para worktree criada no meio da conversa.
    born_in_worktree = head_info(repo_root_of(cwd))[1]
    try:
        if born_in_worktree:
            pass
        elif jsonl and provider == "claude":
            real = claude_worktree(cwd, jsonl)
        elif jsonl and provider == "codex" and cwd:
            real = codex_cwd(cwd, jsonl)
    except Exception as e:   # transcript torto nunca derruba a listagem inteira
        _log.debug("locate: sem leitura de %s: %s", jsonl, e)
    real = real or cwd
    if not real:
        return Location(None, False, None, False)
    if not os.path.isdir(real):
        # ponytail: pasta sumida que não é a de abertura vira "worktree apagada"; uma subpasta
        # comum apagada também cairia aqui. Afinar só se aparecer falso positivo.
        gone = real != cwd or real in removed()
        return Location(None, False, real if gone else None, gone)
    root = repo_root_of(real)
    branch, wt = head_info(root or real)
    # Só quando o agente saiu do repositório de abertura: sem isso, o git da sessão é o do `cwd`.
    moved = root is not None and root != repo_root_of(cwd)
    return Location(branch, wt, (root or real) if wt else None, False, root if moved else None)


def _git(cwd: str, *args: str, failed: list) -> subprocess.CompletedProcess:
    """`_run` para a situação: timeout de uma worktree lenta degrada o campo dela, nunca derruba a
    lista inteira. A falha fica em `failed` para a situação sair marcada e nunca parecer segura."""
    try:
        return _run(cwd, *args)
    except GitError as e:
        _log.warning("worktrees: git %s em %s falhou: %s", args[0], cwd, e.detail)
        failed.append(args[0])
        return subprocess.CompletedProcess(args, 1, "", e.detail)


def _published_base(cwd: str, base: str, refs: dict[str, str], remotes: list[str],
                    failed: list) -> str:
    matches = [ref for ref in (f"refs/heads/{base}", f"refs/remotes/{base}") if ref in refs]
    if len(matches) > 1:
        failed.append("base")
        return base
    ref = matches[0] if matches else base
    if not matches and any(base.startswith(remote + "/") for remote in remotes):
        ref = f"refs/remotes/{base}"
    if ref.startswith("refs/heads/") and ref in refs:
        upstream = refs[ref]
        if upstream.startswith("refs/remotes/"):
            ref = upstream
        else:
            name = ref.removeprefix("refs/heads/")
            candidates = [f"refs/remotes/{remote}/{name}" for remote in remotes
                          if f"refs/remotes/{remote}/{name}" in refs]
            if len(candidates) == 1:
                ref = candidates[0]
            elif len(candidates) > 1:
                failed.append("base")
    if ref not in refs:
        p = _git(cwd, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}",
                 failed=failed)
        if p.returncode != 0:
            failed.append("base")
    short = next((ref[len(prefix):] for prefix in ("refs/heads/", "refs/remotes/")
                  if ref.startswith(prefix)), ref)
    # Tags têm precedência sobre branches; a abreviação não pode trocar a referência escolhida.
    if f"refs/tags/{short}" in refs or (ref.startswith("refs/remotes/")
                                      and f"refs/heads/{short}" in refs):
        return ref
    return short


def _base_of(path: str, branch: str, main: str, failed: list) -> str | None:
    cwd = path if os.path.isdir(path) else main
    p = _git(cwd, "config", "--get", f"branch.{branch}.hangar-base", failed=failed)
    if p.returncode not in (0, 1):
        failed.append("base")
    base = p.stdout.strip() if p.returncode == 0 else ""
    p = _git(cwd, "for-each-ref", "--format=%(refname)%00%(upstream)",
             "refs/heads/", "refs/remotes/", "refs/tags/", failed=failed)
    refs = dict(line.split("\0", 1) for line in p.stdout.splitlines() if "\0" in line)
    if p.returncode != 0:
        failed.append("base")
    p = _git(cwd, "remote", failed=failed)
    remotes = p.stdout.splitlines() if p.returncode == 0 else []
    if p.returncode != 0:
        failed.append("base")
    if not base:
        p = _git(cwd, "reflog", "show", "--format=%gs", f"refs/heads/{branch}", failed=failed)
        if p.returncode != 0:
            failed.append("base")
        source = next((line.removeprefix("branch: Created from ")
                       for line in reversed(p.stdout.splitlines())
                       if line.startswith("branch: Created from ")), "")
        own = {branch, f"refs/heads/{branch}"}
        own.update(f"{remote}/{branch}" for remote in remotes)
        own.update(f"refs/remotes/{remote}/{branch}" for remote in remotes)
        named = (source.startswith(("refs/heads/", "refs/remotes/"))
                 or f"refs/heads/{source}" in refs
                 or any(source.startswith(remote + "/") for remote in remotes))
        # HEAD e hashes não registram qual branch era o destino; o remoto homônimo é a própria
        # branch publicada. O upstream não entra: `worktree add -b x ../w origin/release` rastreia a base.
        if named and source not in own and f"refs/remotes/{source}" not in own:
            base = source
        else:
            base = head_info(main)[0]
    return _published_base(cwd, base, refs, remotes, failed) if base else None


def is_merged(cwd: str, branch: str, base: str, failed: list | None = None) -> bool:
    failed = [] if failed is None else failed
    start = len(failed)
    # Pontas iguais também podem ser uma worktree recém-criada; upstream apagado não prova merge.
    tips = _git(cwd, "rev-parse", branch, base, failed=failed)
    if tips.returncode != 0:
        failed.append("merged")
        return False
    same = len(set(tips.stdout.split())) == 1
    return (not same and _git(cwd, "merge-base", "--is-ancestor", branch, base,
                             failed=failed).returncode == 0 and len(failed) == start)


def _gitdir_branch(main: str, path: str) -> str | None:
    """Branch de uma worktree cuja pasta sumiu, pelo HEAD guardado em `.git/worktrees/<n>`."""
    for e in Path(main, ".git", "worktrees").glob("*"):
        try:
            g = (e / "gitdir").read_text(encoding="utf-8", errors="replace").strip()
            if os.path.dirname(os.path.normpath(os.path.join(e, g))) == path:
                head = (e / "HEAD").read_text(encoding="utf-8", errors="replace").strip()
                return head[len("ref: refs/heads/"):] if head.startswith("ref: refs/heads/") else None
        except OSError:
            continue
    return None


def _ignored_lost(path: str, main: str, failed: list) -> list[str]:
    """Arquivos ignorados (pastas nunca) que só existem aqui; cópia idêntica à da principal não se perde."""
    p = _git(path, "ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z",
             failed=failed)
    out = []
    for rel in p.stdout.split("\0") if p.returncode == 0 else []:
        if not rel or rel.endswith("/"):
            continue
        twin = Path(main, rel)
        try:
            if twin.is_file() and filecmp.cmp(Path(path, rel), twin, shallow=False):
                continue
        except OSError:
            pass
        out.append(rel)
    return sorted(out)


def _closed_count(path: str, live: set[str]) -> int:
    """Conversas guardadas na pasta, menos as das sessões vivas dentro dela."""
    from app.archive import _contas, conversation_files
    from app.registry import sanitize_cwd
    n = 0
    for _cfg, _rot, base in _contas():
        n += sum(1 for f in conversation_files(base / sanitize_cwd(path)) if os.path.realpath(f) not in live)
    return n


def _inside(s, real: str) -> bool:
    """Por realpath: home atrás de symlink dá dois textos para a mesma pasta."""
    wt = getattr(s, "worktree_path", None)
    if wt and os.path.realpath(wt) == real:
        return True
    cwd = os.path.realpath(s.cwd) if s.cwd else ""
    return cwd == real or cwd.startswith(real.rstrip("/") + "/")


def status(path: str, sessions=(), main: str | None = None, measure: bool = True) -> dict:
    """`main`: o repo principal, quando quem chama já sabe (pasta sumida não diz de quem era).
    `measure=False`: não agenda medir o espaço (quem vai apagar ou só quer a branch)."""
    if main is None:
        root = repo_root_of(path) if os.path.isdir(path) else None
        main = main_repo_of(root) if root else _main_of_missing(path)
    exists = os.path.isdir(path)
    branch = head_info(path)[0] if exists else _gitdir_branch(main, path)
    failed: list = []   # por chamada: a listagem roda status em threads diferentes
    base = _base_of(path, branch, main, failed) if branch else None
    cwd = path if exists else main
    # Nome curto perde para uma tag homônima, e o commit dela pareceria já mesclado.
    ref = f"refs/heads/{branch}" if branch else "HEAD"
    ahead = 0
    if branch and base:
        c = _git(cwd, "rev-list", "--count", f"{base}..{ref}", failed=failed)
        ahead = int(c.stdout.strip() or 0) if c.returncode == 0 else 0
    behind = 0
    commits: list[dict] = []
    if branch and base and branch != base:
        c = _git(cwd, "rev-list", "--count", f"{ref}..{base}", failed=failed)
        behind = int(c.stdout.strip() or 0) if c.returncode == 0 else 0
        commits = _log_commits(cwd, f"{base}..{ref}", 3, failed)
    last = _log_commits(cwd, ref, 1, failed) if exists or branch else []
    dirty_files: list[dict] = []
    if exists:
        s = _git(path, "status", "--porcelain", failed=failed)
        if s.returncode == 0:
            dirty_files = [{"code": line[:2].strip() or "?", "path": line[3:]}
                           for line in s.stdout.splitlines() if line.strip()]
    merged = bool(branch and base and branch != base and is_merged(cwd, ref, base, failed))
    ignored = _ignored_lost(path, main, failed) if exists else []
    real = os.path.realpath(path)
    inside = [s for s in sessions if _inside(s, real)]
    size = disk_usage(path, schedule=measure) if exists else None
    return {
        # A conversa retomada abre na principal, que pode estar noutra branch que não a base.
        "path": path, "repo": main, "exists": exists, "branch": branch, "base": base,
        "main_branch": head_info(main)[0],
        # Leitura que falhou deixa dirty/ignored zerados: a situação nunca pode parecer segura.
        "merged": merged and not failed, "degraded": bool(failed),
        "ahead": ahead, "behind": behind, "dirty": len(dirty_files),
        "dirty_files": dirty_files[:_DIRTY_LIST], "ignored": ignored,
        "last_commit": last[0] if last else None, "commits": commits,
        "created_at": _created_at(main, path),
        "size": size["bytes"] if size else None, "size_biggest": size["biggest"] if size else None,
        "size_pending": exists and size is None,
        "size_error": bool(size and size.get("error")), "size_partial": bool(size and size.get("partial")),
        "sessions": sorted(s.name for s in inside),
        "closed": _closed_count(path, {os.path.realpath(s.jsonl) for s in inside if s.jsonl}),
    }


_DIRTY_LIST = 50


def _log_commits(cwd: str, rev: str, n: int, failed: list) -> list[dict]:
    p = _git(cwd, "log", f"-{n}", "--format=%h%x00%s%x00%ct", rev, "--", failed=failed)
    out = []
    for line in p.stdout.splitlines() if p.returncode == 0 else []:
        sha, _, rest = line.partition("\0")
        subject, _, at = rest.rpartition("\0")
        if sha and at.isdigit():
            out.append({"sha": sha, "subject": subject, "at": int(at)})
    return out


def _admin_dir(main: str, path: str) -> Path | None:
    """`.git/worktrees/<n>` desta worktree, achado pelo ponteiro `gitdir` (vale com a pasta sumida)."""
    for e in Path(main, ".git", "worktrees").glob("*"):
        try:
            g = (e / "gitdir").read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            continue
        if _same_path(os.path.dirname(os.path.normpath(os.path.join(e, g))), path):
            return e
    return None


def _same_path(a: str, b: str) -> bool:
    # No Windows o `gitdir` vem com `/` e o normpath devolve `\`; o git lista em estilo POSIX.
    return os.path.normcase(os.path.realpath(a)) == os.path.normcase(os.path.realpath(b))


def _created_at(main: str, path: str) -> int | None:
    # `commondir` é escrito só no `worktree add`; `gitdir` e `HEAD` mudam depois.
    admin = _admin_dir(main, path)
    try:
        return int((admin / "commondir").stat().st_mtime) if admin else None
    except OSError:
        return None


_SIZE_TTL = 15 * 60
_SIZE_RETRY = 60   # medição que falhou volta a ser tentada depois disso, não a cada leitura
_sizes: dict[str, tuple[float, dict]] = {}
_sizes_running: set[str] = set()
_sizes_lock = threading.Lock()
_size_queue: "queue.Queue[str]" = queue.Queue()
_size_worker: threading.Thread | None = None


def disk_usage(path: str, schedule: bool = True) -> dict | None:
    """Espaço da pasta, do cache; vencido ou ausente agenda a medição e devolve o que houver.
    Medir uma pasta de 14 GB leva segundos: a lista nunca espera por ela."""
    global _size_worker
    with _sizes_lock:
        hit = _sizes.get(path)
        stale = hit is None or time.time() - hit[0] > _SIZE_TTL
        if schedule and stale and path not in _sizes_running:
            _sizes_running.add(path)
            _size_queue.put(path)
            if _size_worker is None:
                # Daemon: restart do backend não espera a medição de uma pasta enorme terminar.
                # ponytail: uma medição por vez para não disputar disco com o resto da máquina.
                _size_worker = threading.Thread(target=_size_loop, name="wt-size", daemon=True)
                _size_worker.start()
    return hit[1] if hit else None


def _size_loop() -> None:
    while True:
        _measure(_size_queue.get())


def _measure(path: str) -> None:
    value: dict = {"bytes": None, "biggest": None, "error": True, "partial": False}
    at = time.time() - _SIZE_TTL + _SIZE_RETRY
    try:
        totals: dict[str, int] = {}
        skipped = [0]
        for entry in os.scandir(path):
            totals[entry.name] = _tree_bytes(entry, skipped)
        biggest = max(totals.items(), key=lambda kv: kv[1], default=None)
        if skipped[0]:
            _log.warning("worktrees: %d pastas ilegíveis ao medir %s; o total é parcial", skipped[0], path)
        value = {"bytes": sum(totals.values()),
                 "biggest": {"name": biggest[0], "bytes": biggest[1]} if biggest else None,
                 "error": False, "partial": bool(skipped[0])}
        at = time.time()
    except Exception:   # a thread é única: qualquer erro que escapasse daqui pararia todas as medições
        _log.exception("worktrees: não medi o espaço de %s", path)
    finally:
        with _sizes_lock:
            _sizes_running.discard(path)
            # Apagada durante a medição: não volta para o cache.
            if os.path.isdir(path):
                _sizes[path] = (at, value)
            else:
                _sizes.pop(path, None)


def _entry_bytes(st: os.stat_result) -> int:
    # Blocos ocupados, como o `du`; o Windows não os informa.
    blocks = getattr(st, "st_blocks", None)
    return blocks * 512 if blocks is not None else st.st_size


def _is_link(entry: os.DirEntry) -> bool:
    # Junction do Windows (node_modules do pnpm) não é symlink, mas `is_dir` sem seguir diz que sim.
    return entry.is_symlink() or entry.is_junction()


def _tree_bytes(entry: os.DirEntry, skipped: list[int]) -> int:
    """`skipped[0]` conta o que não deu para ler: o total sai parcial e a tela mostra "≥"."""
    try:
        if _is_link(entry):
            return 0
        if not entry.is_dir(follow_symlinks=False):
            return _entry_bytes(entry.stat(follow_symlinks=False))
        total = 0
        stack = [entry.path]
        while stack:
            try:
                with os.scandir(stack.pop()) as it:
                    for e in it:
                        if _is_link(e):
                            continue
                        if e.is_dir(follow_symlinks=False):
                            stack.append(e.path)
                        else:
                            total += _entry_bytes(e.stat(follow_symlinks=False))
            except OSError:
                skipped[0] += 1
        return total
    except OSError:
        skipped[0] += 1
        return 0


def _main_of_missing(path: str) -> str:
    """Pasta sumida: procura o repo que ainda a lista entre as worktrees, pelo mapa de remoções,
    pela pasta-irmã `<repo>-<nome>` ou subindo as pastas acima (`<repo>/.claude/worktrees/<nome>`)."""
    mapped = removed().get(path)
    if mapped:
        return mapped
    try:
        cands = sorted(Path(path).parent.iterdir())
    except OSError:
        cands = []
    for cand in [*cands, *Path(path).parents]:
        if (cand / ".git").is_dir() and path in worktree_paths(str(cand)):
            return str(cand)
    return path


def list_all(cwds, sessions, roots=None, repo: str | None = None, measure: bool = True) -> list[dict]:
    """Worktrees de cada repo principal visto nas pastas. `roots`: só repos dentro delas (o
    principal sai do ponteiro `.git` da worktree e pode estar fora da raiz que liberou a pasta).
    `repo`: só o desse repositório (qualquer pasta dele)."""
    # ponytail: várias chamadas git por worktree a cada pedido, sem cache; TTL curto se a tela
    # passar a consultar em intervalo.
    # `repo` não depende das pastas: repo sem sessão aberta também tem worktrees a listar.
    mains: set[str] = set()
    for c in set(cwds) if repo is None else [repo]:
        root = repo_root_of(c) if c else None
        if root:
            # realpath: o mesmo repo por um symlink (`~/hangar` -> `~/projetos/hangar`) apareceria duas vezes.
            mains.add(os.path.realpath(main_repo_of(root)))
    out = []
    for main in sorted(mains):
        if roots is not None and not any(Path(os.path.realpath(main)).is_relative_to(r) for r in roots):
            continue
        paths = worktree_paths(main)
        if paths:
            out.append({"repo": main, "worktrees": [status(p, sessions, main, measure) for p in paths]})
    return out


def fetch(repo: str) -> None:
    p = _run(repo, "fetch", "--all", "--prune", timeout=_FETCH_TIMEOUT)
    if p.returncode != 0:
        raise GitError(409, _scrub(p.stderr.strip()) or "fetch falhou")


_removed_lock = threading.Lock()


def _write_removed(path: str, main: str | None) -> None:
    """`main=None` tira a entrada (remoção que falhou)."""
    with _removed_lock:   # ler-alterar-gravar: duas remoções juntas perderiam uma entrada
        data = removed()
        if main is None:
            data.pop(path, None)
        else:
            data[path] = main
        REMOVED_FILE.parent.mkdir(parents=True, exist_ok=True)
        tmp = REMOVED_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
        atomico.substituir(tmp, REMOVED_FILE)


def record_removed(path: str, main: str) -> None:
    _write_removed(path, main)


def _under(cwd: str | None, path: str) -> bool:
    """`cwd` é `path` ou fica dentro dela, pelo texto ou pelo realpath (home atrás de symlink).
    Pasta sumida: o realpath resolve só a parte que ainda existe, o que basta para o symlink."""
    if not cwd:
        return False
    for c, p in ((cwd, path), (os.path.realpath(cwd), os.path.realpath(path))):
        if c == p or c.startswith(p.rstrip("/") + "/"):
            return True
    return False


def redirect(cwd: str | None) -> str | None:
    """Retomar uma conversa cuja worktree foi apagada abre na pasta principal."""
    if not cwd or os.path.isdir(cwd):
        return cwd
    for path, main in removed().items():
        if _under(cwd, path):
            return main
    return cwd


def relocate_transcripts(path: str, main: str, sessions=()) -> list[tuple[Path, Path]]:
    """Leva `<uuid>.jsonl` e a pasta irmã `<uuid>/` do projeto da worktree pro da principal, em
    todas as contas. Mover, não copiar: a mesma conversa listada duas vezes confunde o Arquivo."""
    from app.archive import _contas, _head_info, conversation_files
    from app.registry import sanitize_cwd
    # O nome da pasta de projeto colide (`repo-x` e `repo/x` viram o mesmo): só sai a conversa
    # que esteve dentro da worktree, e nunca a de uma sessão viva. Primeira OU última pasta: quem
    # entrou por `EnterWorktree` começou na principal e terminou aqui.
    live = {os.path.realpath(s.jsonl) for s in sessions if getattr(s, "jsonl", None)}
    names = {sanitize_cwd(path), sanitize_cwd(os.path.realpath(path))}
    subs = tuple(n + "-" for n in names)   # projetos das subpastas da worktree
    # O Claude indexa pelo cwd do processo, que o getcwd devolve resolvido: o realpath.
    target = sanitize_cwd(os.path.realpath(main))
    moved: list[tuple[Path, Path]] = []
    try:
        for _cfg, _rot, base in _contas():
            dst = base / target
            if not base.is_dir():
                continue
            srcs = sorted(d for d in base.iterdir()
                          if d.name != target and (d.name in names or d.name.startswith(subs)) and d.is_dir())
            for src in srcs:
                for f in sorted(conversation_files(src)):
                    if os.path.realpath(f) in live or not (
                            _under(_head_info(f)[1], path) or _under(claude_cwd(str(f)), path)):
                        continue
                    sid = f.stem
                    if (dst / f.name).exists() or (dst / sid).exists():
                        _log.warning("relocate: %s já existe em %s; fica na worktree", f.name, dst)
                        continue
                    dst.mkdir(parents=True, exist_ok=True)
                    os.replace(f, dst / f.name)
                    moved.append((f, dst / f.name))
                    if (src / sid).is_dir():
                        os.replace(src / sid, dst / sid)
                        moved.append((src / sid, dst / sid))
    except OSError as e:
        # Metade movida é pior que nada: o jsonl sem a pasta irmã perde tool-results e subagentes.
        stuck = _undo(moved)
        raise GitError(500, f"não consegui mover as conversas: {e}" + _stuck_note(stuck)) from None
    return moved


def _undo(moved: list[tuple[Path, Path]]) -> list[Path]:
    """Devolve o que não voltou: segue no projeto da principal."""
    stuck = []
    for src, dst in reversed(moved):
        try:
            os.replace(dst, src)
        except OSError as e:
            _log.error("relocate: não desfez %s -> %s: %s", dst, src, e)
            stuck.append(dst)
    return stuck


def _stuck_note(stuck: list[Path]) -> str:
    if not stuck:
        return ""
    return "; ficaram no projeto da pasta principal: " + ", ".join(p.name for p in stuck)


def delete(repo: str, path: str, sessions, *, confirm: bool = False,
           delete_branch: bool = False) -> dict:
    main = main_repo_of(repo_root_of(repo) or repo)
    real = os.path.realpath(path)
    # O caminho que o git guarda é o canônico: é ele que a lista mostra e as conversas citam.
    path = next((p for p in worktree_paths(main) if p == path or os.path.realpath(p) == real), None)
    if path is None:
        raise GitError(404, "não é uma worktree deste repositório")
    busy = sorted(s.name for s in sessions if _inside(s, real))
    if busy:
        raise GitError(409, "sessão aberta dentro: " + ", ".join(busy))
    st = status(path, (), main, measure=False)
    if st["degraded"]:
        # Leitura que falhou deixa dirty/ignored zerados: apagar assim perderia o que não se viu.
        raise GitError(409, "não consegui ler a worktree; tente de novo")
    if (st["dirty"] or st["ignored"]) and not confirm:
        raise GitError(409, "há arquivos que serão perdidos; confirme")
    # O mapa vem antes de mover: falha no meio nunca deixa conversa movida sem o desvio. Com a
    # pasta ainda de pé ele não age (`redirect` só desvia pasta sumida).
    try:
        record_removed(path, main)
    except OSError as e:
        raise GitError(500, f"não consegui gravar o mapa de remoções: {e}") from None
    moved: list[tuple[Path, Path]] = []
    try:
        moved = relocate_transcripts(path, main, sessions)
        # `remove` aceita pasta já sumida e leva só o registro desta; o `prune` levaria o de todas.
        remove_worktree(main, path, force=confirm)
        if path in worktree_paths(main):
            raise GitError(409, "a worktree continua registrada (trancada?)")
    except GitError as e:
        stuck = _undo(moved)
        try:
            _write_removed(path, None)
        except OSError as w:
            _log.error("worktrees: não tirei %s do mapa de remoções: %s", path, w)
        raise GitError(409, e.detail + _stuck_note(stuck)) from None
    with _sizes_lock:
        _sizes.pop(path, None)
    branch_deleted = False
    if st["branch"] and (st["merged"] or delete_branch):
        try:
            b = _run(main, "branch", "-D", st["branch"])
            branch_deleted = b.returncode == 0
            if not branch_deleted:
                _log.warning("worktrees: branch %s ficou: %s", st["branch"], b.stderr.strip())
        except GitError as e:   # a worktree já saiu; a branch que ficou vai em branch_deleted
            _log.warning("worktrees: branch %s ficou: %s", st["branch"], e.detail)
    return {"removed": path, "branch_deleted": branch_deleted, "moved": len(moved)}


def delete_merged(repo: str, sessions, paths: list[str] | None = None, confirm: bool = False,
                  lossy: list[str] | None = None) -> list[str]:
    """Mescladas, sem sessão e lidas sem falha. Sem `confirm`, só as que não perdem nada. `paths`
    restringe às que a tela mostrou: o lote nunca leva uma worktree que a pessoa não viu. `lossy`
    são as que a tela mostrou perdendo algo: só essas saem com perda, nunca uma que sujou depois."""
    main = main_repo_of(repo_root_of(repo) or repo)
    wanted = {os.path.realpath(p) for p in paths} if paths is not None else None
    accepted = {os.path.realpath(p) for p in lossy or ()}
    out = []
    for path in worktree_paths(main):
        if wanted is not None and os.path.realpath(path) not in wanted:
            continue
        st = status(path, sessions, main, measure=False)
        if not st["merged"] or st["degraded"] or st["sessions"]:
            continue
        loses = bool(st["dirty"] or st["ignored"])
        if not loses or confirm and os.path.realpath(path) in accepted:
            try:
                delete(main, path, sessions, confirm=loses)
            except GitError as e:
                if out:   # as anteriores já saíram: o erro tem que dizer quais
                    raise GitError(e.status, f"{e.detail} (já removidas: {', '.join(out)})") from None
                raise
            out.append(path)
    return out
