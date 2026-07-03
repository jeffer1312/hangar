import subprocess

# Git pela sessao (cwd da sessao tmux). Tudo via argv list -> nunca string de shell (sem injecao).
# So acoes fixas: listar/trocar branch + status/pull. Comando git arbitrario NAO e exposto (o usuario
# digita direto na sessao claude se precisar) -> sem superficie de RCE.
_TIMEOUT = 20


class GitError(Exception):
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _run(cwd: str, *args: str) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            ["git", "-C", cwd, *args],
            capture_output=True, text=True, timeout=_TIMEOUT,
        )
    except FileNotFoundError:
        raise GitError(500, "git nao encontrado")
    except subprocess.TimeoutExpired:
        raise GitError(504, "git timeout")
    except OSError as e:
        # ex: sem permissao de executar git -> erro limpo em vez de 500 com traceback.
        raise GitError(500, f"git falhou: {e}")


def list_branches(cwd: str) -> dict:
    """Branches locais + remotas (sem local correspondente) + a atual + se a working tree esta
    suja (pro front avisar antes do checkout: `git switch` carrega mudancas nao-conflitantes pra
    outra branch silenciosamente). As remotas vem pelo nome curto (sem o prefixo do remote): trocar
    pra uma delas faz o DWIM do `git switch` (cria a local rastreando origin/<nome>). Ficam
    defasadas ate um fetch."""
    # --sort=-committerdate: mais recentemente commitada primeiro (nao alfabetico).
    p = _run(cwd, "branch", "--sort=-committerdate", "--format=%(refname:short)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git branch falhou").strip() or "git branch falhou")
    branches = [b.strip() for b in p.stdout.splitlines() if b.strip()]
    local = set(branches)

    # Remotas: -r lista refs/remotes/*. Nome curto = tira o primeiro segmento (o nome do remote).
    # Ignora o 'origin/HEAD' simbolico (refname:short = so 'origin', sem '/') e dedup pelo nome
    # curto contra as locais (uma remota que ja tem local nao precisa aparecer duas vezes).
    r = _run(cwd, "branch", "-r", "--sort=-committerdate", "--format=%(refname:short)")
    remotes: list[str] = []
    seen: set[str] = set()
    if r.returncode == 0:
        for full in r.stdout.splitlines():
            full = full.strip()
            if not full or "/" not in full:   # '' ou 'origin' (o HEAD simbolico) -> ignora
                continue
            short = full.split("/", 1)[1]
            if short == "HEAD" or short in local or short in seen:
                continue
            seen.add(short)
            remotes.append(short)

    cur = _run(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    current = cur.stdout.strip() if cur.returncode == 0 else None
    st = _run(cwd, "status", "--porcelain")
    dirty = st.returncode == 0 and bool(st.stdout.strip())
    return {"current": current, "branches": branches, "remotes": remotes, "dirty": dirty}


def switch_branch(cwd: str, branch: str) -> dict:
    """Troca pra uma branch EXISTENTE — local OU remota (nome curto -> DWIM cria a local
    rastreando o remote). Valida contra a lista real (rejeita injecao/typo/flag-like). Falha (tree
    suja, nome curto ambiguo entre remotes, etc) volta como 409 com o stderr do git."""
    info = list_branches(cwd)
    valid = set(info["branches"]) | set(info["remotes"])
    if branch not in valid:
        raise GitError(400, "branch inexistente")
    p = _run(cwd, "switch", branch)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "switch falhou").strip() or "switch falhou")
    return {"current": branch, "output": (p.stdout + p.stderr).strip()}


# Allowlist de acoes sem argumento -> argv fixo, zero entrada do usuario no comando.
_ACTIONS = {
    "status": ["status", "--short", "--branch"],
    "pull": ["pull", "--ff-only"],
    "fetch": ["fetch", "--all", "--prune"],
    # stash guarda TUDO (inclui untracked) -> deixa a tree limpa pra trocar de branch; pop reaplica.
    "stash": ["stash", "push", "--include-untracked"],
    "stash-pop": ["stash", "pop"],
}


def git_action(cwd: str, action: str) -> dict:
    argv = _ACTIONS.get(action)
    if argv is None:
        raise GitError(400, "acao invalida")
    p = _run(cwd, *argv)
    return {"ok": p.returncode == 0, "output": (p.stdout + p.stderr).strip()}


