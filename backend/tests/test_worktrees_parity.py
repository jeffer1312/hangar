"""Lista de worktrees: o Rust (menos `git` por worktree, em paralelo) sai igual ao `worktrees.py`."""
import json
import os
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest
from app import archive, git_ops, worktrees
from app.registry import sanitize_cwd

from tests.test_workspace_parity import _BINARY


def rust(operation, **arguments):
    result = subprocess.run([str(_BINARY)], input=json.dumps({"op": operation, "args": arguments},
        default=os.fspath, ensure_ascii=False) + "\n", text=True, encoding="utf-8", capture_output=True, check=True)
    return json.loads(result.stdout)


def git(cwd, *args):
    p = git_ops._run(str(cwd), *args)
    assert p.returncode == 0, p.stderr


@pytest.fixture
def scene(tmp_path, monkeypatch):
    monkeypatch.setenv("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+00:00")
    monkeypatch.setenv("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+00:00")
    origin = tmp_path / "origin.git"
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-b", "main")
    git(repo, "config", "user.name", "Teste")
    git(repo, "config", "user.email", "teste@example.invalid")
    (repo / ".gitignore").write_text(".env\nsame.env\nbuild/\n", encoding="utf-8")
    (repo / "a.txt").write_text("a\n", encoding="utf-8")
    (repo / "same.env").write_text("igual\n", encoding="utf-8")
    git(repo, "add", ".gitignore", "a.txt")
    git(repo, "commit", "-m", "Inicial")
    git(tmp_path, "init", "--bare", "-b", "main", str(origin))
    git(repo, "remote", "add", "origin", str(origin))

    def add(name, branch):
        path = tmp_path / f"repo-{name}"
        git(repo, "worktree", "add", "-b", branch, str(path))
        return path

    merged = add("merged", "feat-merged")
    (merged / "m.txt").write_text("m\n", encoding="utf-8")
    git(merged, "add", "m.txt")
    git(merged, "commit", "-m", "Mesclada")
    git(repo, "merge", "--no-ff", "-m", "Merge feat-merged", "feat-merged")

    dirty = add("dirty", "feat-dirty")
    (dirty / "d.txt").write_text("d\n", encoding="utf-8")
    git(dirty, "add", "d.txt")
    git(dirty, "commit", "-m", "Primeiraç")
    (dirty / "d.txt").write_text("mudou\n", encoding="utf-8")
    (dirty / "novo.txt").write_text("n\n", encoding="utf-8")
    (dirty / ".env").write_text("segredo\n", encoding="utf-8")
    (dirty / "same.env").write_text("igual\n", encoding="utf-8")
    (dirty / "build").mkdir()
    (dirty / "build/x.o").write_text("o\n", encoding="utf-8")

    add("fresh", "fresh")

    squash = add("squash", "feat-squash")
    (squash / "s.txt").write_text("s\n", encoding="utf-8")
    git(squash, "add", "s.txt")
    git(squash, "commit", "-m", "Squash")
    git(squash, "push", "-u", "origin", "feat-squash")
    git(repo, "push", "origin", "--delete", "feat-squash")
    git(repo, "fetch", "--prune", "origin")

    based = add("based", "feat-based")
    git(repo, "config", "branch.feat-based.hangar-base", "feat-dirty")

    gone = add("gone", "feat-gone")
    shutil.rmtree(gone)

    detached = add("detached", "tmp-detached")
    git(detached, "checkout", "--detach")

    projects = tmp_path / "projects"
    live = projects / sanitize_cwd(str(dirty)) / "d0000003-0000-4000-8000-000000000000.jsonl"
    live.parent.mkdir(parents=True)
    live.write_text("{}\n", encoding="utf-8")
    (live.parent / "d0000001-0000-4000-8000-000000000000.jsonl").write_text("{}\n", encoding="utf-8")
    (projects / sanitize_cwd(str(merged))).mkdir()
    (projects / sanitize_cwd(str(merged)) / "d0000002-0000-4000-8000-000000000000.jsonl").write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(archive, "_contas", lambda config_dir=None: [(None, "", projects)])
    sessions = [
        {"name": "dentro", "cwd": str(dirty / "build"), "worktree_path": None, "jsonl": str(live)},
        {"name": "pela-worktree", "cwd": str(repo), "worktree_path": str(based), "jsonl": None},
        {"name": "fora", "cwd": str(tmp_path), "worktree_path": None, "jsonl": None},
    ]
    return SimpleNamespace(root=tmp_path, repo=repo, gone=gone, dirty=dirty, sessions=sessions,
                           projects=projects)


def _infos(scene):
    return [SimpleNamespace(**s) for s in scene.sessions]


def test_list_matches_python(scene):
    cwds = [str(scene.dirty), str(scene.repo), str(scene.root)]
    expected = worktrees.list_all(cwds, _infos(scene), [scene.root], None, False)
    got = rust("list_worktrees", cwds=cwds, sessions=scene.sessions, roots=[str(scene.root)],
               repo=None, measure=False, project_bases=[str(scene.projects)])
    assert got["ok"], got
    assert got["result"] == json.loads(json.dumps(expected))
    by = {Path(w["path"]).name: w for w in got["result"][0]["worktrees"]}
    # O cenário cobre cada ramo da situação, para a igualdade acima valer alguma coisa.
    assert by["repo-merged"]["merged"] and not by["repo-squash"]["merged"]
    assert not by["repo-fresh"]["merged"] and not by["repo-dirty"]["merged"]
    assert by["repo-dirty"]["dirty"] == 2 and by["repo-dirty"]["ignored"] == [".env"]
    assert by["repo-dirty"]["sessions"] == ["dentro"] and by["repo-dirty"]["closed"] == 1
    assert by["repo-merged"]["closed"] == 1 and by["repo-based"]["sessions"] == ["pela-worktree"]
    assert by["repo-based"]["base"] == "feat-dirty" and by["repo-based"]["behind"] == 1
    assert by["repo-dirty"]["ahead"] == 1 and by["repo-dirty"]["last_commit"]["subject"] == "Primeiraç"
    assert not by["repo-gone"]["exists"] and by["repo-gone"]["branch"] == "feat-gone"
    assert by["repo-detached"]["branch"] is None and by["repo-detached"]["last_commit"]


def test_list_by_repo_and_outside_roots(scene):
    expected = worktrees.list_all([], _infos(scene), None, str(scene.dirty), False)
    got = rust("list_worktrees", cwds=[], sessions=scene.sessions, repo=str(scene.dirty),
               measure=False, project_bases=[str(scene.projects)])
    assert got == {"ok": True, "result": json.loads(json.dumps(expected))}
    assert rust("list_worktrees", cwds=[str(scene.repo)], sessions=[],
                roots=[str(scene.root / "nada")], measure=False) == {"ok": True, "result": []}


@pytest.mark.parametrize("which", ["gone", "dirty"])
def test_detail_matches_python(scene, which):
    path = str(getattr(scene, which))
    expected = worktrees.status(path, _infos(scene), measure=False)
    got = rust("worktree_status", path=path, sessions=scene.sessions, measure=False,
               project_bases=[str(scene.projects)])
    assert got == {"ok": True, "result": json.loads(json.dumps(expected))}
