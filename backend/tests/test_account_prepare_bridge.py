"""A ponte prepara configuração sem herdar autenticação ou liberar trabalho vivo."""
import json
from pathlib import Path
from types import SimpleNamespace

from fastapi import FastAPI
from fastapi.testclient import TestClient

from app import account_lifecycle, internal_api, runtime_coordinator


from test_codex_contas_sync import isolated, fake_writer

import pytest

def test_prepare_callback_requires_current_operation_and_seeds_pending_account(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / ".claude"))
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="current"))
    shared = tmp_path / ".claude"
    shared.mkdir()
    (shared / "settings.json").write_text('{"permissions":{"allow":["Read"]}}', encoding="utf-8")
    (shared / ".credentials.json").write_text('{"synthetic":"private"}', encoding="utf-8")
    target = tmp_path / ".claude-fresh"
    target.mkdir()
    (target / ".hangar-account-pending").write_text("", encoding="utf-8")
    key = account_lifecycle.AccountKey.new("claude", target)
    body = {"operation": "a" * 32, "instance": "current", "key": {
        "provider": "claude", "canonical_home": str(key.canonical_home)},
        "account_id": "fresh", "seed": True, "force": False, "cwd": None}
    root = account_lifecycle.default_lock_root()
    root.mkdir(parents=True)
    (root / (key.digest + ".prepare.json")).write_text(json.dumps(body), encoding="utf-8")
    app = FastAPI()
    internal_api.set_secret("synthetic-internal")
    app.include_router(internal_api.router)
    headers = {"x-hangar-internal": "synthetic-internal", "x-hangar-runtime-instance": "current"}
    with TestClient(app, client=("127.0.0.1", 31000)) as client:
        response = client.post("/internal/accounts/prepare", json=body, headers=headers)
        assert response.status_code == 200, response.text
        result = response.json()
        assert result["status"] in {"running", "ready"}
        if result["status"] == "running":
            result = client.post("/internal/accounts/prepare/wait", json={"operation": body["operation"]}, headers=headers).json()
        assert result["status"] == "ready", result
    assert (target / "settings.json").is_file()
    assert not (target / "settings.json").is_symlink()
    assert not (target / ".credentials.json").exists()
    assert not (target / "projects").is_symlink()
    assert not (target / ".hangar-conta").exists()
    assert (target / ".hangar-account-pending").exists()

@pytest.mark.parametrize("force_pending", [False, True])
def test_http_python_worker_blocks_delete_after_rust_restart(tmp_path, force_pending):
    import urllib.request
    from tests.accounts_contract import PythonReference
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path, block_handlers=True)
    rust = RustCatalog(reference)
    try:
        barrier = reference.block("account_prepare")
        assert rust.request("POST", "/api/codex-contas/alpha/prepare").status_code == 202
        assert barrier.entered.wait()
        if force_pending:
            assert rust.request("POST", "/api/codex-contas/alpha/prepare?forcar=true").status_code == 202
        assert rust.request("DELETE", "/api/codex-contas/alpha").status_code == 409
        records = list((tmp_path / ".hangar/account-locks").glob("*.prepare.json"))
        assert len(records) == 1
        old_operation = json.loads(records[0].read_text(encoding="utf-8"))
        rust.close()
        assert reference.request("POST", "/__contract__/runtime-instance", {"instance": "replacement"}).status_code == 200
        rust = RustCatalog(reference, instance="replacement")
        response = rust.request("DELETE", "/api/codex-contas/alpha")
        assert response.status_code == 409, response.text
        assert (tmp_path / ".codex-alpha/config.toml").is_file()
        status = rust.request("GET", "/api/codex-contas/alpha/prepare")
        assert status.json()["status"] == "running"
        # Uma entrega da instância morta não inicia um segundo reconciliador.
        request = urllib.request.Request(reference.base_url + "/internal/accounts/prepare",
            data=json.dumps(old_operation).encode(), method="POST", headers={
                "Content-Type": "application/json", "x-hangar-internal": "contract-internal",
                "x-hangar-runtime-instance": "contract-instance"})
        import urllib.error
        try:
            urllib.request.urlopen(request, timeout=10)
            raise AssertionError("pedido antigo foi aceito")
        except urllib.error.HTTPError as error:
            assert error.code == 404
        assert reference.request("GET", "/__contract__/preparation-calls").json() == [False]
        barrier.release.set()
        request = urllib.request.Request(reference.base_url + "/internal/accounts/prepare/wait",
            data=json.dumps({"operation": old_operation["operation"]}).encode(), method="POST", headers={
                "Content-Type": "application/json", "x-hangar-internal": "contract-internal",
                "x-hangar-runtime-instance": "replacement"})
        with urllib.request.urlopen(request, timeout=10) as response:
            assert json.load(response)["status"] == "ready"
        for _ in range(100):
            if rust.request("GET", "/api/codex-contas/alpha/prepare").json()["status"] == "ready":
                break
        else:
            raise AssertionError("a retomada não concluiu o preparo")
        assert reference.request("GET", "/__contract__/preparation-calls").json() == ([False, True] if force_pending else [False])
        assert rust.request("DELETE", "/api/codex-contas/alpha").status_code == 200
        assert reference.calls() == []
    finally:
        reference.request("POST", "/__contract__/cleanup")
        rust.close()
        reference.close()


