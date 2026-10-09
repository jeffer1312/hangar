"""Prova nativa isolada da separacao de contas Codex."""

from __future__ import annotations

import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import time

import pytest

from app import codex_contas as accounts
from app.codex_contas import Account
from app.codex_importador import CodexNativo


pytestmark = [
    pytest.mark.integration,
    pytest.mark.skipif(
        os.environ.get("RUN_CODEX_INTEGRATION") != "1",
        reason="Exige RUN_CODEX_INTEGRATION=1; usa somente HOME temporaria",
    ),
]


def _tree_event(phase: str, **fields) -> None:
    """Linha de instrumentação com o instante da fase; capturada com `pytest -s`."""
    print("process-tree " + json.dumps({"phase": phase, "monotonic": time.monotonic(), **fields}), flush=True)


def _own_processes(native: CodexNativo) -> list[dict]:
    """Processos da contenção própria (Job no Windows) ou, sem ela, só o líder."""
    import psutil

    pids = native._tree._track() if native._tree is not None else [native._proc.pid]
    found = []
    for pid in pids:
        try:
            found.append({"pid": pid, "birth": psutil.Process(pid).create_time()})
        except psutil.NoSuchProcess:
            continue
    return found


def _still_running(process: dict) -> bool:
    import psutil

    try:
        proc = psutil.Process(process["pid"])
        return (proc.create_time() == process["birth"] and proc.status() != psutil.STATUS_ZOMBIE
                and proc.num_threads() > 0)
    except psutil.NoSuchProcess:
        return False


def _holding(root: Path) -> list[dict]:
    """Processos visíveis deste usuário com cwd dentro da raiz do teste."""
    import psutil

    holders = []
    for proc in psutil.process_iter(["pid"]):
        try:
            if Path(proc.cwd()).is_relative_to(root):
                holders.append({"pid": proc.pid, "birth": proc.create_time()})
        except (psutil.NoSuchProcess, psutil.AccessDenied, OSError, ValueError):
            continue
    return holders


def _isolated_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    """Remove identidade e raizes herdadas antes de iniciar os app-servers."""
    runtime = {
        "CODEX_HOME", "CODEX_CONFIG_HOME", "CODEX_SQLITE_HOME", "HOME", "USERPROFILE",
        "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME",
    }
    for name in list(os.environ):
        upper = name.upper()
        if upper in runtime or upper.startswith(("OPENAI_", "CODEX_", "CHATGPT_")):
            monkeypatch.delenv(name, raising=False)


@pytest.fixture
async def native_accounts(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _isolated_environment(monkeypatch)
    with TemporaryDirectory(prefix="codex-native-", dir=tmp_path) as temporary:
        root = Path(temporary)
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: root / "home"))
        monkeypatch.setattr(accounts, "_DEFAULT_HOME", root / "home" / ".codex")

        home = root / "home"
        source_home = home / ".codex"
        target_home = home / ".codex-work"
        home.mkdir()
        source_home.mkdir()
        target_home.mkdir()
        config = 'cli_auth_credentials_store = "file"\n'
        (source_home / "config.toml").write_text(config, encoding="utf-8")
        (target_home / "config.toml").write_text(config, encoding="utf-8")
        hook = {
            "hooks": {
                "SessionStart": [{
                    "hooks": [{"type": "command", "command": "echo inherited-hook"}],
                }],
            },
        }
        (source_home / "hooks.json").write_text(json.dumps(hook), encoding="utf-8")
        (target_home / "hooks.json").write_text(json.dumps(hook), encoding="utf-8")

        source = Account("default", source_home, True)
        target = Account("work", target_home, False)
        async with (
            CodexNativo(home, source_home, account=source) as first,
            CodexNativo(home, target_home, account=target) as second,
        ):
            first_process = first._proc
            second_process = second._proc
            yield home, source, target, first, second
            own = {"first": _own_processes(first), "second": _own_processes(second)}
            work_dirs = [Path(first._work_dir), Path(second._work_dir)]
            _tree_event("before-close", own=own, work_dirs=[str(path) for path in work_dirs])
        survivors = [process for group in own.values() for process in group if _still_running(process)]
        _tree_event("after-close", survivors=survivors,
                    work_dirs_left=[str(path) for path in work_dirs if path.exists()])
        assert survivors == []
        assert not any(path.exists() for path in work_dirs)
        assert first_process is not None and first_process.returncode is not None
        assert second_process is not None and second_process.returncode is not None
        holders = _holding(root)
        _tree_event("before-rmtree", holders=holders)
        assert holders == []
    assert not root.exists()


def _hooks(result: dict) -> list[dict]:
    entries = result.get("data")
    assert isinstance(entries, list), result
    assert all(isinstance(entry, dict) for entry in entries), result
    return [hook for entry in entries for hook in entry.get("hooks", [])]


async def test_contas_codex_nativas_separam_login_config_hooks_e_logout(native_accounts):
    home, source, target, first, second = native_accounts

    await first.request("account/login/start", {
        "type": "apiKey", "apiKey": "sk-test-local-a",
    })
    await second.request("account/login/start", {
        "type": "apiKey", "apiKey": "sk-test-local-b",
    })

    source_auth = source.home / "auth.json"
    target_auth = target.home / "auth.json"
    assert source_auth.is_file()
    assert target_auth.is_file()
    assert source_auth.read_bytes() != target_auth.read_bytes()

    source_config_before = (source.home / "config.toml").read_bytes()
    await second.request("config/value/write", {
        "keyPath": "model_reasoning_effort",
        "value": "low",
        "mergeStrategy": "replace",
    })
    assert (source.home / "config.toml").read_bytes() == source_config_before
    assert "model_reasoning_effort" in (target.home / "config.toml").read_text(encoding="utf-8")

    source_hooks = _hooks(await first.request("hooks/list", {"cwds": [str(home)]}))
    target_hooks = _hooks(await second.request("hooks/list", {"cwds": [str(home)]}))
    assert source_hooks and target_hooks
    assert any(hook.get("command") == "echo inherited-hook" for hook in source_hooks)
    assert any(hook.get("command") == "echo inherited-hook" for hook in target_hooks)
    assert all(hook.get("trustStatus") in {"untrusted", "modified"} for hook in source_hooks + target_hooks)

    await second.request("account/logout", None)
    first_account = await first.request("account/read", {"refreshToken": False})
    second_account = await second.request("account/read", {"refreshToken": False})
    assert first_account["account"]["type"] == "apiKey"
    assert second_account["account"] is None
