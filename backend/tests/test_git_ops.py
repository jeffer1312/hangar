"""Cobertura do git_ops: list/switch/action contra um repo temporario + rejeicoes e erro de binario."""
import pytest

from app import git_ops
from app.git_ops import GitError


def _repo(tmp_path):
    d = str(tmp_path)
    for args in (
        ["init", "-q", "-b", "main"],
        ["config", "user.email", "t@t"],
        ["config", "user.name", "t"],
        ["commit", "-q", "--allow-empty", "-m", "init"],
        ["branch", "feature"],
    ):
        git_ops._run(d, *args)
    return d


def test_list_branches(tmp_path):
    info = git_ops.list_branches(_repo(tmp_path))
    assert info["current"] == "main"
    assert set(info["branches"]) == {"main", "feature"}
    assert info["dirty"] is False


def test_list_branches_dirty(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "novo.txt").write_text("x")  # arquivo untracked -> tree suja
    assert git_ops.list_branches(d)["dirty"] is True


def test_switch_branch_ok(tmp_path):
    d = _repo(tmp_path)
    assert git_ops.switch_branch(d, "feature")["current"] == "feature"
    assert git_ops.list_branches(d)["current"] == "feature"


@pytest.mark.parametrize("bad", ["nope", "main; rm -rf /", "--orphan x"])
def test_switch_branch_rejects_invalid(tmp_path, bad):
    with pytest.raises(GitError) as e:
        git_ops.switch_branch(_repo(tmp_path), bad)
    assert e.value.status == 400  # nao esta na lista -> sem injecao/flag


def test_git_action_status_ok(tmp_path):
    assert git_ops.git_action(_repo(tmp_path), "status")["ok"] is True


def test_git_action_invalid(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.git_action(_repo(tmp_path), "rm -rf")  # fora da allowlist
    assert e.value.status == 400


def _with_remote(tmp_path):
    """Repo local + um segundo repo como 'origin' (com uma branch so-remota), ja com fetch feito."""
    d = _repo(tmp_path)
    (tmp_path / "remote").mkdir()   # git -C exige o path existente
    rd = str(tmp_path / "remote")
    for args in (
        ["init", "-q", "-b", "main"],
        ["config", "user.email", "t@t"],
        ["config", "user.name", "t"],
        ["commit", "-q", "--allow-empty", "-m", "r"],
        ["branch", "only-remote"],
    ):
        git_ops._run(rd, *args)
    git_ops._run(d, "remote", "add", "origin", rd)
    assert git_ops.git_action(d, "fetch")["ok"] is True
    return d


def test_list_branches_remotes_dedup(tmp_path):
    info = git_ops.list_branches(_with_remote(tmp_path))
    assert "only-remote" in info["remotes"]        # remota sem local -> aparece
    assert "main" not in info["remotes"]            # ja tem local -> dedup pelo nome curto
    assert "only-remote" not in info["branches"]    # ainda nao e local


def test_switch_remote_dwim_creates_local(tmp_path):
    d = _with_remote(tmp_path)
    assert git_ops.switch_branch(d, "only-remote")["current"] == "only-remote"
    assert "only-remote" in git_ops.list_branches(d)["branches"]  # DWIM criou a local


def test_run_git_not_found(tmp_path, monkeypatch):
    # FileNotFoundError (git ausente) -> GitError 500, nao traceback cru.
    def boom(*a, **k):
        raise FileNotFoundError("no git")
    monkeypatch.setattr(git_ops.subprocess, "run", boom)
    with pytest.raises(GitError) as e:
        git_ops._run(str(tmp_path), "status")
    assert e.value.status == 500