def test_http_claude_seed_preserves_configuration_and_does_not_publish_oauth(tmp_path):
    from tests.accounts_contract import PythonReference
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path, block_handlers=True)
    rust = RustCatalog(reference)
    try:
        source = tmp_path / ".claude"
        (source / "settings.json").write_text('{"permissions":{"allow":["Read"]}}', encoding="utf-8")
        (source / ".credentials.json").write_text('{"token":"synthetic"}', encoding="utf-8")
        (tmp_path / ".claude.json").write_text('{"oauthAccount":{"email":"synthetic"},"projects":{"fixture":{"hasTrustDialogAccepted":true}}}', encoding="utf-8")
        (source / "skills").mkdir()
        (source / "skills/fixture.md").write_text("Skill de teste", encoding="utf-8")
        (source / "sessions").mkdir()
        result = rust.request("POST", "/api/claude-configs", {"nome": "fresh"})
        assert result.status_code == 200, result.text
        target = tmp_path / ".claude-fresh"
        assert (target / "settings.json").read_text(encoding="utf-8") == (source / "settings.json").read_text(encoding="utf-8")
        assert not (target / "settings.json").is_symlink()
        assert (target / "skills/fixture.md").read_text(encoding="utf-8") == "Skill de teste"
        assert not (target / "sessions").exists()
        assert not (target / ".credentials.json").exists()
        settings = json.loads((target / ".claude.json").read_text(encoding="utf-8"))
        assert "oauthAccount" not in settings
        assert settings["projects"]["fixture"]["hasTrustDialogAccepted"] is True
        assert (target / ".hangar-conta").is_file()
        assert not (target / ".hangar-account-pending").exists()
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()


def test_http_invalid_main_configuration_does_not_publish_claude_account(tmp_path):
    from tests.accounts_contract import PythonReference
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path, block_handlers=True)
    rust = RustCatalog(reference)
    try:
        (tmp_path / ".claude.json").write_text("{invalid", encoding="utf-8")
        result = rust.request("POST", "/api/claude-configs", {"nome": "fresh"})
        assert result.status_code == 500, result.text
        assert not (tmp_path / ".claude-fresh").exists()
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()

