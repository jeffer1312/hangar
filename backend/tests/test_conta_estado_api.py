"""O consumidor Python da autenticação Claude pergunta ao Rust pela porta privada (`request_claude`)."""


def test_python_consumer_uses_private_rust_auth_for_equivalent_account_paths(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    from urllib.parse import quote
    reference = PythonReference(tmp_path / "home")
    server = RustClaude(reference)
    try:
        account = reference.root / ".claude-work"
        (account / "auth-reply.json").write_text('{"loggedIn":true,"email":"fixture@example.test"}', encoding="utf-8")
        address = server.request("GET", "/__hangar_server/health").json()["terminal_address"]
        assert reference.request("POST", "/__contract__/claude-owner", {"address": address}).json() == {"ok": True}
        normal = reference.request("GET", "/__contract__/claude-auth?path=" + quote(str(account), safe=""))
        assert normal.status_code == 200 and normal.json()["loggedIn"] is True
        alias = str(account / ".." / ".claude-work")
        reply = reference.request("GET", "/__contract__/claude-auth?path=" + quote(alias, safe=""))
        assert reply.status_code == 200, "o consumidor Python deve resolver a conta pela identidade canônica"
        assert reply.json()["loggedIn"] is True
        assert reply.json()["email"] == "fixture@example.test"
        assert server.request("POST", "/__hangar_server/accounts/claude", {"action": "auth", "path": alias}).status_code == 404
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()
