"""Proteção real do código, preservando relatórios e o lançador no pane."""
import asyncio
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from unittest.mock import Mock

import pytest

from app import providers_bridge


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


_SID = "33333333-3333-3333-3333-333333333333"
_PREFIX = ["/usr/bin/bwrap", "--bind", "/", "/", "--setenv", "HANGAR_ORQ_READ_ONLY", "1", "--"]


def _protected_pane(monkeypatch, tmp_path, order):
    """Pane Claude dentro do bwrap, na conta A, pronto para a troca de conta."""
    from app import pqueue, registry
    from app.adapters.claude_headless import sessions

    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(sessions, "_trocando", {})
    monkeypatch.setattr(registry.procinfo, "pid_vivo", lambda pid: False)
    monkeypatch.setattr(registry.tmux, "has_session", lambda _: False)
    reg = registry.SessionRegistry(str(tmp_path / "projects"))
    jsonl = tmp_path / "projects" / "x" / f"{_SID}.jsonl"
    monkeypatch.setattr(reg, "_pane_of", lambda n: {"name": n, "cwd": str(tmp_path), "pid": 999})
    monkeypatch.setattr(reg, "resolve_tracked", lambda n, c: (str(jsonl), True))
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid: [999, 1000])
    monkeypatch.setattr(registry, "_cmdline", lambda pid: " ".join(_PREFIX) if pid == 999 else "claude")
    monkeypatch.setattr(registry, "agente_do_pane", lambda pid, children=None: ("claude", 1000))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: tmp_path / ".claude-a")
    monkeypatch.setattr(registry, "_engine_of", lambda pid: None)
    monkeypatch.setattr(registry.procinfo, "_model_of", lambda pid: ("sonnet", "high"))
    monkeypatch.setattr(registry.procinfo, "_env_var_of", lambda pid, key: None)
    monkeypatch.setattr(registry, "_escolhas_status", lambda sid: (None, None))
    monkeypatch.setattr(registry.tmux, "kill_session", lambda n: order.append("kill") or True)
    return reg


def test_read_only_transfer_source_is_refused_before_kill(monkeypatch, tmp_path):
    from app.adapters.claude_headless import sessions

    order = []
    monkeypatch.setattr(providers_bridge, "local", Mock(side_effect=AssertionError("a origem não reabre protegida")))
    reg = _protected_pane(monkeypatch, tmp_path, order)
    transfer_meta = {"session_id": _SID, "cwd": str(tmp_path), "pane_pid": 999, "key": "k", "transfer_id": "t"}
    with pytest.raises(ValueError, match="recrie com --read-only"):
        reg.para_headless("review", "manual", transfer_meta=transfer_meta)
    assert order == [] and not sessions.exists("review")


def test_parked_read_only_sidecar_is_not_transferred(monkeypatch, tmp_path):
    from app import registry
    from app.adapters.claude_headless import sessions
    from app.conversation_transfer import TransferError
    from app.models import SessionInfo

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "hl")
    sessions.save("review", str(tmp_path), _SID, read_only=True)
    reg = registry.SessionRegistry(str(tmp_path / "projects"))
    with pytest.raises(TransferError) as raised:
        reg.transfer_origin(SessionInfo(name="review", cwd=str(tmp_path), provider="claude", headless=True))
    assert raised.value.code == "session_transfer_read_only"


@pytest.mark.skipif(sys.platform != "linux" or not shutil.which("bash"),
                    reason="stub de curl usa executável POSIX")
@pytest.mark.parametrize("status", [200, 422])
def test_cli_sends_read_only_and_does_not_claim_success_on_old_backend(tmp_path, status, hangar_server_bin):
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / ".env").write_text("CP_AUTH_TOKEN=test-only\n")
    cli = scripts / "hangar-send"
    shutil.copyfile(Path(__file__).resolve().parents[2] / "scripts" / "hangar-send", cli)
    shutil.copy(Path(__file__).resolve().parents[2] / "scripts" / "hangar-server-command", scripts / "hangar-server-command")
    capture = tmp_path / "payload.json"
    curl = scripts / "curl"
    curl.write_text(f"#!{sys.executable}\n" + """import json,os,pathlib,sys
args=sys.argv[1:]
if '-d' in args:
    pathlib.Path(os.environ['TEST_PAYLOAD']).write_text(args[args.index('-d')+1])
    print('{}')
    print(os.environ['TEST_STATUS'])
else:
    print('{"claude": {"disponivel": true, "default": true}}')
    print('200')
""")
    curl.chmod(0o700)
    result = subprocess.run(["bash", str(cli), "--new", "review", str(tmp_path), "--read-only"],
                            env={**os.environ, "HOME": str(tmp_path), "USERPROFILE": str(tmp_path),
                                 "CP_RUST_SERVER_BIN": str(hangar_server_bin),
                                 "PATH": str(scripts) + os.pathsep + os.environ["PATH"],
                                 "TEST_PAYLOAD": str(capture), "TEST_STATUS": str(status)},
                            capture_output=True, text=True, timeout=10)
    assert json.loads(capture.read_text())["read_only"] is True
    assert (result.returncode == 0) is (status == 200)
    assert ("sessão criada" in result.stdout) is (status == 200)
