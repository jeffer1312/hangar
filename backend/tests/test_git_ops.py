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


def _repo_with_file(tmp_path):
    """Repo com um arquivo tracked commitado (pra exercitar changed_files/diff/discard/stash)."""
    d = _repo(tmp_path)
    f = tmp_path / "tracked.txt"
    f.write_text("linha original\n")
    git_ops._run(d, "add", "tracked.txt")
    git_ops._run(d, "commit", "-q", "-m", "add tracked")
    return d, f


def test_changed_files_lists_tracked_and_untracked(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha modificada\n")            # tracked modificado
    (tmp_path / "novo.txt").write_text("x")        # untracked
    files = {c["path"]: c for c in git_ops.changed_files(d)}
    assert files["tracked.txt"]["code"] == " M" and files["tracked.txt"]["staged"] is False
    assert files["novo.txt"]["code"] == "??"


def test_file_diff_rejects_unlisted_path(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    for bad in ("nao-existe.txt", "../../etc/passwd", "--output=x"):
        with pytest.raises(GitError) as e:
            git_ops.file_diff(d, bad)              # so paths na lista de alterados passam
        assert e.value.status == 400


def test_file_diff_shows_edits(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha modificada\n")
    out = git_ops.file_diff(d, "tracked.txt")["diff"]
    assert "linha modificada" in out and "linha original" in out


def test_file_diff_untracked_shows_content(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "novo.txt").write_text("conteudo novo\n")
    assert "conteudo novo" in git_ops.file_diff(d, "novo.txt")["diff"]


def test_discard_tracked_restores_head(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("estragado\n")
    git_ops.discard_file(d, "tracked.txt")
    assert f.read_text() == "linha original\n"     # voltou ao HEAD
    assert "tracked.txt" not in git_ops._changed_map(d)


def test_discard_untracked_removes_file(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    novo = tmp_path / "novo.txt"
    novo.write_text("lixo")
    git_ops.discard_file(d, "novo.txt")
    assert not novo.exists()


def test_discard_rejects_unlisted_path(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    with pytest.raises(GitError) as e:
        git_ops.discard_file(d, "../../etc/passwd")
    assert e.value.status == 400


def test_stash_and_pop_roundtrip(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("mudanca pendente\n")
    assert git_ops.git_action(d, "stash")["ok"] is True
    assert git_ops.changed_files(d) == []          # tree limpa apos stash
    assert git_ops.git_action(d, "stash-pop")["ok"] is True
    assert f.read_text() == "mudanca pendente\n"    # reaplicado


def test_run_git_not_found(tmp_path, monkeypatch):
    # FileNotFoundError (git ausente) -> GitError 500, nao traceback cru.
    def boom(*a, **k):
        raise FileNotFoundError("no git")
    monkeypatch.setattr(git_ops.subprocess, "run", boom)
    with pytest.raises(GitError) as e:
        git_ops._run(str(tmp_path), "status")
    assert e.value.status == 500