def test_worker_keeps_own_descriptor_through_two_cancellations(tmp_path, monkeypatch):
    import asyncio
    import threading
    import pytest
    from app import account_bridge, contas
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    instance = SimpleNamespace(instance="current")
    monkeypatch.setattr(runtime_coordinator, "current", lambda: instance)
    target = tmp_path / ".claude-fresh"
    target.mkdir()
    (target / ".hangar-account-pending").touch()
    key = account_lifecycle.AccountKey.new("claude", target)
    body = {"operation": "c" * 32, "instance": "current", "key": {
        "provider": "claude", "canonical_home": str(key.canonical_home)},
        "account_id": "fresh", "seed": True, "force": False, "cwd": None}
    root = account_lifecycle.default_lock_root()
    root.mkdir(parents=True)
    (root / (key.digest + ".prepare.json")).write_text(json.dumps(body), encoding="utf-8")
    entered, release = threading.Event(), threading.Event()
    original = contas.prepare_configuration
    def blocked(*args, **kwargs):
        entered.set()
        assert release.wait(10)
        return original(*args, **kwargs)
    monkeypatch.setattr(contas, "prepare_configuration", blocked)
    async def scenario():
        jobs = account_bridge.PreparationJobs()
        await jobs.start(body)
        task = jobs.jobs[body["operation"]][1]
        try:
            assert await asyncio.to_thread(entered.wait, 5)
            for _ in range(2):
                task.cancel()
                turn = asyncio.get_running_loop().create_future()
                asyncio.get_running_loop().call_soon(turn.set_result, None)
                await turn
                with pytest.raises(account_lifecycle.AccountLockError):
                    account_lifecycle.acquire(key, account_lifecycle.GuardMode.EXCLUSIVE, deadline=0)
            instance.instance = "replacement"
            with pytest.raises(ValueError):
                await jobs.start(body)
        finally:
            release.set()
        result = await task
        assert result["status"] == "error"
        with account_lifecycle.acquire(key, account_lifecycle.GuardMode.EXCLUSIVE, deadline=0):
            assert (target / ".claude.json").is_file()
    asyncio.run(scenario())


def test_worker_revalidates_instance_after_acquiring_its_descriptor(tmp_path, monkeypatch):
    import asyncio
    import threading
    from app import account_bridge
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    instance = SimpleNamespace(instance="current")
    monkeypatch.setattr(runtime_coordinator, "current", lambda: instance)
    target = tmp_path / ".claude-fresh"
    target.mkdir()
    (target / ".hangar-account-pending").touch()
    key = account_lifecycle.AccountKey.new("claude", target)
    body = {"operation": "d" * 32, "instance": "current", "key": {
        "provider": "claude", "canonical_home": str(key.canonical_home)},
        "account_id": "fresh", "seed": True, "force": False, "cwd": None}
    root = account_lifecycle.default_lock_root()
    root.mkdir(parents=True)
    (root / (key.digest + ".prepare.json")).write_text(json.dumps(body), encoding="utf-8")
    entered, release = threading.Event(), threading.Event()
    original = account_lifecycle.acquire
    def acquire(*args, **kwargs):
        entered.set()
        assert release.wait(10)
        return original(*args, **kwargs)
    monkeypatch.setattr(account_lifecycle, "acquire", acquire)
    async def scenario():
        jobs = account_bridge.PreparationJobs()
        await jobs.start(body)
        try:
            assert await asyncio.to_thread(entered.wait, 5)
            instance.instance = "replacement"
        finally:
            release.set()
        assert (await jobs.jobs[body["operation"]][1])["status"] == "error"
        assert list(target.iterdir()) == [target / ".hangar-account-pending"]
    asyncio.run(scenario())

def test_http_claude_validation_preserves_real_python_envelopes(tmp_path):
    from tests.accounts_contract import PythonReference, normalize
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path / "native", block_handlers=True)
    baseline = PythonReference(tmp_path / "baseline")
    rust = RustCatalog(reference)
    try:
        for body in ({"nome": "conta\n"}, {"nome": ""}, {"nome": "x" * 33},
                     {"nome": "fresh", "extra": True}, {"nome": "work"}, {}, {"nome": 17}):
            actual = rust.request("POST", "/api/claude-configs", body)
            expected = baseline.request("POST", "/api/claude-configs", body)
            assert (actual.status_code, normalize(actual.json(), root=reference.root)) == (
                expected.status_code, normalize(expected.json(), root=baseline.root)), body
        assert reference.calls() == []
    finally:
        rust.close()
        baseline.close()
        reference.close()


