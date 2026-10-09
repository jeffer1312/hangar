"""Autoria HTTP e exclusão durante o nascimento real entre Python e Rust."""
from __future__ import annotations

import json
import os
from pathlib import Path
import queue
import subprocess
import threading

import pytest

from accounts_contract import PythonReference, HttpTransport, isolated_environment, normalize, assert_rust_ownership, FIXTURES


class RustCatalog(HttpTransport):
    def __init__(self, reference: PythonReference, *, instance="contract-instance", native_fixture=False):
        target = Path(os.environ.get("CARGO_TARGET_DIR") or Path(__file__).resolve().parents[2] / "crates/target") / "debug" / "deps"
        candidates = [path for path in target.glob("accounts_catalog-*.exe" if os.name == "nt" else "accounts_catalog-*")
                      if path.is_file() and path.suffix not in {".d", ".pdb", ".lib", ".exp"}]
        assert candidates, "compile accounts_catalog antes desta prova"
        binary = max(candidates, key=lambda path: path.stat().st_mtime)
        environment = isolated_environment(reference.root)
        if native_fixture:
            fixture_root = reference.root / "native-fixture"
            script = fixture_root / "node_modules/@openai/codex/bin/codex.js"
            script.parent.mkdir(parents=True, exist_ok=True)
            source = Path(__file__).parent / "fixtures/disconnected-codex.cjs"
            script.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
            if os.name != "nt":
                executable = fixture_root / "codex"
                executable.write_text("#!/usr/bin/env node\n" + source.read_text(encoding="utf-8"), encoding="utf-8")
                executable.chmod(0o700)
            environment["PATH"] = str(fixture_root) + os.pathsep + environment["PATH"]
        environment.update(ACCOUNT_HTTP_UPSTREAM=reference.base_url.removeprefix("http://"),
                           HANGAR_RUNTIME_INSTANCE=instance)
        self.process = subprocess.Popen([str(binary), "--exact", "http_probe_process", "--nocapture"],
                                        env=environment, cwd=reference.root, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding="utf-8")
        ready = queue.Queue()
        def read():
            for line in self.process.stdout:
                if line.startswith("ACCOUNT_HTTP:"):
                    ready.put(line.strip().removeprefix("ACCOUNT_HTTP:"))
                    return
            ready.put(None)
        threading.Thread(target=read, daemon=True).start()
        address = ready.get(timeout=30)
        assert address, "servidor de prova não anunciou HTTP"
        super().__init__("http://" + address, "contract-only")

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=15)
        self.process.stdout.close()
        self.process.stderr.close()


def test_claimed_http_routes_match_goldens_without_python_handlers(tmp_path):
    reference = PythonReference(tmp_path / "home")
    server = RustCatalog(reference)
    golden = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))
    try:
        for case, payload in [
            ("claude_catalog", None), ("codex_missing", None), ("codex_protected", None),
            ("codex_invalid_name", {"name": "conta\n"}),
            ("codex_extra_field", {"name": "fresh", "extra": True}),
            ("codex_create", {"name": "fresh"}), ("codex_duplicate", {"name": "fresh"}),
            ("codex_delete", None),
        ]:
            expected = golden[case]
            response = server.request(expected["method"], expected["path"], payload)
            assert response.status_code == expected["status"], (case, response.json())
            assert normalize(response.json(), root=reference.root) == expected["body"], case
            if "tree" in expected:
                assert reference.tree("fresh") == expected["tree"]
        assert_rust_ownership(reference.calls())
        assert reference.calls() == []
    finally:
        server.close()
        reference.close()


@pytest.mark.parametrize("cancelled", [False, True])
def test_http_delete_is_blocked_after_creator_returns_and_across_rust_restart(tmp_path, cancelled):
    reference = PythonReference(tmp_path / "home")
    server = RustCatalog(reference)
    try:
        launcher = reference.block("launcher_before_publication")
        if cancelled:
            creation = reference.block("session_before_registration")
        birth = reference.start_session_async(provider="codex", account_id="alpha")
        if cancelled:
            assert creation.entered.wait()
            assert reference.cancel_birth().json()["cancelled"]
            assert reference.cancel_birth().json()["cancelled"]
            creation.release.set()
        assert birth.result(timeout=20).status_code == (499 if cancelled else 200)
        assert launcher.entered.wait()
        for attempt in range(2):
            response = server.request("DELETE", "/api/codex-contas/alpha")
            assert response.status_code == 409, response.json()
            assert response.json() == {"detail": {"code": "codex_account_in_use",
                                                   "params": {"account_id": "alpha"},
                                                   "msg": "conta Codex está em uso"}}
            assert reference.account_exists("codex", "alpha")
            if attempt == 0:
                server.close()
                server = RustCatalog(reference)
        assert reference.calls() == []
        launcher.release.set()
        assert reference.request("GET", "/__contract__/published").json()["published"]
    finally:
        server.close()
        reference.close()


