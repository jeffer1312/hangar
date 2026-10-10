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

from app import orq_readonly


@pytest.mark.skipif(sys.platform != "linux" or not shutil.which("bwrap"),
                    reason="prova Linux exige bubblewrap instalado")
def test_child_cannot_write_worktree_or_git_but_can_save_report(tmp_path):
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
    prefix = orq_readonly.prepare(str(alias))
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


@pytest.mark.parametrize("platform,binary,message", [
    ("win32", "/usr/bin/bwrap", "Linux"),
    ("linux", None, "instale bwrap"),
])
def test_missing_support_refuses_before_process(monkeypatch, tmp_path, platform, binary, message):
    monkeypatch.setattr(orq_readonly.sys, "platform", platform)
    monkeypatch.setattr(orq_readonly.shutil, "which", lambda _: binary)
    run = Mock()
    monkeypatch.setattr(orq_readonly.subprocess, "run", run)
    with pytest.raises(ValueError, match=message):
        orq_readonly.prepare(str(tmp_path))
    run.assert_not_called()


def test_refused_probe_has_no_unprotected_fallback(monkeypatch, tmp_path):
    monkeypatch.setattr(orq_readonly.sys, "platform", "linux")
    monkeypatch.setattr(orq_readonly.shutil, "which", lambda _: "/usr/bin/bwrap")
    monkeypatch.setattr(orq_readonly.subprocess, "run", Mock(side_effect=[
        subprocess.CompletedProcess([], 128, "", "not a repository"),
        subprocess.CompletedProcess([], 1, "", "user namespaces disabled"),
    ]))
    with pytest.raises(ValueError, match="user namespaces disabled"):
        orq_readonly.prepare(str(tmp_path))


def test_home_is_not_made_read_only(monkeypatch, tmp_path):
    monkeypatch.setattr(orq_readonly.sys, "platform", "linux")
    monkeypatch.setattr(orq_readonly.shutil, "which", lambda _: "/usr/bin/bwrap")
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    run = Mock(return_value=subprocess.CompletedProcess([], 128, "", "not a repository"))
    monkeypatch.setattr(orq_readonly.subprocess, "run", run)
    with pytest.raises(ValueError, match="HOME inteira"):
        orq_readonly.prepare(str(tmp_path))
    assert run.call_count == 1


def test_runtime_inside_code_is_refused(monkeypatch, tmp_path):
    monkeypatch.setattr(orq_readonly.sys, "platform", "linux")
    monkeypatch.setattr(orq_readonly.shutil, "which", lambda _: "/usr/bin/bwrap")
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / ".codex"))
    run = Mock(return_value=subprocess.CompletedProcess([], 128, "", "not a repository"))
    monkeypatch.setattr(orq_readonly.subprocess, "run", run)
    with pytest.raises(ValueError, match="runtime"):
        orq_readonly.prepare(str(tmp_path))
    assert run.call_count == 1


@pytest.mark.parametrize("method", ["resume", "resume_candidates"])
def test_live_resume_refuses_before_killing_protected_pane(monkeypatch, tmp_path, method):
    from app import registry

    reg = registry.SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "_pane_of", lambda _: {"pid": 123, "cwd": str(tmp_path)})
    monkeypatch.setattr(registry, "_descendant_pids", lambda _: [123])
    monkeypatch.setattr(registry, "_cmdline", lambda _: "bwrap --setenv HANGAR_ORQ_READ_ONLY 1")
    kill = Mock()
    monkeypatch.setattr(registry.tmux, "kill_session", kill)
    args = ["protected"]
    if method == "resume":
        args += ["12345678-1234-1234-1234-123456789abc"]
    with pytest.raises(ValueError, match="recrie com --read-only"):
        getattr(reg, method)(*args)
    kill.assert_not_called()


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


