"""Login Claude pelo Rust (`/api/conta-estado/{label}/login…`) contra a referência Python isolada.

O Rust decide e confirma o login; o Python só opera a janela escondida pela ponte
(`/internal/accounts/claude-window`) e nenhum handler de conta Python atende o pedido.
"""
import pytest


def test_rust_login_requires_changed_token_and_authenticated_identity(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    try:
        credential = account / ".credentials.json"
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        reply = account / "auth-reply.json"
        reply.write_text(json.dumps({"loggedIn": True, "email": "fixture@example.test", "subscriptionType": "pro"}), encoding="utf-8")
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        response = server.request("POST", path)
        assert response.status_code == 200, "o Rust deve abrir o login sem delegar ao handler Python bloqueado"
        assert server.request("GET", path + "/passo").json() == {
            "etapa": "aguardando", "url": "https://claude.ai/oauth/authorize?fixture=1", "email": None, "plano": None}
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        reply.write_text('{"loggedIn":false}', encoding="utf-8")
        assert server.request("GET", path + "/passo").json()["etapa"] == "aguardando"
        reply.write_text(json.dumps({"loggedIn": True, "email": "fixture@example.test", "subscriptionType": "pro"}), encoding="utf-8")
        assert server.request("GET", path + "/passo").json() == {
            "etapa": "concluido", "url": None, "email": "fixture@example.test", "plano": "pro"}
        assert json.loads((account / ".claude.json").read_text(encoding="utf-8"))["hasCompletedOnboarding"] is True
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()

def test_rust_restart_closes_only_the_abandoned_login_window(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        assert server.request("POST", path).status_code == 200
        old = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert len(old) == 1
        server.close()
        server = RustClaude(reference)
        assert server.request("GET", path + "/passo").json()["etapa"] == "idle"
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
        assert server.request("POST", path).status_code == 200
        new = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert len(new) == 1 and new != old
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
    finally:
        server.close()
        reference.close()

def test_rust_old_confirmation_cannot_clean_or_complete_a_new_attempt(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    from concurrent.futures import ThreadPoolExecutor
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    pool = ThreadPoolExecutor()
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        account = reference.root / ".claude-work"
        credential = account / ".credentials.json"
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        (account / "auth-reply.json").write_text('{"loggedIn":true}', encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        old = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        confirming = pool.submit(server.request, "POST", path + "/codigo", {"codigo": "synthetic-code-not-in-argv"})
        assert reference.request("GET", "/__contract__/wait-claude-code").json()["entered"]
        assert server.request("POST", path + "/cancelar").json() == {"ok": True}
        assert server.request("POST", path).status_code == 200
        new = reference.request("GET", "/__contract__/claude-windows").json()["windows"]
        assert new != old and len(new) == 1
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        assert confirming.result(timeout=20).status_code == 409
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == new
        assert server.request("GET", path + "/passo").json()["etapa"] == "concluido"
        assert "synthetic-code-not-in-argv" not in json.dumps(reference.calls())
        assert "synthetic-code-not-in-argv" not in (reference.root / "worker.log").read_text(encoding="utf-8")
        assert_rust_ownership(reference.calls())
    finally:
        server.close()
        reference.close()
        pool.shutdown(wait=True)


@pytest.mark.parametrize("reply", ['{"loggedIn":"true"}', 'invalid-json'])
def test_rust_unreadable_identity_cleans_attempt_without_onboarding(tmp_path, reply):
    from accounts_contract import PythonReference, RustClaude
    import json
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    try:
        path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
        assert server.request("POST", path).status_code == 200
        (account / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-new"}}), encoding="utf-8")
        (account / "auth-reply.json").write_text(reply, encoding="utf-8")
        response = server.request("GET", path + "/passo")
        assert response.status_code == 409
        assert not (account / ".claude.json").exists()
        assert reference.request("GET", "/__contract__/claude-windows").json()["windows"] == []
    finally:
        server.close()
        reference.close()

def test_python_consumer_uses_private_rust_auth_for_equivalent_account_paths(tmp_path):
    from accounts_contract import PythonReference, RustClaude, assert_rust_ownership
    from urllib.parse import quote
    reference = PythonReference(tmp_path / "home", block_handlers=True)
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


def test_step_does_not_conclude_when_credential_changes_during_identity_probe(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    import json
    import os
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    credential = account / ".credentials.json"
    path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
    try:
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-initial"}}), encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-before-probe"}}), encoding="utf-8")
        source = """const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;
const marker=p.join(d,'probe-raced');
if(!fs.existsSync(marker)){
  fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-after-probe'}}));
  fs.writeFileSync(marker,'1');
  process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));
}else{process.stdout.write(JSON.stringify({loggedIn:false}));}
"""
        fixture = reference.root / "claude-native"
        (fixture / "node_modules/@anthropic-ai/claude-code/cli.js").write_text(source, encoding="utf-8")
        if os.name != "nt":
            (fixture / "claude").write_text("#!/usr/bin/env node\n" + source, encoding="utf-8")
        response = server.request("GET", path + "/passo")
        assert response.status_code == 200
        assert (account / "probe-raced").exists()
        assert json.loads(credential.read_text())["claudeAiOauth"]["accessToken"] == "synthetic-after-probe"
        assert response.json()["etapa"] == "aguardando", "identidade da credencial anterior não comprova autenticação do token atual"
        assert len(reference.request("GET", "/__contract__/claude-windows").json()["windows"]) == 1
        assert not (account / ".claude.json").exists()
    finally:
        server.close()
        reference.close()


def test_confirmation_does_not_accept_identity_read_before_token_replacement(tmp_path):
    from accounts_contract import PythonReference, RustClaude
    from concurrent.futures import ThreadPoolExecutor
    import json
    import os
    import time
    reference = PythonReference(tmp_path / "home", block_handlers=True)
    server = RustClaude(reference)
    account = reference.root / ".claude-work"
    credential = account / ".credentials.json"
    path = "/api/conta-estado/Trabalho%20de%20revis%C3%A3o/login"
    pool = ThreadPoolExecutor()
    try:
        credential.write_text(json.dumps({"claudeAiOauth": {"accessToken": "synthetic-old"}}), encoding="utf-8")
        assert server.request("POST", path).status_code == 200
        source = """const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;
const marker=p.join(d,'probe-count');
const count=fs.existsSync(marker)?Number(fs.readFileSync(marker,'utf8'))+1:1;
if(count===1){
  fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-new'}}));
  process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));
}else{process.stdout.write(JSON.stringify({loggedIn:false}));}
fs.writeFileSync(marker,String(count));
"""
        fixture = reference.root / "claude-native"
        (fixture / "node_modules/@anthropic-ai/claude-code/cli.js").write_text(source, encoding="utf-8")
        if os.name != "nt":
            (fixture / "claude").write_text("#!/usr/bin/env node\n" + source, encoding="utf-8")
        confirming = pool.submit(server.request, "POST", path + "/codigo", {"codigo": "synthetic-code"})
        marker = account / "probe-count"
        deadline = time.monotonic() + 15
        while not confirming.done():
            if marker.exists() and marker.read_text() not in {"", "1"}:
                break
            assert time.monotonic() < deadline, "a releitura nativa não avançou"
        assert marker.exists()
        assert server.request("POST", path + "/cancelar").status_code == 200
        response = confirming.result(timeout=20)
        assert response.status_code == 409, "confirmação com identidade anterior deve permanecer pendente até cancelar"
        assert not (account / ".claude.json").exists()
    finally:
        server.close()
        reference.close()
        pool.shutdown(wait=True)
