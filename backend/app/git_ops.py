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


def list_branches(cwd: str) -> dict:
    """Branches locais + a atual."""
    p = _run(cwd, "branch", "--format=%(refname:short)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git branch falhou").strip() or "git branch falhou")
    branches = [b.strip() for b in p.stdout.splitlines() if b.strip()]
    cur = _run(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    current = cur.stdout.strip() if cur.returncode == 0 else None
    return {"current": current, "branches": branches}


def switch_branch(cwd: str, branch: str) -> dict:
    """Troca pra uma branch EXISTENTE. Valida contra a lista (rejeita injecao/typo/flag-like).
    Falha (tree suja, etc) volta como 409 com o stderr do git."""
    valid = list_branches(cwd)["branches"]
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
}


def git_action(cwd: str, action: str) -> dict:
    argv = _ACTIONS.get(action)
    if argv is None:
        raise GitError(400, "acao invalida")
    p = _run(cwd, *argv)
    return {"ok": p.returncode == 0, "output": (p.stdout + p.stderr).strip()}


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

        print("git_ops self-check OK")
