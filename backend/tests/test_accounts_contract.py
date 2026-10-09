"""Contratos públicos de contas; executar somente em VM ou CI isolado."""

import json
import pytest

from accounts_contract import (FIXTURES, PythonReference, assert_rust_ownership,
                               capture_reference, isolated_environment, normalize)


@pytest.fixture
def account_contract(tmp_path):
    reference = PythonReference(tmp_path / "home")
    try:
        yield reference
    finally:
        reference.close()


def test_ownership_rejects_successful_python_proxy():
    with pytest.raises(AssertionError, match="claude.catalog"):
        assert_rust_ownership([{"operation": "claude.catalog", "status": 200}])


def test_ownership_allows_delimited_preparation_bridge():
    assert_rust_ownership([{"operation": "bridge.prepare", "status": 200}])


def test_catalogue_keeps_disconnected_base(account_contract):
    response = account_contract.request("GET", "/api/claude-configs")
    assert response.status_code == 200
    assert any(row["active"] for row in response.json())
    assert any(row["label"] == "Trabalho de revisão" for row in response.json())


def test_python_routes_match_explicit_reference(account_contract):
    expected = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))
    actual = capture_reference(account_contract)
    assert actual["claude_state"]["status"] == actual["codex_catalog"]["status"] == 200
    assert [row["id"] for row in actual["codex_catalog"]["body"]] == ["default", "alpha", "zeta"]
    assert actual["codex_create"]["status"] == 201
    assert actual["codex_login_null"]["body"] is None
    assert actual["codex_delete"]["body"] == {"ok": True, "merged": 0, "skipped": 0, "renamed": 0}
    assert actual == expected


def test_blocked_python_handler_cannot_fake_rust_ownership(tmp_path):
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    try:
        response = reference.request("GET", "/api/claude-configs")
        assert response.status_code == 503
        assert response.json()["detail"]["code"] == "contract_python_handler_blocked"
        with pytest.raises(AssertionError, match="claude.catalog"):
            assert_rust_ownership(reference.calls())
    finally:
        reference.close()


def test_environment_discards_inherited_account_and_proxy(tmp_path, monkeypatch):
    for key in ("OPENAI_API_KEY", "CP_CLAUDE_CONFIG_DIRS", "HANGAR_SESSION_KEY", "TMUX", "HTTPS_PROXY"):
        monkeypatch.setenv(key, "inherited-synthetic")
    environment = isolated_environment(tmp_path)
    assert not {"OPENAI_API_KEY", "CP_CLAUDE_CONFIG_DIRS", "HANGAR_SESSION_KEY", "TMUX", "HTTPS_PROXY"} & environment.keys()
    assert environment["HOME"] == environment["USERPROFILE"] == str(tmp_path)
    assert environment["CLAUDE_CONFIG_DIR"] == str(tmp_path / ".claude")
    assert environment["CODEX_HOME"] == str(tmp_path / ".codex")
    assert environment["HOMEDRIVE"] + environment["HOMEPATH"] == str(tmp_path)


def test_normalization_keeps_semantic_values_and_omitted_fields(tmp_path):
    raw = {"path": str(tmp_path / ".claude"), "email": None, "label": "Revisão",
           "timestamp": 123.4, "uuid": "original", "items": [2, 1]}
    value = normalize(raw, root=tmp_path)
    assert value == {"path": "<HOME>/.claude", "email": None, "label": "Revisão",
                     "timestamp": 123.4, "uuid": "original", "items": [2, 1]}
    assert "plan" not in value


def test_worker_cleanup_collects_owned_process(tmp_path):
    reference = PythonReference(tmp_path / "home")
    reference.close()
    assert reference.process.poll() is not None