def test_http_pretrust_only_on_explicit_valid_cwd(tmp_path):
    from tests.accounts_contract import PythonReference
    from tests.test_accounts_catalog import RustCatalog
    import urllib.parse
    reference = PythonReference(tmp_path, block_handlers=True)
    rust = RustCatalog(reference)
    try:
        project = tmp_path / "project"
        project.mkdir()
        target = tmp_path / ".codex"
        before = set(target.iterdir())
        assert rust.request("GET", "/api/codex-contas/default/prepare").json()["status"] == "ready"
        assert set(target.iterdir()) == before
        result = rust.request("GET", "/api/codex-contas/default/prepare?cwd=" + urllib.parse.quote(str(project)))
        assert result.status_code == 200, result.text
        assert result.json()["status"] == "ready", result.json()
        import tomllib
        data = tomllib.loads((target / "config.toml").read_text(encoding="utf-8"))
        assert data["projects"][str(project)]["trust_level"] == "trusted"
        before_text = (target / "config.toml").read_bytes()
        assert rust.request("GET", "/api/codex-contas/default/prepare?cwd=relative").status_code == 400
        assert (target / "config.toml").read_bytes() == before_text
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()


async def test_valid_windows_toml_resource_is_prepared_without_changing_auth(isolated, fake_writer):
    import tomllib
    from app import codex_contas_sync as sync
    _, source, account = isolated
    hook = source / "hooks/probe.py"
    hook.parent.mkdir()
    hook.write_text("print('ok')", encoding="utf-8")
    agent = source / "agents/probe.toml"
    agent.parent.mkdir()
    agent.write_text("config_file = " + json.dumps(str(hook)) + "\n", encoding="utf-8")
    (source / "config.toml").write_text('model = "high"\n', encoding="utf-8")
    (source / "auth.json").write_text('{"token":"synthetic-source"}', encoding="utf-8")
    (account.home / "auth.json").write_text('{"token":"synthetic-own"}', encoding="utf-8")
    result = await sync.prepare_account(account)
    assert result["status"] == "ready", result
    data = tomllib.loads((account.home / "agents/probe.toml").read_text(encoding="utf-8"))
    assert data["config_file"] == str(account.home / "hooks/probe.py")
    assert json.loads((account.home / "auth.json").read_text(encoding="utf-8")) == {"token": "synthetic-own"}

def test_http_codex_catalog_reads_native_identity_with_real_preparation_status(tmp_path):
    from tests.accounts_contract import PythonReference, normalize
    from tests.test_accounts_catalog import RustCatalog
    roots = [tmp_path / "python", tmp_path / "rust"]
    for root in roots:
        system = root / ".codex/skills/.system"
        system.mkdir(parents=True)
        (system / "fixture.md").write_text("Configuração compartilhada de referência", encoding="utf-8")
    reference = PythonReference(roots[0])
    upstream = PythonReference(roots[1], block_handlers=True)
    rust = RustCatalog(upstream)
    try:
        expected = reference.request("GET", "/api/codex-contas")
        response = rust.request("GET", "/api/codex-contas")
        assert response.status_code == expected.status_code == 200
        assert normalize(response.json(), root=roots[1]) == normalize(expected.json(), root=roots[0])
        assert upstream.calls() == []
        assert upstream.request("GET", "/__contract__/preparation-calls").json() == []
    finally:
        rust.close()
        upstream.close()
        reference.close()


def test_http_codex_catalog_matches_empty_settings_with_equivalent_native_transport(tmp_path):
    from tests.accounts_contract import PythonReference, FIXTURES, normalize
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path, block_handlers=True)
    rust = RustCatalog(reference, native_fixture=True)
    try:
        expected = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))["codex_catalog"]
        response = rust.request("GET", "/api/codex-contas")
        assert response.status_code == expected["status"], response.text
        assert normalize(response.json(), root=tmp_path) == expected["body"]
        assert not (tmp_path / ".codex/skills").exists()
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()