def changed_files(cwd: str) -> list[dict]:
    """Arquivos com mudanca nao-commitada (git status --porcelain). Cada item: `path`, `code` (os 2
    chars XY do porcelain: ' M', 'M ', '??', 'A '...) e `staged`. So leitura."""
    p = _run(cwd, "status", "--porcelain")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git status falhou").strip() or "git status falhou")
    out = []
    for line in p.stdout.splitlines():
        if len(line) < 4:
            continue
        code, path = line[:2], line[3:]
        if " -> " in path:                      # rename/copy: "old -> new" -> usa o novo path
            path = path.split(" -> ", 1)[1]
        out.append({"path": path, "code": code, "staged": code[0] not in " ?"})
    return out


def _changed_map(cwd: str) -> dict:
    return {f["path"]: f for f in changed_files(cwd)}


def file_diff(cwd: str, path: str) -> dict:
    """Diff (staged + unstaged, vs HEAD) de UM arquivo. Valida o path contra a lista real de
    alterados (rejeita traversal/injecao/flag-like); `--` separa o path de flags no argv."""
    files = _changed_map(cwd)
    if path not in files:
        raise GitError(400, "arquivo nao esta na lista de alterados")
    if files[path]["code"] == "??":             # untracked: HEAD nao tem -> mostra tudo como adicao
        p = _run(cwd, "diff", "--no-index", "--", "/dev/null", path)
    else:
        p = _run(cwd, "diff", "HEAD", "--", path)
    # git diff sai 1 quando HA diferenca (normal) -> so 128+ e erro real (path invalido, etc)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    return {"path": path, "diff": p.stdout}


def discard_file(cwd: str, path: str) -> dict:
    """DESTRUTIVO: descarta a mudanca nao-commitada de UM arquivo. Valida o path contra a lista real.
    Untracked -> remove o arquivo do disco (git clean); tracked -> git restore ao HEAD (staged+tree)."""
    files = _changed_map(cwd)
    if path not in files:
        raise GitError(400, "arquivo nao esta na lista de alterados")
    if files[path]["code"] == "??":
        p = _run(cwd, "clean", "-f", "--", path)
    else:
        p = _run(cwd, "restore", "--staged", "--worktree", "--source=HEAD", "--", path)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "descartar falhou").strip() or "descartar falhou")
    return {"ok": True, "path": path}


if __name__ == "__main__":
    # Self-check: repo temp, cria branch, valida switch + rejeicao + allowlist. Sem framework.
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        _run(d, "init", "-q", "-b", "main")
        _run(d, "config", "user.email", "t@t")
        _run(d, "config", "user.name", "t")
        _run(d, "commit", "-q", "--allow-empty", "-m", "init")
        _run(d, "branch", "feature")

        info = list_branches(d)
        assert info["current"] == "main", info
        assert set(info["branches"]) == {"main", "feature"}, info

        assert switch_branch(d, "feature")["current"] == "feature"
        assert list_branches(d)["current"] == "feature"

        for bad in ["nope", "main; rm -rf /", "--orphan x"]:
            try:
                switch_branch(d, bad)
                raise AssertionError(f"deveria rejeitar: {bad!r}")
            except GitError as e:
                assert e.status == 400, e.status

        assert git_action(d, "status")["ok"] is True
        try:
            git_action(d, "rm -rf")
            raise AssertionError("acao invalida deveria falhar")
        except GitError as e:
            assert e.status == 400

        # Remotas: um segundo repo vira 'origin'; fetch traz as refs. Valida dedup + DWIM + fetch.
        with tempfile.TemporaryDirectory() as rd:
            _run(rd, "init", "-q", "-b", "main")
            _run(rd, "config", "user.email", "t@t")
            _run(rd, "config", "user.name", "t")
            _run(rd, "commit", "-q", "--allow-empty", "-m", "r")
            _run(rd, "branch", "only-remote")
            _run(d, "remote", "add", "origin", rd)

            assert git_action(d, "fetch")["ok"] is True
            info = list_branches(d)
            assert "only-remote" in info["remotes"], info      # remota sem local -> aparece
            assert "main" not in info["remotes"], info          # ja tem local -> dedup
            assert "only-remote" not in info["branches"], info  # ainda nao e local
            # switch DWIM: cria a local rastreando origin/only-remote
            assert switch_branch(d, "only-remote")["current"] == "only-remote"
            assert "only-remote" in list_branches(d)["branches"], "DWIM devia criar local"

        print("git_ops self-check OK")