def test_codex_success_waits_for_identity_and_owns_http(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference)
    try:
        response = server.request("POST", "/api/codex-contas/alpha/login")
        assert response.status_code == 200, response.json()
        attempt = response.json()
        assert attempt["status"] == "waiting"
        assert attempt["verification_url"] == "https://example.test/device"
        assert attempt["user_code"] == "fixture-code"
        import time
        deadline = time.monotonic() + 10
        while not any(call["method"] == "account/read" for call in server.native_calls("alpha")):
            assert time.monotonic() < deadline, "sucesso não iniciou confirmação de identidade"
        assert server.request("GET", "/api/codex-contas/alpha/login").json()["status"] == "waiting"
        account = reference.root / ".codex-alpha"
        assert reference.request("POST", "/__contract__/codex-model-cache").json()["cached"]
        (account / "identity.json").write_text(json.dumps({
            "type": "chatgpt", "email": "fixture@example.test", "planType": "plus",
        }), encoding="utf-8")
        final = server.wait_status("alpha", "completed")
        assert final == {**attempt, "status": "completed"}
        assert not reference.request("GET", "/__contract__/codex-model-cache").json()["cached"], "login concluído invalida o catálogo de modelos"
        catalog = server.request("GET", "/api/codex-contas").json()
        assert next(row for row in catalog if row["id"] == "alpha")["auth"] == {
            "method": "oauth", "status": "connected", "email": "fixture@example.test", "plan": "plus",
        }
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()


