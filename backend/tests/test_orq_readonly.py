"""Proteção real do código, preservando relatórios e o lançador no pane."""
import asyncio
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from unittest.mock import Mock

import pytest

import providers_bridge


def _prepare(cwd, *, home=None, env=None):
    return providers_bridge.local("provider-info", op="read-only", cwd=str(cwd),
                                  home=str(home or Path.home()), env=dict(os.environ) if env is None else env)


@pytest.mark.skipif(sys.platform != "linux" or not shutil.which("bwrap"),
                    reason="prova Linux exige bubblewrap instalado")
def test_child_cannot_write_worktree_or_git_but_can_save_report(tmp_path, hangar_server_bin):
    repo = tmp_path / "repo"
    worktree = tmp_path / "worktree"
    subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
    (repo / "source.txt").write_text("base")
    subprocess.run(["git", "-C", str(repo), "add", "source.txt"], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test", "-c",
                    "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                    "commit", "--allow-empty", "-m", "initial"], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(repo), "worktree", "add", "--detach", str(worktree)],
                   check=True, capture_output=True)
    source = worktree / "source.txt"
    source.write_text("original")
    snapshot = subprocess.check_output(
        ["git", "-C", str(worktree), "-c", "user.name=Test", "-c",
         "user.email=test@example.invalid", "stash", "create"], text=True).strip()
    subprocess.run(["git", "-C", str(worktree), "stash", "store", snapshot],
                   check=True, capture_output=True)
    alias = tmp_path / "alias"
    alias.symlink_to(worktree, target_is_directory=True)
    prefix = _prepare(alias)
    gitdir = Path(subprocess.check_output(
        ["git", "-C", str(worktree), "rev-parse", "--absolute-git-dir"], text=True).strip())
    report = tmp_path / "report.txt"
    probe = """import pathlib,subprocess,sys
source,gitdir,common,report,copy=map(pathlib.Path,sys.argv[1:6])
snapshot=sys.argv[6]
assert source.read_text() == 'original'
for path in (source, gitdir/'forbidden', common/'forbidden'):
    try: path.write_text('changed')
    except OSError: pass
    else: raise AssertionError(f'write allowed: {path}')
subprocess.run(['git','clone','--no-hardlinks','--no-checkout',str(source.parent),str(copy)],check=True)
subprocess.run(['git','-C',str(copy),'checkout','--detach',snapshot],check=True)
assert (copy/'source.txt').read_text() == 'original'
(copy/'source.txt').write_text('changed copy')
assert source.read_text() == 'original'
report.write_text('verified')
"""
    child = shlex.join([sys.executable, "-c", probe, str(alias / "source.txt"),
                        str(gitdir), str(repo / ".git"), str(report), str(tmp_path / "copy"), snapshot])
    result = subprocess.run([*prefix, "/bin/sh", "-c", child], capture_output=True,
                            text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert source.read_text() == "original"
    assert report.read_text() == "verified"


def test_missing_support_refuses_before_process(tmp_path, hangar_server_bin):
    message = "instale bwrap" if sys.platform == "linux" else "Linux"
    with pytest.raises(ValueError, match=message):
        _prepare(tmp_path, env={"PATH": str(tmp_path)})


def _fake_programs(tmp_path):
    programs = tmp_path / "bin"
    programs.mkdir()
    marker = tmp_path / "bwrap-called"
    for name, script in {
        "git": "#!/bin/sh\nexit 128\n",
        "bwrap": f"#!/bin/sh\n: > '{marker}'\necho 'user namespaces disabled' >&2\nexit 1\n",
    }.items():
        executable = programs / name
        executable.write_text(script, encoding="utf-8")
        executable.chmod(0o700)
    return {"PATH": str(programs)}, marker


@pytest.mark.skipif(sys.platform != "linux", reason="bubblewrap é exclusivo do Linux")
def test_refused_probe_has_no_unprotected_fallback(tmp_path, hangar_server_bin):
    env, marker = _fake_programs(tmp_path)
    with pytest.raises(ValueError, match="user namespaces disabled"):
        _prepare(tmp_path, env=env)
    assert marker.exists()


@pytest.mark.skipif(sys.platform != "linux", reason="bubblewrap é exclusivo do Linux")
def test_home_is_not_made_read_only(tmp_path, hangar_server_bin):
    env, marker = _fake_programs(tmp_path)
    with pytest.raises(ValueError, match="HOME inteira"):
        _prepare(tmp_path, home=tmp_path, env=env)
    assert not marker.exists()


@pytest.mark.skipif(sys.platform != "linux", reason="bubblewrap é exclusivo do Linux")
def test_runtime_inside_code_is_refused(tmp_path, hangar_server_bin):
    env, marker = _fake_programs(tmp_path)
    env["CODEX_HOME"] = str(tmp_path / ".codex")
    with pytest.raises(ValueError, match="runtime"):
        _prepare(tmp_path, home=tmp_path.parent, env=env)
    assert not marker.exists()


@pytest.mark.skipif(sys.platform != "linux", reason="bubblewrap é exclusivo do Linux")
def test_file_is_refused_before_the_probe(tmp_path, hangar_server_bin):
    env, marker = _fake_programs(tmp_path)
    source = tmp_path / "source.txt"
    source.write_text("conteúdo", encoding="utf-8")
    with pytest.raises(ValueError, match="diretório de trabalho existente"):
        _prepare(source, env=env)
    assert not marker.exists()


@pytest.mark.skipif(sys.platform != "linux", reason="bubblewrap é exclusivo do Linux")
def test_tilde_runtime_inside_code_is_refused_before_the_probe(tmp_path, hangar_server_bin):
    env, marker = _fake_programs(tmp_path)
    source = tmp_path / "repo"
    source.mkdir()
    env["CODEX_HOME"] = "~/repo/ausente/../.codex"
    with pytest.raises(ValueError, match="runtime"):
        _prepare(source, home=tmp_path, env=env)
    assert not marker.exists()
