"""Proteção entre processos: executada somente em ambiente descartável."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import queue
import subprocess
import threading

import pytest

from accounts_contract import rust_integration_binary


def lock_path(root: Path, provider: str, home: Path) -> Path:
    canonical = str(home.resolve()).replace("\\", "/")
    if os.name == "nt":
        canonical = canonical.removeprefix("//?/").lower()
    digest = hashlib.sha256(f"{provider}\0{canonical}".encode()).hexdigest()
    return root / f"{digest}.lock"


@pytest.fixture(scope="session")
def rust_probe():
    return rust_integration_binary()


class RustProbe:
    def __init__(self, executable: Path, path: Path, mode="exclusive"):
        env = dict(os.environ, ACCOUNT_PROBE_PATH=str(path), ACCOUNT_PROBE_MODE=mode)
        self.process = subprocess.Popen([str(executable), "--exact", "accounts_lifecycle::probe_process", "--nocapture"],
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


def test_terminal_generation_snapshot_rejects_incomplete_mux_format(monkeypatch):
    from subprocess import CompletedProcess
    from app import account_bridge, tmux
    monkeypatch.setattr(tmux, "_run", lambda args: CompletedProcess(args, 0, "birth\t123:$4:567\r\n", ""))
    assert account_bridge.terminal_instances() == {"birth": "123:$4:567"}
    monkeypatch.setattr(tmux, "_run", lambda args: CompletedProcess(args, 0, "birth\t#{pid}:$4:567\n", ""))
    with pytest.raises(RuntimeError, match="account_mux_unknown"):
        account_bridge.terminal_instances()


