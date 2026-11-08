import pytest
from app import worktrees


def _repo(path):
    path.mkdir(parents=True, exist_ok=True)
    d = str(path)
    return d


@pytest.fixture(autouse=True)
def _isolated_removed(tmp_path, monkeypatch):
    monkeypatch.setattr(worktrees, "REMOVED_FILE", tmp_path / "removidas.json")


def test_tree_bytes_skips_windows_junctions(monkeypatch):
    """No Windows a junction (node_modules do pnpm) tem `is_symlink()` falso e `is_dir` verdadeiro."""
    import os
    from types import SimpleNamespace

    def entry(path, *, is_dir=False, junction=False):
        return SimpleNamespace(name=path.rsplit("/", 1)[-1], path=path,
                               is_symlink=lambda: False, is_junction=lambda: junction,
                               is_dir=lambda follow_symlinks=True: is_dir,
                               stat=lambda follow_symlinks=True: SimpleNamespace(st_size=100))

    tree = {"/w/pkg": [entry("/w/pkg/a.js"), entry("/w/pkg/link", is_dir=True, junction=True)],
            "/w/link": [entry("/w/link/big.bin")],
            "/w/pkg/link": [entry("/w/pkg/link/big.bin")]}

    class _It(list):
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr(os, "scandir", lambda p: _It(tree[p]))
    skipped = [0]
    assert worktrees._tree_bytes(entry("/w/link", is_dir=True, junction=True), skipped) == 0
    assert worktrees._tree_bytes(entry("/w/pkg", is_dir=True), skipped) == 100
    assert skipped == [0]


def test_redirect_through_symlink(tmp_path):
    main = _repo(tmp_path / "repo")
    (tmp_path / "link").symlink_to(tmp_path)
    worktrees.record_removed(str(tmp_path / "repo-x"), main)
    assert worktrees.redirect(str(tmp_path / "link" / "repo-x" / "sub")) == main
    assert worktrees.redirect(str(tmp_path / "outra")) == str(tmp_path / "outra")