def test_account_switch_reopens_read_only_pane_inside_bwrap_for_new_account(monkeypatch, tmp_path):
    from app import registry
    from app.adapters.claude_headless import sessions

    order = []
    prepare = Mock(side_effect=lambda cwd, runtime_dirs=(): order.append(("prepare", runtime_dirs)) or _PREFIX)
    monkeypatch.setattr(orq_readonly, "prepare", prepare)
    reg = _protected_pane(monkeypatch, tmp_path, order)
    target = str(tmp_path / ".claude-b")

    meta = reg.para_headless("review", "manual", target_config_dir=target)
    assert order == [("prepare", (target,)), ("prepare", (str(tmp_path / ".claude-a"),)), "kill"]
    assert meta["read_only"] is True and sessions.load("review")["read_only"] is True

    # A API grava a conta nova no sidecar antes de reabrir o terminal.
    sessions.update("review", config_dir=target)
    hl = Mock(transcript_path_de=lambda m: str(tmp_path / "missing.jsonl"), escolhas=lambda n: (None, None))
    monkeypatch.setattr("app.adapters.get_adapter", lambda key: hl)
    pane = Mock(return_value=True)
    monkeypatch.setattr(registry.tmux, "new_session", pane)
    reg.para_terminal("review")
    assert prepare.call_args == ((str(tmp_path),), {"runtime_dirs": (target,)})
    argv = shlex.split(pane.call_args.args[2])
    assert argv[:len(_PREFIX)] == _PREFIX and argv[len(_PREFIX):len(_PREFIX) + 2] == ["/bin/sh", "-c"]
    inner = shlex.split(argv[-1])
    assert inner[:3] == ["claude", "--session-id", _SID] and "--model" in inner
    assert pane.call_args.args[3] == target


@pytest.mark.parametrize("target", [None, "/accounts/b"])
def test_read_only_pane_is_not_parked_without_working_protection(monkeypatch, tmp_path, target):
    from app.adapters.claude_headless import sessions

    order = []
    monkeypatch.setattr(orq_readonly, "prepare", Mock(side_effect=ValueError("read-only exige bubblewrap")))
    reg = _protected_pane(monkeypatch, tmp_path, order)
    with pytest.raises(ValueError, match="recrie com --read-only" if target is None else "bubblewrap"):
        reg.para_headless("review", "manual", target_config_dir=target)
    assert order == [] and not sessions.exists("review")


def test_parked_read_only_session_never_starts_without_terminal(tmp_path):
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _Sessao

    sess = _Sessao("review", {"name": "review", "cwd": str(tmp_path), "session_id": _SID, "read_only": True})
    with pytest.raises(ValueError, match="terminal protegido"):
        asyncio.run(ClaudeHeadlessAdapter()._launch_account_cano_owned(sess))


@pytest.mark.parametrize("created,alive", [(True, True), (True, False), (False, False)])
def test_sidecar_failure_reopens_origin_pane_with_protection_prepared_before_kill(monkeypatch, tmp_path, created, alive):
    from app import registry
    from app.adapters.claude_headless import sessions

    order = []

    def prepare(cwd, runtime_dirs=()):
        assert "kill" not in order, "a proteção do pane de volta tem que estar pronta antes do kill"
        order.append(("prepare", runtime_dirs))
        return _PREFIX

    monkeypatch.setattr(orq_readonly, "prepare", prepare)
    reg = _protected_pane(monkeypatch, tmp_path, order)
    monkeypatch.setattr(sessions, "save", Mock(side_effect=OSError("disco cheio")))
    pane = Mock(return_value=created)
    monkeypatch.setattr(registry.tmux, "new_session", pane)
    # O pane que morre ao nascer não conta como reaberto.
    monkeypatch.setattr(registry.tmux, "has_session", lambda _: alive)
    monkeypatch.setattr("app.terminal_input._wait_input_ready", lambda name, timeout=None: alive)
    reopened = created and alive
    origin, target = str(tmp_path / ".claude-a"), str(tmp_path / ".claude-b")

    with pytest.raises(OSError) as raised:
        reg.para_headless("review", "manual", target_config_dir=target)
    assert ("encerrada" in str(raised.value)) is not reopened
    assert order == [("prepare", (target,)), ("prepare", (origin,)), "kill"]
    argv = shlex.split(pane.call_args.args[2])
    assert argv[:len(_PREFIX)] == _PREFIX and pane.call_args.args[3] == origin


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


def test_parked_read_only_sidecar_waits_for_terminal_without_retrying(monkeypatch, tmp_path):
    from app.adapters.claude_headless import adapter as A, sessions

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(sessions, "_trocando", {})
    sessions.save("review", str(tmp_path), _SID, read_only=True)
    hl = A.ClaudeHeadlessAdapter()
    hl._lancar_cano = Mock(side_effect=AssertionError("não pode lançar sem terminal"))
    for _ in range(A._TETO_SUBIDAS + 1):
        with pytest.raises(A._SubidaEsgotada, match="terminal protegido"):
            asyncio.run(hl.launch_process("review"))
        with pytest.raises(A._SubidaEsgotada, match="terminal protegido"):
            asyncio.run(hl._spawn(A._Sessao("review", sessions.load("review"))))
    # Nenhuma tentativa contada: sem espera crescente entre um prompt e outro.
    assert "review" not in hl._subidas
    assert sessions.load("review")["problema"][0] == "headless_nao_subiu"


