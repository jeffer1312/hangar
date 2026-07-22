"""Testes de _parse_status_branch: parse do `git status --porcelain=v1 --branch`."""
from app.git_ops import _parse_status_branch, git_summary


def test_ahead_limpo():
    assert _parse_status_branch("## main...origin/main [ahead 5]\n") == {
        "dirty": 0, "ahead": 5, "behind": 0}


def test_ahead_behind_e_sujo():
    out = "## main...origin/main [ahead 2, behind 1]\n M a.py\n?? b\n"
    assert _parse_status_branch(out) == {"dirty": 2, "ahead": 2, "behind": 1}


def test_sincronizado():
    assert _parse_status_branch("## main...origin/main\n") == {
        "dirty": 0, "ahead": 0, "behind": 0}


def test_rename_conta_uma_linha():
    out = "## main...origin/main\nR  old -> new\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": 0, "behind": 0}


def test_sem_upstream():
    out = "## feature\n M a.py\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_detached():
    out = "## HEAD (no branch)\n M a.py\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_upstream_gone():
    assert _parse_status_branch("## main...origin/main [gone]\n") == {
        "dirty": 0, "ahead": None, "behind": None}


def test_sem_commits():
    out = "## No commits yet on main\n?? a\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_saida_vazia_ou_invalida():
    assert _parse_status_branch("") is None
    assert _parse_status_branch("lixo\n") is None


def test_git_summary_nao_repo(tmp_path):
    # sem .git -> None sem nem forkar git.
    assert git_summary(str(tmp_path)) is None
    assert git_summary(None) is None


def test_git_summary_timeout_vira_none(tmp_path, monkeypatch):
    # _run levanta GitError no timeout (repo enorme/NFS); git_summary TEM que virar None — nunca
    # deixar a exceção subir (senão 500 no /api/sessions + morte do SSE de TODAS as sessões).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    def boom(*a, **k):
        raise git_ops.GitError(500, "timeout")

    monkeypatch.setattr(git_ops, "_run", boom)
    assert git_ops.git_summary(str(tmp_path)) is None
