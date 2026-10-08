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
    assert actual["codex_delete"]["body"] == {"ok": True}
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