@pytest.mark.skipif(sys.platform != "linux", reason="read-only só existe no Linux")
@pytest.mark.parametrize("pane_pid", [999, None])
def test_account_switch_refuses_when_protection_cannot_be_read(monkeypatch, tmp_path, pane_pid):
    from app import registry

    order = []
    monkeypatch.setattr(orq_readonly, "prepare", Mock(side_effect=AssertionError("sem leitura não há prepare")))
    reg = _protected_pane(monkeypatch, tmp_path, order)
    monkeypatch.setattr(reg, "_pane_of", lambda n: {"name": n, "cwd": str(tmp_path), "pid": pane_pid})
    monkeypatch.setattr(registry, "_cmdline", lambda pid: "claude")
    monkeypatch.setattr(registry, "provider_of_pane", lambda pid: "claude")
    monkeypatch.setattr(registry.procinfo, "_environ_legivel", lambda pid: False)
    with pytest.raises(ValueError, match="não consegui confirmar"):
        reg.para_headless("review", "manual", target_config_dir="/accounts/b")
    assert order == []


def test_codex_launcher_is_inside_protected_pane(monkeypatch, tmp_path):
    from app import registry

    prefix = ["/usr/bin/bwrap", "--bind", "/", "/", "--ro-bind", str(tmp_path), str(tmp_path), "--"]
    monkeypatch.setattr(orq_readonly, "prepare", Mock(return_value=prefix))
    monkeypatch.setattr(registry, "_exigir_lancador_codex", lambda: None)
    monkeypatch.setattr(registry.tmux, "has_session", lambda _: False)
    monkeypatch.setattr(registry.codex_sessions, "exists", lambda _: False)
    monkeypatch.setattr(registry.codex_sessions, "pretrust_cwd", lambda *a, **kw: None)
    pane = Mock(return_value=False)
    monkeypatch.setattr(registry.tmux, "new_session", pane)
    with pytest.raises(ValueError, match="falha ao criar sessao"):
        registry.SessionRegistry(projects_dir=tmp_path).create(
            "readonly-codex", str(tmp_path), provider="codex", read_only=True,
            initial_prompt="review only", model="gpt-6-astra", effort="high")
    argv = shlex.split(pane.call_args.args[2])
    assert argv[:-3] == prefix
    assert argv[-3:-1] == ["/bin/sh", "-c"]
    launcher = shlex.split(argv[-1])
    assert launcher[0] == "hangar-codex-tui"
    assert launcher[launcher.index("--prompt") + 1] == "review only"
    assert launcher[launcher.index("--model") + 1] == "gpt-6-astra"


def test_api_checks_protection_before_creation_and_passes_flag(monkeypatch, tmp_path):
    from fastapi import HTTPException
    from app import api

    prepare = Mock(side_effect=ValueError("bwrap unavailable"))
    monkeypatch.setattr(orq_readonly, "prepare", prepare)
    create = Mock()
    monkeypatch.setattr(api.registry, "create", create)
    with pytest.raises(HTTPException) as error:
        asyncio.run(api.create_session(api.CreateBody(
            name="readonly", cwd=str(tmp_path), provider="codex", read_only=True)))
    assert error.value.status_code == 400
    create.assert_not_called()

    prepare.side_effect = None
    create = Mock(return_value=api.SessionInfo(name="readonly", cwd=str(tmp_path), provider="kimi"))
    monkeypatch.setattr(api.registry, "create", create)
    asyncio.run(api.create_session(api.CreateBody(
        name="readonly", cwd=str(tmp_path), provider="kimi", read_only=True)))
    assert create.call_args.kwargs["read_only"] is True


@pytest.mark.skipif(sys.platform != "linux" or not shutil.which("bash"),
                    reason="stub de curl usa executável POSIX")
@pytest.mark.parametrize("status", [200, 422])
def test_cli_sends_read_only_and_does_not_claim_success_on_old_backend(tmp_path, status):
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / ".env").write_text("CP_AUTH_TOKEN=test-only\n")
    cli = scripts / "hangar-send"
    shutil.copyfile(Path(__file__).resolve().parents[2] / "scripts" / "hangar-send", cli)
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
                            env={**os.environ, "PATH": str(scripts) + os.pathsep + os.environ["PATH"],
                                 "TEST_PAYLOAD": str(capture), "TEST_STATUS": str(status)},
                            capture_output=True, text=True, timeout=10)
    assert json.loads(capture.read_text())["read_only"] is True
    assert (result.returncode == 0) is (status == 200)
    assert ("sessão criada" in result.stdout) is (status == 200)