@pytest.mark.parametrize("identity", ["known", "unknown", "reused_name"])
def test_http_delete_keeps_renamed_boot_protected(tmp_path, identity):
    reference = PythonReference(tmp_path / "home")
    server = RustCatalog(reference)
    try:
        assert reference.request("POST", "/__contract__/launcher-options", {
            "opaque": True, "unknown_instance": identity == "unknown"}).status_code == 200
        launcher = reference.block("launcher_before_publication")
        birth = reference.start_session_async(provider="codex", account_id="alpha")
        assert birth.result(timeout=20).status_code == 200
        assert launcher.entered.wait()
        renamed = reference.request("POST", "/api/sessions/birth/rename", {"new": "renamed"})
        assert renamed.status_code == 200, renamed.json()
        if identity == "reused_name":
            assert reference.request("POST", "/__contract__/reuse-name/birth").status_code == 200
        assert reference.request("POST", "/__contract__/retire-birth").status_code == 200
        for attempt in range(2):
            response = server.request("DELETE", "/api/codex-contas/alpha")
            assert response.status_code == 409, response.json()
            expected = "account_usage_unknown" if identity == "unknown" else "codex_account_in_use"
            assert response.json()["detail"]["code"] == expected
            assert reference.account_exists("codex", "alpha")
            assert list((reference.root / ".hangar/account-locks/births").glob("*.json"))
            if attempt == 0:
                server.close()
                server = RustCatalog(reference)
        assert reference.calls() == []
    finally:
        server.close()
        reference.close()


def test_python_readers_do_not_select_pending_accounts(tmp_path, monkeypatch):
    from app import contas, codex_contas, config
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    for provider, marker in [("claude", ".hangar-conta"), ("codex", ".hangar-codex-conta")]:
        target = tmp_path / f".{provider}-partial"
        target.mkdir()
        (target / marker).write_text(json.dumps({"version": 1, "id": "partial"}), encoding="utf-8")
        (target / ".hangar-account-pending").touch()
        (target / ".credentials.json").write_text("{}", encoding="utf-8")
        (target / "projects").mkdir()
    assert contas.listar() == []
    assert [account.id for account in codex_contas.list_accounts()] == ["default"]
    assert not config._is_config_dir(tmp_path / ".claude-partial")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", "Parcial:" + str(tmp_path / ".claude-partial"))
    assert config.list_config_dirs() == []

def test_claimed_validation_matches_real_python_envelopes(tmp_path):
    golden = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))
    reference = PythonReference(tmp_path / "native")
    server = RustCatalog(reference)
    try:
        differences = []
        for expected in golden["catalog_validation_envelopes"]:
            actual = server.request(expected["method"], expected["path"], expected["body"])
            left = (actual.status_code, normalize(actual.json(), root=reference.root))
            right = (expected["status"], expected["response"])
            if left != right:
                differences.append((expected["method"], expected["path"], expected["body"], left, right))
        assert not differences, differences
        assert reference.calls() == []
    finally:
        server.close()
        reference.close()

def test_environment_matches_shared_golden(tmp_path, monkeypatch):
    from app.codex_contas import Account, environment
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    fixture = json.loads((FIXTURES / "environment.json").read_text(encoding="utf-8"))
    def materialize(name):
        return {key: str(tmp_path / value.removeprefix("<HOME>").lstrip("/"))
                if value.startswith("<HOME>") else value for key, value in fixture[name].items()}
    for account_id, is_default, golden in [("default", True, "default"), ("extra", False, "additional")]:
        home = tmp_path / ("captured" if is_default else ".codex-extra")
        assert environment(Account(account_id, home, is_default), base=materialize("base")) == materialize(golden)
