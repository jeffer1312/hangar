import json


def test_http_claude_seed_preserves_configuration_and_does_not_publish_oauth(tmp_path):
    from tests.accounts_contract import PythonReference
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path)
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
    reference = PythonReference(tmp_path)
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

def test_http_claude_validation_preserves_real_python_envelopes(tmp_path):
    from tests.accounts_contract import FIXTURES, PythonReference, normalize
    from tests.test_accounts_catalog import RustCatalog
    golden = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))
    reference = PythonReference(tmp_path / "native")
    rust = RustCatalog(reference)
    try:
        for expected in golden["claude_validation_envelopes"]:
            actual = rust.request(expected["method"], expected["path"], expected["body"])
            assert (actual.status_code, normalize(actual.json(), root=reference.root)) == (
                expected["status"], expected["response"]), expected["body"]
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()


def test_http_codex_catalog_reads_native_identity_with_real_preparation_status(tmp_path):
    from tests.accounts_contract import FIXTURES, PythonReference, normalize
    from tests.test_accounts_catalog import RustCatalog
    expected = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))[
        "codex_catalog_with_shared_settings"]
    next(account for account in expected["body"] if account["id"] == "default")["sync"]["status"] = "idle"
    root = tmp_path / "rust"
    system = root / ".codex/skills/.system"
    system.mkdir(parents=True)
    (system / "fixture.md").write_text("Configuração compartilhada de referência", encoding="utf-8")
    upstream = PythonReference(root)
    # A preparação é real; a identidade nativa tem resposta controlada, sem depender da partida do CLI.
    rust = RustCatalog(upstream, native_fixture=True)
    try:
        response = rust.request("GET", "/api/codex-contas")
        assert response.status_code == expected["status"] == 200
        assert normalize(response.json(), root=root) == expected["body"]
        assert upstream.calls() == []
    finally:
        rust.close()
        upstream.close()


def test_http_codex_catalog_matches_empty_settings_with_equivalent_native_transport(tmp_path):
    from tests.accounts_contract import PythonReference, FIXTURES, normalize
    from tests.test_accounts_catalog import RustCatalog
    reference = PythonReference(tmp_path)
    rust = RustCatalog(reference, native_fixture=True)
    try:
        expected = json.loads((FIXTURES / "python-reference.json").read_text(encoding="utf-8"))["codex_catalog"]
        next(account for account in expected["body"] if account["id"] == "default")["sync"]["status"] = "idle"
        response = rust.request("GET", "/api/codex-contas")
        assert response.status_code == expected["status"], response.text
        assert normalize(response.json(), root=tmp_path) == expected["body"]
        assert not (tmp_path / ".codex/skills").exists()
        assert reference.calls() == []
    finally:
        rust.close()
        reference.close()