def test_codex_cancel_old_attempt_does_not_stop_new_helper(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex-alpha"
    (account / "emit-success.json").write_text("false", encoding="utf-8")
    (account / "spawn-descendant.json").write_text("true", encoding="utf-8")
    server = RustCodex(reference)
    try:
        first = server.request("POST", "/api/codex-contas/alpha/login").json()
        assert "attempt_id" in first, first
        assert server.request("POST", "/api/codex-contas/alpha/login").json() == first
        import psutil
        owned = [psutil.Process(json.loads((account / name).read_text(encoding="utf-8")))
                 for name in ("native-pid.json", "descendant-pid.json")]
        assert all(process.is_running() for process in owned)
        cancelled = server.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + first["attempt_id"])
        assert cancelled.status_code == 200
        assert cancelled.json() == {**first, "status": "cancelled"}
        assert all(not process.is_running() or process.status() == psutil.STATUS_ZOMBIE for process in owned), "cancelamento respondeu antes de encerrar a árvore"
        assert server.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + first["attempt_id"]).json() == cancelled.json()
        second = server.request("POST", "/api/codex-contas/alpha/login").json()
        assert second["attempt_id"] != first["attempt_id"]
        stale = server.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + first["attempt_id"])
        assert stale.status_code == 409
        assert stale.json()["detail"]["code"] == "codex_login_attempt_mismatch"
        assert server.request("GET", "/api/codex-contas/alpha/login").json() == second
        assert server.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + second["attempt_id"]).json()["status"] == "cancelled"
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_early_completion_survives_obsolete_event_burst(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex-alpha"
    (account / "obsolete-events.json").write_text("512", encoding="utf-8")
    (account / "identity.json").write_text(json.dumps({
        "type": "chatgpt", "email": "fixture@example.test", "planType": "plus",
    }), encoding="utf-8")
    server = RustCodex(reference)
    try:
        assert server.request("POST", "/api/codex-contas/alpha/login").status_code == 200
        assert server.wait_status("alpha", "completed")["status"] == "completed"
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_missing_cli_reports_failure_without_fallback(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference, missing_cli=True)
    try:
        response = server.request("POST", "/api/codex-contas/alpha/login")
        assert response.status_code == 200
        assert response.json()["status"] == "failed"
        assert response.json()["error"] == {"code": "codex_account_cli_missing", "params": {}}
        assert server.native_calls("alpha") == []
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_empty_identity_reopens_helper_and_fails(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference)
    try:
        assert server.request("POST", "/api/codex-contas/alpha/login").json()["status"] == "waiting"
        final = server.wait_status("alpha", "failed")
        assert final["error"] == {"code": "codex_account_login_failed", "params": {}}
        calls = server.native_calls("alpha")
        assert sum(call["method"] == "initialize" for call in calls) == 2
        assert sum(call["method"] == "account/login/start" for call in calls) == 1
        assert sum(call["method"] == "account/read" for call in calls) >= 2
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_identity_changed_during_read_never_completes(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex-alpha"
    (account / "mutate-auth-on-read.json").write_text("true", encoding="utf-8")
    (account / "identity.json").write_text(json.dumps({
        "type": "chatgpt", "email": "fixture@example.test", "planType": "plus",
    }), encoding="utf-8")
    server = RustCodex(reference)
    try:
        response = server.request("POST", "/api/codex-contas/alpha/login")
        assert response.status_code == 200
        final = server.wait_status("alpha", "failed")
        assert final["error"] == {"code": "codex_account_login_failed", "params": {}}
        assert sum(call["method"] == "initialize" for call in server.native_calls("alpha")) == 2
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_restart_cleans_attempt_and_does_not_restore_old_id(tmp_path):
    from accounts_contract import RustCodex
    import psutil
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex-alpha"
    (account / "emit-success.json").write_text("false", encoding="utf-8")
    server = RustCodex(reference)
    try:
        first = server.request("POST", "/api/codex-contas/alpha/login").json()
        assert first["status"] == "waiting"
        process = psutil.Process(json.loads((account / "native-pid.json").read_text(encoding="utf-8")))
        server.close()
        assert not process.is_running(), "reinício soltou a conta com auxiliar vivo"
        server = RustCodex(reference)
        assert server.request("GET", "/api/codex-contas/alpha/login").json() is None
        second = server.request("POST", "/api/codex-contas/alpha/login").json()
        assert second["attempt_id"] != first["attempt_id"]
        stale = server.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + first["attempt_id"])
        assert stale.status_code == 409
        assert server.request("GET", "/api/codex-contas/alpha/login").json() == second
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()


@pytest.mark.parametrize("drain", [False, True])
def test_codex_shutdown_does_not_abandon_owned_auth_reader(tmp_path, drain):
    import concurrent.futures
    import psutil
    import time
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex"
    for name in ("spawn-reader-descendant.json", "hold-read.json"):
        (account / name).write_text("true", encoding="utf-8")
    server = RustCodex(reference)
    owned = []
    pool = concurrent.futures.ThreadPoolExecutor()
    closed = False
    try:
        reading = pool.submit(server.request, "GET", "/api/codex-contas")
        deadline = time.monotonic() + 10
        while not (account / "read-entered.json").exists():
            assert time.monotonic() < deadline, "O leitor não iniciou account/read."
        for pid in json.loads((account / "reader-pids.json").read_text(encoding="utf-8")):
            process = psutil.Process(pid)
            owned.append((process, process.create_time()))
        assert all(process.is_running() for process, _ in owned)
        if drain:
            response = reading.result(timeout=30)
            assert response.status_code == 200
        server.close()
        closed = True
        live = []
        for process, born in owned:
            try:
                if process.create_time() == born and process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                    live.append({"pid": process.pid, "name": process.name()})
            except psutil.NoSuchProcess:
                pass
        print(json.dumps({"drain": drain, "survivors": live}), flush=True)
        assert not live, "Encerramento controlado devolveu a posse com descendente do leitor vivo."
    finally:
        if not closed:
            server.close()
        for process, born in owned:
            try:
                if process.create_time() == born and process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                    process.kill()
                    process.wait(timeout=10)
            except psutil.NoSuchProcess:
                pass
        pool.shutdown(wait=True)
        reference.close()


def test_codex_lost_http_response_keeps_owned_auth_reader_until_cleanup(tmp_path):
    import concurrent.futures
    import psutil
    import socket
    import time
    from urllib.request import Request
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    account = reference.root / ".codex"
    for name in ("spawn-reader-descendant.json", "hold-read.json"):
        (account / name).write_text("true", encoding="utf-8")
    (account / "identity.json").write_text(json.dumps({
        "type": "chatgpt", "email": "fixture@example.test", "planType": "plus",
    }), encoding="utf-8")
    server = RustCodex(reference)
    owned = []
    try:
        def lose_response():
            request = Request(server.base_url + "/api/codex-contas",
                              headers={"Authorization": "Bearer " + server.token})
            try:
                with server.opener.open(request, timeout=0.4) as response:
                    response.read()
            except (TimeoutError, socket.timeout):
                return "lost"
            raise AssertionError("A resposta não ficou retida pela barreira nativa.")
        with concurrent.futures.ThreadPoolExecutor() as pool:
            lost = pool.submit(lose_response)
            deadline = time.monotonic() + 10
            while not (account / "read-entered.json").exists():
                assert time.monotonic() < deadline, "O leitor não chegou à barreira."
            for pid in json.loads((account / "reader-pids.json").read_text(encoding="utf-8")):
                process = psutil.Process(pid)
                owned.append((process, process.create_time()))
            assert lost.result(timeout=5) == "lost"
        assert all(process.is_running() for process, _ in owned), "Perder a resposta abandonou a operação."
        (account / "release-read.json").write_text("true", encoding="utf-8")
        deadline = time.monotonic() + 10
        while True:
            live = []
            for process, born in owned:
                try:
                    if process.create_time() == born and process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                        live.append(process.pid)
                except psutil.NoSuchProcess:
                    pass
            if not live:
                break
            assert time.monotonic() < deadline, "A operação não limpou sua árvore após a resposta perdida."
        response = server.request("GET", "/api/codex-contas")
        assert response.status_code == 200
        assert next(row for row in response.json() if row["id"] == "default")["auth"]["email"] == "fixture@example.test"
        assert_rust_ownership(reference.calls())
    finally:
        (account / "release-read.json").write_text("true", encoding="utf-8")
        server.close()
        for process, born in owned:
            try:
                if process.create_time() == born and process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                    process.kill()
                    process.wait(timeout=10)
            except psutil.NoSuchProcess:
                pass
        reference.close()

def test_codex_birth_blocks_login_before_and_after_restart(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    barrier = reference.block("session_before_registration")
    birth = reference.start_session_async(provider="codex", account_id="alpha")
    server = RustCodex(reference)
    try:
        assert barrier.entered.wait()
        for index in range(2):
            response = server.request("POST", "/api/codex-contas/alpha/login")
            assert response.status_code == 409
            assert response.json()["detail"]["code"] == "codex_account_in_use"
            assert server.native_calls("alpha") == []
            if index == 0:
                server.close()
                server = RustCodex(reference)
        barrier.release.set()
        assert birth.result(timeout=20).status_code == 200
    finally:
        barrier.release.set()
        server.close()
        reference.close()

def test_codex_uncertain_usage_refuses_login_without_cli(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference)
    try:
        reference.request("POST", "/__contract__/runtime-instance", {"instance": "obsolete"})
        response = server.request("POST", "/api/codex-contas/alpha/login")
        assert response.status_code == 409
        assert response.json()["detail"]["code"] == "account_usage_unknown"
        assert server.native_calls("alpha") == []
        assert server.request("GET", "/api/codex-contas/alpha/login").json() is None
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_codex_cache_callback_requires_secret_instance_and_valid_key(tmp_path):
    from urllib.request import Request
    from urllib.error import HTTPError
    reference = PythonReference(tmp_path / "home")
    def invalidate(body, *, secret="contract-internal", instance="contract-instance"):
        request = Request(reference.base_url + "/internal/accounts/codex-invalidate",
                          data=json.dumps(body).encode(), method="POST", headers={
                              "Content-Type": "application/json",
                              "x-hangar-internal": secret,
                              "x-hangar-runtime-instance": instance,
                          })
        try:
            response = reference.opener.open(request, timeout=5)
        except HTTPError as error:
            response = error
        with response:
            return response.status
    try:
        key = {"key": {"provider": "codex", "canonical_home": str(reference.root / ".codex-alpha")}}
        reference.request("POST", "/__contract__/codex-model-cache")
        assert invalidate(key, secret="wrong") == 404
        assert invalidate(key, instance="old") == 404
        assert reference.request("GET", "/__contract__/codex-model-cache").json()["cached"]
        assert invalidate({"key": {"provider": "codex", "canonical_home": "relative"}}) == 400
        assert invalidate({**key, "token": "synthetic"}) == 400
        assert invalidate(key) == 200
        assert not reference.request("GET", "/__contract__/codex-model-cache").json()["cached"]
    finally:
        reference.close()

def test_codex_python_consumers_delegate_and_pending_does_not_fallback(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home")
    account = reference.root / ".codex-alpha"
    (account / "emit-success.json").write_text("false", encoding="utf-8")
    server = RustCodex(reference)
    try:
        reference.request("POST", "/__contract__/claude-owner", {
            "address": server.request("GET", "/__hangar_server/health").json()["terminal_address"], "mode": "rust",
        })
        assert reference.request("GET", "/api/codex-contas/alpha/login").json() is None
        attempt = reference.request("POST", "/api/codex-contas/alpha/login").json()
        assert attempt["status"] == "waiting"
        assert server.request("GET", "/api/codex-contas/alpha/login").json() == attempt
        cancelled = reference.request("DELETE", "/api/codex-contas/alpha/login?attempt_id=" + attempt["attempt_id"])
        assert cancelled.json()["status"] == "cancelled"
        assert sum(call["method"] == "account/login/start" for call in server.native_calls("alpha")) == 1
        reference.request("POST", "/__contract__/claude-owner", {
            "address": "127.0.0.1:1", "mode": "pending",
        })
        for method in ("POST", "GET", "DELETE"):
            response = reference.request(method, "/api/codex-contas/alpha/login?attempt_id=" + attempt["attempt_id"])
            assert response.status_code == 503
            assert response.json()["detail"]["code"] == "account_auth_bridge_unavailable"
        assert sum(call["method"] == "account/login/start" for call in server.native_calls("alpha")) == 1
    finally:
        server.close()
        reference.close()


def test_codex_native_identity_cache_is_bound_to_auth_files(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference)
    account = reference.root / ".codex-alpha"
    def identity(email):
        (account / "identity.json").write_text(json.dumps({
            "type": "chatgpt", "email": email, "planType": "plus",
        }), encoding="utf-8")
    def auth():
        response = server.request("GET", "/api/codex-contas")
        assert response.status_code == 200
        return next(row for row in response.json() if row["id"] == "alpha")["auth"]
    try:
        identity("first@example.test")
        first = auth()
        assert first["email"] == "first@example.test"
        identity("second@example.test")
        assert auth() == first, "arquivo intacto conserva o cache de 60 s"
        (account / "auth.json").write_text('{"synthetic":"changed"}', encoding="utf-8")
        assert auth()["email"] == "second@example.test", "credencial trocada invalida o cache"
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()


def test_codex_login_preserves_storage_and_query_errors(tmp_path):
    from accounts_contract import RustCodex
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustCodex(reference)
    account = reference.root / ".codex-alpha"
    try:
        for config, code in [
            ("[tools]\ncli_auth_credentials_store='file'\n", "codex_account_auth_storage_invalid"),
            ("cli_auth_credentials_store='file'\ncli_auth_credentials_store='keyring'\n", "codex_account_prepare_required"),
        ]:
            (account / "config.toml").write_text(config, encoding="utf-8")
            response = server.request("POST", "/api/codex-contas/alpha/login")
            assert response.status_code == 409
            assert response.json()["detail"]["code"] == code
            assert not (account / "native-pid.json").exists(), "armazenamento inválido não abre auxiliar"
        response = server.request("DELETE", "/api/codex-contas/alpha/login")
        assert response.status_code == 422
        assert response.json()["detail"][0]["loc"] == ["query", "attempt_id"]
        missing = server.request("GET", "/api/codex-contas/missing/login")
        assert missing.status_code == 404
        assert missing.json()["detail"]["code"] == "codex_account_not_found"
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_rust_claude_logout_remains_allowed_with_live_session(tmp_path):
    from accounts_contract import RustClaude
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    import json
    account = reference.root / ".claude-work"
    (account / "auth-reply.json").write_text('{"loggedIn":true}', encoding="utf-8")
    (account / ".credentials.json").write_text(
        json.dumps({"claudeAiOauth": {"accessToken": "synthetic-live"}}), encoding="utf-8")
    try:
        # Um runtime vivo é fato confirmado, não uma guarda de nascimento eterna.
        live = reference.request("POST", "/__contract__/launcher-options", {"live_claude": True})
        assert live.json()["pid"] > 0
        response = server.request("POST", "/api/claude-configs/Trabalho%20de%20revis%C3%A3o/logout")
        assert response.status_code == 200
        assert response.json() == {"ok": True}
        state = server.request("GET", "/api/conta-estado").json()
        row = next(r for r in state if r["label"] == "Trabalho de revisão")
        assert row["login"]["estado"] == "ok" and row["login"]["loggedIn"] is False
        assert not (account / ".credentials.json").exists()
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()
