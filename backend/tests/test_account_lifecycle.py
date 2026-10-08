"""Proteção entre processos: executada somente em ambiente descartável."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import queue
import subprocess
import threading

import pytest

from app import codex_contas as accounts
from app.codex_contas_login import CodexContasLogin


def lock_path(root: Path, provider: str, home: Path) -> Path:
    canonical = str(home.resolve()).replace("\\", "/")
    if os.name == "nt":
        canonical = canonical.removeprefix("//?/").lower()
    digest = hashlib.sha256(f"{provider}\0{canonical}".encode()).hexdigest()
    return root / f"{digest}.lock"


@pytest.fixture(scope="session")
def rust_probe():
    root = Path(__file__).resolve().parents[2]
    target = Path(os.environ["CARGO_TARGET_DIR"])
    candidates = list((target / "debug" / "deps").glob("accounts_lifecycle-*.exe" if os.name == "nt" else "accounts_lifecycle-*"))
    candidates = [path for path in candidates if path.is_file() and path.suffix not in {".d", ".pdb"}]
    assert candidates, "compile accounts_lifecycle antes desta prova"
    return max(candidates, key=lambda path: path.stat().st_mtime)


class RustProbe:
    def __init__(self, executable: Path, path: Path, mode="exclusive"):
        env = dict(os.environ, ACCOUNT_PROBE_PATH=str(path), ACCOUNT_PROBE_MODE=mode)
        self.process = subprocess.Popen([str(executable), "--exact", "probe_process", "--nocapture"],
                                        env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="strict")
        result = queue.Queue()
        def read():
            for line in self.process.stdout:
                if line.startswith("ACCOUNT_PROBE:"):
                    result.put(line.strip().split(":", 1)[1])
                    return
            result.put("terminated")
        threading.Thread(target=read, daemon=True).start()
        self.state = result.get(timeout=20)

    def close(self):
        self.process.communicate("release\n", timeout=20)
        assert self.process.returncode == 0

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def test_shared_key_fixture():
    import json
    from app.account_lifecycle import key_digest, normalize_windows_path
    fixture = Path(__file__).parent / "fixtures/accounts_contract/account-keys.json"
    for row in json.loads(fixture.read_text(encoding="utf-8")):
        for value in row["inputs"]:
            canonical = normalize_windows_path(value) if row["windows"] else value
            assert canonical == row["canonical"]
            assert key_digest(row["provider"], canonical) == row["sha256"]


def test_key_survives_missing_home_and_directory_recreation(tmp_path):
    from app.account_lifecycle import AccountKey, GuardMode, acquire
    home = tmp_path / "revisão" / "nova"
    before = AccountKey.new("claude", home)
    home.mkdir(parents=True)
    assert before == AccountKey.new("claude", home)
    with acquire(before, root=tmp_path / "locks") as guard:
        home.rmdir()
        assert before == guard.key == AccountKey.new("claude", home)
    assert (tmp_path / "locks" / f"{before.digest}.lock").exists()


def test_rust_exclusion_blocks_python_birth_login_and_preparation(tmp_path, monkeypatch, rust_probe):
    from app import account_lifecycle
    root = tmp_path / "locks"
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: root)
    account = accounts.Account("work", tmp_path / ".codex-work", False)
    account.home.mkdir()
    service = CodexContasLogin(account_in_use=lambda account: False)
    with RustProbe(rust_probe, lock_path(root, "codex", account.home)) as exclusion:
        assert exclusion.state == "acquired"
        for kind in ("creation", "prepare", "login"):
            with pytest.raises(accounts.AccountError):
                service._reserve(account, kind)


def test_rust_preparation_coexists_with_python_birth(tmp_path, monkeypatch, rust_probe):
    from app import account_lifecycle
    root = tmp_path / "locks"
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: root)
    account = accounts.Account("work", tmp_path / ".codex-work", False)
    account.home.mkdir()
    service = CodexContasLogin(account_in_use=lambda account: False)
    with RustProbe(rust_probe, lock_path(root, "codex", account.home), "shared") as preparation:
        assert preparation.state == "acquired"
        birth = service.reserve_creation(account)
        birth.release()


def test_cancelled_wait_does_not_start_operation(tmp_path):
    import time
    from app.account_lifecycle import AccountKey, GuardMode, acquire, AccountLockError
    key = AccountKey.new("claude", tmp_path / "account")
    with acquire(key, GuardMode.EXCLUSIVE, root=tmp_path / "locks"):
        cancelled = threading.Event()
        cancelled.set()
        with pytest.raises(AccountLockError, match="account_lock_cancelled"):
            acquire(key, root=tmp_path / "locks", cancel=cancelled)
        with pytest.raises(AccountLockError, match="account_busy"):
            acquire(key, root=tmp_path / "locks", deadline=time.monotonic())


@pytest.mark.parametrize("cancel", [False, True])
def test_actual_birth_remains_protected_after_two_cancellations_and_rust_restart(tmp_path, rust_probe, cancel):
    from tests.accounts_contract import PythonReference
    reference = PythonReference(tmp_path / "reference")
    barrier = reference.block("session_before_registration")
    birth = reference.start_session_async(provider="codex", account_id="alpha")
    try:
        assert barrier.entered.wait(), birth.result(timeout=1).body
        if cancel:
            assert reference.cancel_birth().json()["cancelled"]
            assert reference.cancel_birth().json()["cancelled"]
        for _ in range(2):
            path = lock_path(reference.root / ".hangar/account-locks", "codex", reference.root / ".codex-alpha")
            with RustProbe(rust_probe, path) as exclusion:
                assert exclusion.state == "busy"
                assert reference.account_exists("codex", "alpha")
        barrier.release.set()
        response = birth.result(timeout=20)
        assert response.status_code == (499 if cancel else 200), response.body
    finally:
        barrier.release.set()
        reference.close()


class ProcessFixture:
    pid = 71
    def __init__(self, home, *, error=None, started=1):
        self.home, self.error, self.started = home, error, started
    def name(self):
        return "codex.exe"
    def cmdline(self):
        return ["codex", "app-server"]
    def create_time(self):
        return self.started
    def environ(self):
        if self.error:
            raise self.error
        return {"CODEX_HOME": str(self.home)}


def test_unknown_process_environment_and_reused_pid_refuse_exclusion(tmp_path):
    import psutil
    from app.account_bridge import inspect_processes
    from app.account_lifecycle import AccountKey
    key = AccountKey.new("codex", tmp_path)
    unreadable = ProcessFixture(tmp_path, error=psutil.AccessDenied(71))
    facts = inspect_processes(key, processes=[unreadable])
    with pytest.raises(RuntimeError, match="account_usage_unknown"):
        facts.ensure_unused()
    stale = ProcessFixture(tmp_path)
    reused = ProcessFixture(tmp_path, started=2)
    facts = inspect_processes(key, processes=[stale], process_factory=lambda pid: reused)
    assert not facts.complete
    assert facts.pids == []
    vanished = ProcessFixture(tmp_path, error=psutil.NoSuchProcess(71))
    assert inspect_processes(key, processes=[vanished]).complete
    facts = inspect_processes(key, processes=[stale], process_factory=lambda pid: stale)
    assert facts.complete
    assert facts.pids == [71]
    with pytest.raises(RuntimeError, match="account_in_use"):
        facts.ensure_unused()


def test_pending_headless_birth_counts_before_process_exists(tmp_path, monkeypatch):
    import json
    from app import account_bridge
    from app.account_lifecycle import AccountKey
    from app.adapters.claude_headless import sessions
    directory = tmp_path / "sidecars"
    directory.mkdir()
    account = tmp_path / ".claude-work"
    account.mkdir()
    (directory / "birth.json").write_text(json.dumps({"name": "birth", "session_id": "pending", "config_dir": str(account)}))
    monkeypatch.setattr(sessions, "_dir", lambda: directory)
    monkeypatch.setattr(account_bridge, "inspect_processes", lambda key: account_bridge.UsageFacts(complete=True))
    facts = account_bridge.inspect_usage(AccountKey.new("claude", account))
    assert facts.complete
    assert facts.sessions == ["birth"]
    with pytest.raises(RuntimeError, match="account_in_use"):
        facts.ensure_unused()
    (directory / "birth.json").write_text("{")
    assert not account_bridge.inspect_usage(AccountKey.new("claude", account)).complete


@pytest.mark.parametrize("worker_cycle", [False, True])
def test_claude_preparation_keeps_own_descriptor_after_birth_returns(tmp_path, monkeypatch, rust_probe, worker_cycle):
    from concurrent.futures import ThreadPoolExecutor
    from app import account_lifecycle, contas
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    home = tmp_path / ".claude-work"
    home.mkdir()
    (home / contas.MARCADOR).write_text("")
    (tmp_path / ".claude").mkdir()
    entered, release = threading.Event(), threading.Event()
    def prepare(*args):
        entered.set()
        assert release.wait(20)
        return []
    monkeypatch.setattr(contas, "_reconciliar", prepare)
    with ThreadPoolExecutor() as pool:
        try:
            with contas.ciclo_conta("work", mode=account_lifecycle.GuardMode.SHARED) as cycle:
                preparation = (pool.submit(cycle.reconciliar) if worker_cycle else
                               pool.submit(contas.reconciliar, "work"))
                assert entered.wait(10), "preparo ficou bloqueado pelo nascimento"
            path = lock_path(tmp_path / ".hangar/account-locks", "claude", home)
            with RustProbe(rust_probe, path) as exclusion:
                assert exclusion.state == "busy"
        finally:
            release.set()
        assert preparation.result(timeout=20) == []


async def test_deferred_cano_launch_holds_guard_until_worker_finishes(tmp_path, monkeypatch, rust_probe):
    import asyncio
    from app import account_lifecycle
    from app.adapters.claude_headless import adapter
    root = tmp_path / "locks"
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: root)
    entered, release = asyncio.Event(), asyncio.Event()
    async def launch(*args, **kwargs):
        entered.set()
        await release.wait()
        return {"pid": 17}, None
    monkeypatch.setattr(adapter, "_spawn_account_process", launch)
    task = asyncio.create_task(adapter.subir_cano_processo(
        ["codex", "app-server"], cwd=str(tmp_path), env={"CODEX_HOME": str(tmp_path)},
        key="test", log=tmp_path / "log", account_provider="codex"))
    try:
        await entered.wait()
        task.cancel()
        def dispute():
            with RustProbe(rust_probe, lock_path(root, "codex", tmp_path)) as exclusion:
                return exclusion.state
        assert await asyncio.to_thread(dispute) == "busy"
        task.cancel()
        assert await asyncio.to_thread(dispute) == "busy"
    finally:
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task


def test_real_external_process_is_usage(tmp_path):
    import sys
    import psutil
    from app.account_bridge import inspect_processes
    from app.account_lifecycle import AccountKey
    from tests.accounts_contract import isolated_environment
    env = isolated_environment(tmp_path)
    env["CODEX_HOME"] = str(tmp_path / "account")
    process = subprocess.Popen([sys.executable, "-c",
                                "import sys; print('ready', flush=True); sys.stdin.readline()", "--", "codex"],
                               env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="strict")
    try:
        assert process.stdout.readline().strip() == "ready"
        facts = inspect_processes(AccountKey.new("codex", tmp_path / "account"),
                                  processes=[psutil.Process(process.pid)])
        assert facts.complete
        assert facts.pids == [process.pid]
    finally:
        process.communicate("release\n", timeout=10)


def test_private_usage_facts_require_current_instance_and_do_not_relock(tmp_path, monkeypatch):
    from dataclasses import asdict
    from types import SimpleNamespace
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import account_bridge, internal_api, runtime_coordinator
    from app.account_lifecycle import AccountKey, GuardMode, acquire
    from app.adapters.claude_headless import sessions
    directory = tmp_path / "sidecars"
    directory.mkdir()
    monkeypatch.setattr(sessions, "_dir", lambda: directory)
    monkeypatch.setattr(account_bridge, "inspect_processes", lambda key: account_bridge.UsageFacts(complete=True))
    monkeypatch.setattr(internal_api, "_secret", "internal-test")
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="current"))
    app = FastAPI()
    app.include_router(internal_api.router)
    key = AccountKey.new("claude", tmp_path / "account")
    payload = {"keys": [{"provider": key.provider.value, "canonical_home": str(key.canonical_home)}]}
    headers = {"x-hangar-internal": "internal-test", "x-hangar-runtime-instance": "current"}
    with TestClient(app, client=("127.0.0.1", 1212)) as client:
        assert client.post("/internal/accounts/facts", json=payload).status_code == 404
        assert client.post("/internal/accounts/facts", json=payload,
                           headers={**headers, "x-hangar-runtime-instance": "old"}).status_code == 404
        with acquire(key, GuardMode.EXCLUSIVE, root=tmp_path / "locks"):
            response = client.post("/internal/accounts/facts", json=payload, headers=headers)
        assert response.status_code == 200
        assert response.json() == [{"key": payload["keys"][0], "facts": {"complete": True, "sessions": [], "pids": []}}]


def test_linux_process_reader_does_not_hide_unreadable_environment(tmp_path):
    from app.account_bridge import LinuxProcess, inspect_processes
    from app.account_lifecycle import AccountKey
    process = LinuxProcess(71)
    process.root = tmp_path / "proc"
    process.root.mkdir()
    (process.root / "comm").write_text("codex")
    (process.root / "cmdline").write_bytes(b"codex\0app-server\0")
    (process.root / "stat").write_text("71 (codex (test)) " + " ".join(["S"] + ["0"] * 18 + ["123"]))
    (process.root / "environ").write_bytes(("CODEX_HOME=" + str(tmp_path)).encode() + b"\0")
    key = AccountKey.new("codex", tmp_path)
    facts = inspect_processes(key, processes=[process], process_factory=lambda pid: process)
    assert facts.complete and facts.pids == [71]
    (process.root / "environ").write_bytes(b"")
    assert not inspect_processes(key, processes=[process], process_factory=lambda pid: process).complete


async def test_cancelled_cano_birth_still_publishes_its_process(tmp_path, monkeypatch, rust_probe):
    import asyncio
    import json
    from app import account_lifecycle
    from app.adapters.codex import sem_terminal, sessions
    from app.adapters.claude_headless import adapter
    root = tmp_path / "locks"
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: root)
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    monkeypatch.setattr(sem_terminal.shutil, "which", lambda command: command)
    monkeypatch.setattr(sem_terminal, "_ambiente", lambda meta: {"CODEX_HOME": str(tmp_path)})
    meta = {"name": "birth", "key": "test", "cwd": str(tmp_path)}
    (tmp_path / "birth.json").write_text(json.dumps(meta))
    entered, release = asyncio.Event(), asyncio.Event()
    async def launch(*args, **kwargs):
        entered.set()
        await release.wait()
        return {"pid": 17, "escuta": "test"}, None
    monkeypatch.setattr(adapter, "_spawn_account_process", launch)
    task = asyncio.create_task(sem_terminal.subir(meta))
    try:
        await entered.wait()
        for _ in range(2):
            task.cancel()
            def dispute():
                with RustProbe(rust_probe, lock_path(root, "codex", tmp_path)) as exclusion:
                    return exclusion.state
            assert await asyncio.to_thread(dispute) == "busy"
        assert sessions.load("birth").get("cano") is None
    finally:
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
    assert sessions.load("birth")["cano"] == {"pid": 17, "escuta": "test"}


def test_old_terminal_sidecar_needs_live_or_pending_birth(tmp_path, monkeypatch):
    import json
    from app import account_bridge
    from app.account_lifecycle import AccountKey
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    monkeypatch.setattr(account_bridge, "inspect_processes", lambda key: account_bridge.UsageFacts(complete=True))
    sidecar = tmp_path / "old.json"
    row = {"name": "old", "codex_home": str(tmp_path / "account")}
    sidecar.write_text(json.dumps(row))
    key = AccountKey.new("codex", tmp_path / "account")
    monkeypatch.setattr(account_bridge, "terminal_sessions", lambda: {"old"})
    assert account_bridge.inspect_usage(key).sessions == ["old"]
    monkeypatch.setattr(account_bridge, "terminal_sessions", lambda: set())
    account_bridge.inspect_usage(key).ensure_unused()
    def unavailable():
        raise RuntimeError("mux indisponível")
    monkeypatch.setattr(account_bridge, "terminal_sessions", unavailable)
    with pytest.raises(RuntimeError, match="account_usage_unknown"):
        account_bridge.inspect_usage(key).ensure_unused()
    sidecar.write_text(json.dumps({**row, "launching": True}))
    facts = account_bridge.inspect_usage(key)
    assert facts.complete and facts.sessions == ["old"]


@pytest.mark.parametrize("cancel", [False, True])
def test_terminal_birth_is_published_before_real_lease_release(tmp_path, rust_probe, cancel):
    from tests.accounts_contract import PythonReference
    reference = PythonReference(tmp_path / "reference")
    before = reference.block("session_before_registration")
    launcher = reference.block("launcher_before_publication")
    birth = reference.start_session_async(provider="codex", account_id="alpha")
    try:
        assert before.entered.wait()
        if cancel:
            assert reference.cancel_birth().json()["cancelled"]
            assert reference.cancel_birth().json()["cancelled"]
        before.release.set()
        assert birth.result(timeout=20).status_code == (499 if cancel else 200)
        assert launcher.entered.wait()
        path = lock_path(reference.root / ".hangar/account-locks", "codex", reference.root / ".codex-alpha")
        for _ in range(2):
            with RustProbe(rust_probe, path) as exclusion:
                assert exclusion.state == "acquired", "o nascimento publicado não deve reter FD pela sessão inteira"
                facts = reference.request("GET", "/__contract__/usage").json()
                assert facts["complete"]
                assert facts["sessions"] == ["birth"], "liberou a guarda antes da publicação durável do nascimento"
                assert facts["pids"], "o lançador com --codex-home também usa a conta antes de publicar o ambiente"
        assert reference.request("POST", "/__contract__/stop-launcher").status_code == 200
        facts = reference.request("GET", "/__contract__/usage").json()
        assert facts == {"complete": True, "sessions": [], "pids": []}
        assert reference.request("POST", "/__contract__/retire-birth").status_code == 200
        assert not list((reference.root / ".hangar/account-locks/births").glob("*.json"))
    finally:
        before.release.set()
        launcher.release.set()
        reference.close()


def test_terminal_pending_birth_retires_after_verified_launcher_binding(tmp_path, rust_probe):
    from tests.accounts_contract import PythonReference
    reference = PythonReference(tmp_path / "reference")
    launcher = reference.block("launcher_before_publication")
    birth = reference.start_session_async(provider="codex", account_id="alpha")
    try:
        assert birth.result(timeout=20).status_code == 200
        assert launcher.entered.wait()
        directory = reference.root / ".hangar/account-locks/births"
        assert len(list(directory.glob("*.json"))) == 1
        launcher.release.set()
        assert reference.request("GET", "/__contract__/published").json()["published"]
        assert reference.request("POST", "/__contract__/retire-birth").status_code == 200
        assert not list(directory.glob("*.json"))
        facts = reference.request("GET", "/__contract__/usage").json()
        assert facts["complete"] and facts["sessions"] == ["birth"] and facts["pids"]
        path = lock_path(reference.root / ".hangar/account-locks", "codex", reference.root / ".codex-alpha")
        with RustProbe(rust_probe, path) as exclusion:
            assert exclusion.state == "acquired"
    finally:
        launcher.release.set()
        reference.close()


def test_terminal_pending_identity_and_uncertainty_are_not_absence(tmp_path, monkeypatch):
    from app import account_lifecycle, account_bridge
    from app.adapters.codex import sessions
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: tmp_path / "locks")
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sidecars")
    monkeypatch.setattr(account_bridge, "inspect_processes",
                        lambda key: account_bridge.UsageFacts(complete=True))
    monkeypatch.setattr(account_bridge, "terminal_instances", lambda: {"birth": "server:old:epoch"})
    key = account_lifecycle.AccountKey.new("codex", tmp_path / "account")
    with account_lifecycle.acquire(key) as guard:
        record = account_lifecycle.publish_terminal_birth(guard, "birth", "token")
    assert account_bridge.inspect_usage(key).sessions == ["birth"]

    def unavailable():
        raise RuntimeError("multiplexador indisponível")

    monkeypatch.setattr(account_bridge, "terminal_instances", unavailable)
    assert not account_bridge.inspect_usage(key).complete
    assert not account_lifecycle.retire_terminal_birth(record)
    assert record.exists()
    monkeypatch.setattr(account_bridge, "terminal_instances", lambda: {"birth": "server:new:epoch"})
    account_bridge.inspect_usage(key).ensure_unused()
    assert account_lifecycle.retire_terminal_birth(record)
    assert not record.exists()

    with account_lifecycle.acquire(key) as guard:
        record = account_lifecycle.publish_terminal_birth(guard, "birth", "token")
    record.write_text("{", encoding="utf-8")
    assert not account_bridge.inspect_usage(key).complete
    assert not account_lifecycle.retire_terminal_birth(record)


def test_terminal_generation_snapshot_rejects_incomplete_mux_format(monkeypatch):
    from subprocess import CompletedProcess
    from app import account_bridge, tmux
    monkeypatch.setattr(tmux, "_run", lambda args: CompletedProcess(args, 0, "birth\t123:$4:567\r\n", ""))
    assert account_bridge.terminal_instances() == {"birth": "123:$4:567"}
    monkeypatch.setattr(tmux, "_run", lambda args: CompletedProcess(args, 0, "birth\t#{pid}:$4:567\n", ""))
    with pytest.raises(RuntimeError, match="account_mux_unknown"):
        account_bridge.terminal_instances()


def test_python_creation_blocks_rust_exclusion_across_restart(tmp_path, monkeypatch, rust_probe):
    root = tmp_path / "locks"
    from app import account_lifecycle
    monkeypatch.setattr(account_lifecycle, "default_lock_root", lambda: root)
    home = tmp_path / ".codex-work"
    home.mkdir()
    account = accounts.Account("work", home, False)
    service = CodexContasLogin(account_in_use=lambda account: False)
    reservation = service.reserve_creation(account)
    try:
        for _ in range(2):
            with RustProbe(rust_probe, lock_path(root, "codex", home)) as contender:
                assert contender.state == "busy", "nascimento Python permitiu exclusão no Rust"
    finally:
        reservation.release()
    with RustProbe(rust_probe, lock_path(root, "codex", home)) as contender:
        assert contender.state == "acquired"
