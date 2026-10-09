"""Login OAuth do ChatGPT feito pelo app e espalhado pros CLIs (app/oauth_codex.py).

O que trava: a propagação grava o cofre (0600) e os três stores no formato de cada um; store que
já tem login é mantido; CLI ausente é `nao-instalado`, não erro. O fluxo de dispositivo é do Rust,
provado aqui contra a referência isolada.
"""
import base64
import json
import sqlite3
from contextlib import closing
import time

import pytest

from app import oauth_codex as o


def _jwt(account="acc-1", plano="plus", exp=4102444800):
    corpo = base64.urlsafe_b64encode(json.dumps({
        "exp": exp, "https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type": plano},
    }).encode()).rstrip(b"=").decode()
    return f"h.{corpo}.s"


def _assert_private_file(path):
    """No Windows, compara a DACL com a escrita atômica da base no mesmo diretório."""
    import hashlib
    import inspect
    import os
    import subprocess

    assert path.is_file()
    assert path.read_bytes(), "O arquivo persistido não pôde ser lido"
    if os.name != "nt":
        assert oct(path.stat().st_mode & 0o777) == "0o600"
        return
    baseline = path.with_name(path.name + ".acl-baseline")
    o._gravar_json(baseline, {"controle": "acl"})
    script = r"""
$ErrorActionPreference = 'Stop'
$paths = [Console]::In.ReadToEnd() | ConvertFrom-Json
$items = @(foreach ($path in $paths) {
    $acl = Get-Acl -LiteralPath $path
    $rules = @($acl.Access | ForEach-Object {
        @{ sid = $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
           rights = [long]$_.FileSystemRights; type = $_.AccessControlType.ToString() }
    })
    @{ owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
       dacl = $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
       rules = $rules }
})
@{ identity = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
   files = $items } | ConvertTo-Json -Compress -Depth 5
"""
    try:
        result = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
            input=json.dumps([str(baseline), str(path)]), text=True,
            capture_output=True, timeout=20, check=True,
        )
        proof = json.loads(result.stdout)
        base, candidate = proof["files"]
        assert base["dacl"] and candidate["dacl"], "Consulta nativa não retornou DACL"
        assert base["rules"] and candidate["rules"], "DACL vazia não prova proteção"
        assert candidate["owner"] == base["owner"], "A gravação mudou o proprietário"
        def rights(item, kind):
            masks = {}
            for rule in item["rules"]:
                if rule["type"] == kind:
                    masks[rule["sid"]] = masks.get(rule["sid"], 0) | (rule["rights"] & 0xffffffff)
            return masks
        base_allow, candidate_allow = rights(base, "Allow"), rights(candidate, "Allow")
        base_deny, candidate_deny = rights(base, "Deny"), rights(candidate, "Deny")
        assert all(mask & ~base_allow.get(sid, 0) == 0 for sid, mask in candidate_allow.items()), "A gravação ampliou direitos"
        assert all(mask & ~candidate_deny.get(sid, 0) == 0 for sid, mask in base_deny.items()), "A gravação retirou restrições"
        proof["writer_sha256"] = hashlib.sha256(inspect.getsource(o._gravar_json).encode()).hexdigest()
        proof["method"] = "DACL por SID/máscara; mesmo diretório, identidade e processo"
        print("Prova nativa de ACL: " + json.dumps(proof, sort_keys=True))
    finally:
        baseline.unlink()


def test_private_file_control_rejects_real_permission_expansion(tmp_path):
    import os
    import subprocess
    path = tmp_path / "controle.json"
    o._gravar_json(path, {"controle": "acl"})
    if os.name == "nt":
        script = r"""
$ErrorActionPreference = 'Stop'
$path = [Console]::In.ReadToEnd()
$acl = Get-Acl -LiteralPath $path
$sid = New-Object Security.Principal.SecurityIdentifier('S-1-1-0')
$rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, 'Read', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $path -AclObject $acl
"""
        subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                       input=str(path), text=True, capture_output=True, timeout=20, check=True)
        with pytest.raises(AssertionError, match="ampliou direitos"):
            _assert_private_file(path)
    else:
        path.chmod(0o644)
        with pytest.raises(AssertionError):
            _assert_private_file(path)


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("PI_CODING_AGENT_DIR", raising=False)
    monkeypatch.setattr(o, "cofre", lambda: tmp_path / ".hangar" / "auth" / "openai-codex.json")
    (tmp_path / ".codex").mkdir()
    (tmp_path / ".pi" / "agent").mkdir(parents=True)
    (tmp_path / ".omp" / "agent").mkdir(parents=True)
    con = sqlite3.connect(tmp_path / ".omp" / "agent" / "agent.db")
    con.execute("create table auth_credentials (id integer primary key autoincrement, provider text not null, "
                "credential_type text not null, data text not null, identity_key text)")
    con.commit(); con.close()
    return tmp_path


def test_propagar_grava_cofre_e_os_tres_stores(casa):
    t = o.Tokens.de_resposta({"access_token": _jwt(), "refresh_token": "r1", "id_token": "i1",
                              "expires_in": 10})
    o.salvar_cofre(t)
    _assert_private_file(o.cofre())
    resultado = o.propagar(None, casa)
    assert {k: v["ok"] for k, v in resultado.items()} == {"codex": True, "pi": True, "omp": True}
    codex = json.loads((casa / ".codex" / "auth.json").read_text())
    assert codex["auth_mode"] == "chatgpt" and codex["tokens"]["account_id"] == "acc-1"
    pi = json.loads((casa / ".pi" / "agent" / "auth.json").read_text())["openai-codex"]
    assert pi == {"type": "oauth", "access": _jwt(), "refresh": "r1", "expires": 4102444800000, "accountId": "acc-1"}
    con = sqlite3.connect(casa / ".omp" / "agent" / "agent.db")
    prov, tipo, dados, ident = con.execute("select provider, credential_type, data, identity_key from auth_credentials").fetchone()
    assert (prov, tipo, ident) == ("openai-codex", "oauth", "acc-1")
    assert json.loads(dados)["refresh"] == "r1" and "type" not in json.loads(dados)
    assert o._codex_tem_login(casa) and o._pi_tem_login(casa) and o._omp_tem_login(casa)


def test_store_com_login_e_mantido_e_cli_ausente_nao_e_erro(casa):
    (casa / ".codex" / "auth.json").write_text(json.dumps({"tokens": {"refresh_token": "dele"}}))
    (casa / ".pi" / "agent" / "auth.json").write_text(json.dumps({"openai-codex": {"type": "oauth", "refresh": "dele"}}))
    (casa / ".omp" / "agent" / "agent.db").unlink()
    t = o.Tokens(access=_jwt(), refresh="novo", id_token="", expires_ms=1, account_id="acc-1")
    r = o.propagar(t, casa)
    assert r["codex"] == {"ok": True, "motivo": "ja-logado"}
    assert r["pi"] == {"ok": True, "motivo": "ja-logado"}
    assert r["omp"] == {"ok": False, "motivo": "nao-instalado"}
    assert json.loads((casa / ".codex" / "auth.json").read_text())["tokens"]["refresh_token"] == "dele"


def test_importar_do_codex_alimenta_o_cofre(casa):
    (casa / ".codex" / "auth.json").write_text(json.dumps({
        "tokens": {"access_token": _jwt("acc-9", "pro"), "refresh_token": "r9", "account_id": "acc-9"}}))
    t = o.importar_do_codex(casa)
    assert t and t.account_id == "acc-9" and t.plano == "pro"
    assert o.ler_cofre().refresh == "r9"


@pytest.mark.parametrize("mode", ["rust", "pending"])
def test_managed_propagation_never_falls_back_to_python_codex_writer(casa, monkeypatch, mode):
    from types import SimpleNamespace
    from fastapi import HTTPException
    from app import account_bridge, runtime_coordinator
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode=mode))
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    tokens = o.Tokens(access=_jwt(), refresh="fixture", id_token="", expires_ms=1, account_id="acc-1")
    o.salvar_cofre(tokens)
    with pytest.raises(HTTPException) as issue:
        o.propagar(home=casa)
    assert issue.value.status_code == 503
    assert not (casa / ".codex" / "auth.json").exists(), "Python reintroduziu escritor Codex no modo gerenciado"


@pytest.fixture
def managed_device(tmp_path, request):
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from types import SimpleNamespace
    import threading
    from accounts_contract import PythonReference, RustCodex
    fixture = SimpleNamespace(grant=threading.Event(), requested=threading.Event(),
                              exchange=threading.Event(), release_exchange=threading.Event(),
                              hold_exchange=False, deny=False, malformed=False)
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            if self.path.endswith("/usercode"):
                assert json.loads(body) == {"client_id": "app_EMoamEEZ73f0CkXaXp7hrann"}
                status, result = 200, {"device_auth_id": "fixture-device", "user_code": "FIXTURE-CODE", "interval": "1"}
            elif self.path.endswith("/deviceauth/token"):
                assert json.loads(body) == {"device_auth_id": "fixture-device", "user_code": "FIXTURE-CODE"}
                fixture.requested.set()
                if fixture.deny:
                    status, result = 401, {"error": "access_denied", "secret": "fixture-secret-not-public"}
                elif not fixture.grant.is_set():
                    status, result = 403, {}
                else:
                    status, result = 200, {"authorization_code": "fixture-code", "code_verifier": "fixture-verifier"}
            elif self.path.endswith("/oauth/token"):
                import urllib.parse
                assert urllib.parse.parse_qs(body.decode()) == {
                    "grant_type": ["authorization_code"], "client_id": ["app_EMoamEEZ73f0CkXaXp7hrann"],
                    "code": ["fixture-code"], "code_verifier": ["fixture-verifier"],
                    "redirect_uri": ["https://auth.openai.com/deviceauth/callback"]}
                fixture.exchange.set()
                if fixture.hold_exchange:
                    assert fixture.release_exchange.wait(15), "troca retida não foi liberada"
                status, result = 200, {"access_token": _jwt(), "refresh_token": "fixture-refresh", "id_token": "fixture-id"}
                if fixture.malformed:
                    result.pop("refresh_token")
            else:
                raise AssertionError(self.path)
            data = json.dumps(result).encode()
            self.send_response(status)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            try:
                self.wfile.write(data)
            except (BrokenPipeError, ConnectionResetError):
                pass
    oauth = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=oauth.serve_forever, daemon=True)
    thread.start()
    reference = PythonReference(tmp_path / "home", block_handlers=getattr(request, "param", True))
    fixture.device_url = f"http://127.0.0.1:{oauth.server_port}"
    server = RustCodex(reference, device_url=fixture.device_url)
    fixture.reference, fixture.server = reference, server
    root = reference.root
    (root / ".pi" / "agent").mkdir(parents=True)
    (root / ".omp" / "agent").mkdir(parents=True)
    with closing(sqlite3.connect(root / ".omp" / "agent" / "agent.db")) as con:
        con.execute("create table auth_credentials (id integer primary key, provider text, credential_type text, data text, identity_key text)")
    try:
        yield fixture
    finally:
        fixture.release_exchange.set()
        fixture.server.close()
        reference.close()
        oauth.shutdown()
        oauth.server_close()
        thread.join(timeout=5)


def _wait_legacy(server, stage):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        response = server.request("GET", "/api/credenciais/codex/login")
        assert response.status_code == 200, response.json()
        if response.json()["etapa"] == stage:
            return response.json()
    raise AssertionError(f"device flow não chegou a {stage}")


def test_legacy_rust_writes_vault_codex_and_secondary_destinations(managed_device):
    fixture = managed_device
    fixture.grant.set()
    assert fixture.reference.request("POST", "/__contract__/codex-model-cache", {"account": "default"}).json()["cached"]
    start = fixture.server.request("POST", "/api/credenciais/codex/login")
    assert start.status_code == 200, start.json()
    assert start.json()["etapa"] == "aguardando"
    result = _wait_legacy(fixture.server, "concluido")
    root = fixture.reference.root
    assert not fixture.reference.request("GET", "/__contract__/codex-model-cache?account=default").json()["cached"]
    assert {name: item["ok"] for name, item in result["resultado"].items()} == {"codex": True, "pi": True, "omp": True}
    vault = json.loads((root / ".hangar/auth/openai-codex.json").read_text())
    assert vault == {"access": _jwt(), "refresh": "fixture-refresh", "id_token": "fixture-id",
                     "expires_ms": 4102444800000, "account_id": "acc-1", "plano": "plus"}
    auth = json.loads((root / ".codex/auth.json").read_text())
    assert auth["tokens"] == {"access_token": _jwt(), "refresh_token": "fixture-refresh", "id_token": "fixture-id", "account_id": "acc-1"}
    assert auth["auth_mode"] == "chatgpt" and auth["OPENAI_API_KEY"] is None
    pi = json.loads((root / ".pi/agent/auth.json").read_text())["openai-codex"]
    assert pi == {"type": "oauth", "access": _jwt(), "refresh": "fixture-refresh", "expires": 4102444800000, "accountId": "acc-1"}
    with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as con:
        provider, kind, data, identity = con.execute("select provider,credential_type,data,identity_key from auth_credentials").fetchone()
    assert (provider, kind, identity) == ("openai-codex", "oauth", "acc-1")
    assert json.loads(data) == {key: value for key, value in pi.items() if key != "type"}
    _assert_private_file(root / ".codex/auth.json")
    _assert_private_file(root / ".hangar/auth/openai-codex.json")
    state = fixture.server.request("GET", "/api/credenciais/codex")
    assert state.json() == {"cofre": True, "plano": "plus", "expira_em": 4102444800000, "codex": True, "pi": True, "omp": True}
    from accounts_contract import assert_rust_ownership
    assert_rust_ownership(fixture.reference.calls())
    public = json.dumps([start.json(), result, state.json(), fixture.reference.calls()])
    assert _jwt() not in public and "fixture-refresh" not in public and "fixture-id" not in public


def test_legacy_preserves_each_authenticated_destination_and_partial_result(managed_device):
    fixture = managed_device
    root = fixture.reference.root
    codex = b'{"tokens":{"refresh_token":"existing-codex"},"native":"kept"}'
    pi = b'{"openai-codex":{"type":"oauth","refresh":"existing-pi"}}'
    (root / ".codex/auth.json").write_bytes(codex)
    (root / ".pi/agent/auth.json").write_bytes(pi)
    (root / ".omp/agent/agent.db").unlink()
    fixture.grant.set()
    assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
    result = _wait_legacy(fixture.server, "concluido")["resultado"]
    assert result == {"codex": {"ok": True, "motivo": "ja-logado"},
                      "pi": {"ok": True, "motivo": "ja-logado"},
                      "omp": {"ok": False, "motivo": "nao-instalado"}}
    assert (root / ".codex/auth.json").read_bytes() == codex
    assert (root / ".pi/agent/auth.json").read_bytes() == pi


def test_legacy_cancel_waits_and_old_attempt_cannot_cancel_new_login(managed_device):
    fixture = managed_device
    fixture.hold_exchange = True
    fixture.grant.set()
    first = fixture.server.request("POST", "/api/credenciais/codex/login").json()
    assert fixture.exchange.wait(10), "a troca não começou"
    assert fixture.server.request("DELETE", "/api/credenciais/codex/login?attempt_id=" + first["attempt_id"]).json() == {"etapa": "idle"}
    fixture.release_exchange.set()
    fixture.grant.clear()
    second = fixture.server.request("POST", "/api/credenciais/codex/login").json()
    assert second["attempt_id"] != first["attempt_id"]
    stale = fixture.server.request("DELETE", "/api/credenciais/codex/login?attempt_id=" + first["attempt_id"])
    assert stale.status_code == 409 and stale.json()["detail"]["code"] == "codex_login_attempt_mismatch"
    assert fixture.server.request("GET", "/api/credenciais/codex/login").json()["attempt_id"] == second["attempt_id"]
    assert not (fixture.reference.root / ".hangar/auth/openai-codex.json").exists()
    assert not (fixture.reference.root / ".codex/auth.json").exists()
    assert fixture.server.request("DELETE", "/api/credenciais/codex/login").json() == {"etapa": "idle"}


@pytest.mark.parametrize("malformed,deny,expected", [(True, False, "device_tokens_invalid"), (False, True, "device_authorization_failed")])
def test_legacy_failure_never_publishes_tokens_or_writes_auth(managed_device, malformed, deny, expected):
    fixture = managed_device
    fixture.malformed, fixture.deny = malformed, deny
    fixture.grant.set()
    assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
    result = _wait_legacy(fixture.server, "falhou")
    assert result["erro"] == expected
    assert "fixture-secret-not-public" not in json.dumps(result)
    assert not (fixture.reference.root / ".codex/auth.json").exists()
    assert not (fixture.reference.root / ".hangar/auth/openai-codex.json").exists()


def test_legacy_incompatible_storage_refuses_before_authorization(managed_device):
    fixture = managed_device
    (fixture.reference.root / ".codex/config.toml").write_text('cli_auth_credentials_store="keyring"\n')
    response = fixture.server.request("POST", "/api/credenciais/codex/login")
    assert response.status_code == 409 and response.json()["detail"]["code"] == "codex_account_auth_storage_invalid"
    assert not fixture.requested.is_set()


def test_retired_secondary_routes_are_unavailable(managed_device):
    from urllib.request import Request
    from urllib.error import HTTPError
    fixture = managed_device
    _seed_vault(fixture.reference.root)
    for path in ("device-propagate", "device-propagate/ack", "device-propagate/close", "device-propagate-state"):
        request = Request(fixture.reference.base_url + "/internal/accounts/" + path, data=b"{}", method="POST",
                          headers={"x-hangar-internal": "contract-internal",
                                   "x-hangar-runtime-instance": "contract-instance",
                                   "x-hangar-operation-id": "fixture-operation"})
        try:
            response = fixture.reference.opener.open(request, timeout=5)
        except HTTPError as error:
            response = error
        with response:
            assert response.status == 404, f"rota secundária aposentada ainda atende: {path}"
    assert not (fixture.reference.root / ".pi/agent/auth.json").exists()
    with closing(sqlite3.connect(fixture.reference.root / ".omp/agent/agent.db")) as connection:
        assert connection.execute("select count(*) from auth_credentials").fetchone()[0] == 0


def test_legacy_restart_discards_attempt_and_releases_default_account_guard(managed_device):
    from accounts_contract import RustCodex
    fixture = managed_device
    first = fixture.server.request("POST", "/api/credenciais/codex/login").json()
    fixture.server.close()
    fixture.server = RustCodex(fixture.reference)
    assert fixture.server.request("GET", "/api/credenciais/codex/login").json() == {"etapa": "idle"}
    stale = fixture.server.request("DELETE", "/api/credenciais/codex/login?attempt_id=" + first["attempt_id"])
    assert stale.status_code == 200 and stale.json() == {"etapa": "idle"}
    assert not (fixture.reference.root / ".codex/auth.json").exists()
    # A leitura após reinício pode tomar a mesma conta sem auxiliar de login abandonado.
    assert fixture.server.request("GET", "/api/codex-contas").status_code == 200


def test_legacy_uncertain_usage_refuses_before_oauth_request(managed_device):
    fixture = managed_device
    fixture.reference.request("POST", "/__contract__/runtime-instance", {"instance": "obsolete"})
    response = fixture.server.request("POST", "/api/credenciais/codex/login")
    assert response.status_code == 409 and response.json()["detail"]["code"] == "account_usage_unknown"
    assert fixture.server.request("GET", "/api/credenciais/codex/login").json() == {"etapa": "idle"}
    assert not fixture.requested.is_set()


def test_health_import_and_propagation_are_owned_by_rust(managed_device):
    fixture = managed_device
    root = fixture.reference.root
    original = b'{"tokens":{"access_token":"' + _jwt().encode() + b'","refresh_token":"fixture-existing","id_token":"fixture-id"}}'
    (root / ".codex/auth.json").write_bytes(original)
    fixture.reference.request("POST", "/__contract__/claude-owner", {
        "address": fixture.server.request("GET", "/__hangar_server/health").json()["terminal_address"], "mode": "rust"})
    response = fixture.reference.request("POST", "/__contract__/device-repair", {"action": "oauth"})
    assert response.status_code == 200, response.json()
    assert json.loads((root / ".hangar/auth/openai-codex.json").read_text())["refresh"] == "fixture-existing"
    assert json.loads((root / ".pi/agent/auth.json").read_text())["openai-codex"]["refresh"] == "fixture-existing"
    with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as con:
        assert json.loads(con.execute("select data from auth_credentials").fetchone()[0])["refresh"] == "fixture-existing"
    assert (root / ".codex/auth.json").read_bytes() == original
    fixture.reference.request("POST", "/__contract__/claude-owner", {"address": "127.0.0.1:1", "mode": "pending"})
    pending = fixture.reference.request("POST", "/__contract__/device-repair", {"action": "oauth"})
    assert pending.status_code == 503
    assert (root / ".codex/auth.json").read_bytes() == original


def test_legacy_no_cli_is_required_and_login_does_not_create_native_helper(managed_device):
    fixture = managed_device
    fixture.grant.set()
    assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
    assert _wait_legacy(fixture.server, "concluido")["resultado"]["codex"]["ok"]
    assert not (fixture.reference.root / ".codex/native-pid.json").exists()


def _native_only(fixture):
    assert fixture.reference.request("POST", "/__contract__/secondary-native-only", {}).status_code == 200


def _seed_vault(root, refresh="fixture-refresh"):
    vault = root / ".hangar/auth/openai-codex.json"
    vault.parent.mkdir(parents=True, exist_ok=True)
    vault.write_text(json.dumps({"access": _jwt(), "refresh": refresh, "id_token": "fixture-id",
                                 "expires_ms": 4102444800000, "account_id": "acc-1", "plano": "plus"}))
    return vault


def _private(server, action, *, timeout=30):
    from urllib.request import Request
    address = server.request("GET", "/__hangar_server/health").json()["terminal_address"]
    request = Request("http://" + address + "/__hangar_server/accounts",
                      data=json.dumps({"device_action": action}).encode(), method="POST",
                      headers={"Content-Type": "application/json", "x-hangar-internal": "contract-internal"})
    with server.opener.open(request, timeout=timeout) as response:
        return json.loads(response.read())


def _omp_rows(database):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as connection:
        return connection.execute(
            "select id, provider, credential_type, data, identity_key from auth_credentials order by id").fetchall()


def test_secondary_native_writes_and_state_without_python_writer(managed_device):
    fixture = managed_device
    assert fixture.reference.request(
        "POST", "/__contract__/secondary-native-only", {}
    ).status_code == 200
    fixture.grant.set()
    assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
    result = _wait_legacy(fixture.server, "concluido")["resultado"]
    assert {name: item["ok"] for name, item in result.items()} == {
        "codex": True, "pi": True, "omp": True,
    }
    root = fixture.reference.root
    assert json.loads((root / ".pi/agent/auth.json").read_text())["openai-codex"]["refresh"] == "fixture-refresh"
    with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as connection:
        rows = connection.execute(
            "select provider, credential_type, data, identity_key from auth_credentials"
        ).fetchall()
    assert len(rows) == 1
    assert rows[0][:2] == ("openai-codex", "oauth")
    assert json.loads(rows[0][2])["refresh"] == "fixture-refresh"
    assert rows[0][3] == "acc-1"
    state = fixture.server.request("GET", "/api/credenciais/codex")
    assert state.status_code == 200
    assert state.json() == {"cofre": True, "plano": "plus", "expira_em": 4102444800000,
                            "codex": True, "pi": True, "omp": True}
    from accounts_contract import assert_rust_ownership
    assert_rust_ownership(fixture.reference.calls())
    public = json.dumps([result, state.json(), fixture.reference.calls()])
    assert _jwt() not in public and "fixture-refresh" not in public and "fixture-id" not in public


@pytest.mark.parametrize("previous_oauth", [True, False], ids=["varias-oauth", "somente-api-key"])
def test_secondary_native_preserves_previous_credentials(managed_device, previous_oauth):
    from accounts_contract import assert_rust_ownership
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    database = root / ".omp/agent/agent.db"
    rows = [(3, "openai-codex", "api_key", '{"key":"synthetic-api-key"}', None),
            (5, "anthropic", "oauth", '{"refresh":"synthetic-other"}', "other")]
    if previous_oauth:
        rows += [(7, "openai-codex", "oauth", '{"refresh": "synthetic-first"}', "first"),
                 (9, "openai-codex", "oauth", '{"refresh": "synthetic-second"}', None)]
    with closing(sqlite3.connect(database)) as connection:
        connection.executemany("insert into auth_credentials values (?, ?, ?, ?, ?)", rows)
        connection.commit()
    before = database.read_bytes()
    pi = root / ".pi/agent/auth.json"
    pi.write_text(json.dumps({"zeta": {"type": "api_key", "key": "synthetic-zeta"}, "alpha": {"keep": [1, 2.5]}},
                             indent=2))
    _seed_vault(root)
    result = _private(fixture.server, "propagate")
    assert set(result) == {"codex", "pi", "omp"}
    assert result["pi"] == {"ok": True, "motivo": str(pi)}
    written = json.loads(pi.read_text())
    assert list(written) == ["zeta", "alpha", "openai-codex"]
    assert written["zeta"] == {"type": "api_key", "key": "synthetic-zeta"}
    assert written["alpha"] == {"keep": [1, 2.5]}
    assert written["openai-codex"] == {"type": "oauth", "access": _jwt(), "refresh": "fixture-refresh",
                                       "expires": 4102444800000, "accountId": "acc-1"}
    after = _omp_rows(database)
    if previous_oauth:
        assert result["omp"] == {"ok": True, "motivo": "ja-logado"}
        assert after == rows, "OAuth/api_key anteriores foram alterados ou duplicados"
        assert database.read_bytes() == before, "O banco com OAuth anterior foi reescrito"
    else:
        assert result["omp"] == {"ok": True, "motivo": str(database)}
        assert [row for row in after if row[0] in (3, 5)] == rows, "A linha api_key foi alterada"
        inserted = [row for row in after if row[0] not in (3, 5)]
        assert len(inserted) == 1 and inserted[0][1:3] == ("openai-codex", "oauth") and inserted[0][4] == "acc-1"
        assert json.loads(inserted[0][3]) == {"access": _jwt(), "refresh": "fixture-refresh",
                                              "expires": 4102444800000, "accountId": "acc-1"}
    assert_rust_ownership(fixture.reference.calls())
    public = json.dumps([result, fixture.reference.calls()])
    assert "fixture-refresh" not in public and "synthetic-api-key" not in public


@pytest.mark.parametrize("linked", ["arquivo", "diretorio"])
def test_secondary_native_accepts_linked_pi_destinations(managed_device, linked):
    import os
    import shutil
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    agent = root / ".pi/agent"
    original = b'{"keep": {"k": 1}}'
    if linked == "arquivo":
        target = root / "shared-auth.json"
        target.write_bytes(original)
        os.symlink(target, agent / "auth.json")
    else:
        shutil.rmtree(agent)
        target = root / "real-agent"
        target.mkdir()
        (target / "auth.json").write_bytes(original)
        os.symlink(target, agent, target_is_directory=True)
    _seed_vault(root)
    result = _private(fixture.server, "propagate")
    assert result["pi"] == {"ok": True, "motivo": str(agent / "auth.json")}, "destino Pi vinculado passou a falhar"
    written = json.loads((agent / "auth.json").read_text())
    assert written["keep"] == {"k": 1} and written["openai-codex"]["refresh"] == "fixture-refresh"
    if linked == "arquivo":
        assert not (agent / "auth.json").is_symlink(), "o link de arquivo não foi substituído"
        assert target.read_bytes() == original, "a escrita atravessou o link de arquivo"
    else:
        assert agent.is_symlink(), "o diretório Pi deixou de ser vinculado"


def test_secondary_native_state_reads_without_writing(managed_device):
    from accounts_contract import assert_rust_ownership
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    database = root / ".omp/agent/agent.db"
    pi = root / ".pi/agent/auth.json"
    _seed_vault(root)
    expected = {"cofre": True, "plano": "plus", "expira_em": 4102444800000, "codex": False}
    def state():
        response = fixture.server.request("GET", "/api/credenciais/codex")
        assert response.status_code == 200, response.json()
        return response.json()
    before = database.read_bytes()
    assert state() == {**expected, "pi": False, "omp": False}
    assert not pi.exists(), "A leitura de estado criou o arquivo Pi"
    assert database.read_bytes() == before
    pi.write_text(json.dumps({"openai-codex": {"type": "oauth", "refresh": ""}}))
    with closing(sqlite3.connect(database)) as connection:
        connection.execute("insert into auth_credentials values (1, 'openai-codex', 'api_key', '{}', NULL)")
        connection.commit()
    pi_bytes, before = pi.read_bytes(), database.read_bytes()
    assert state() == {**expected, "pi": False, "omp": False}
    pi.write_text(json.dumps({"openai-codex": {"type": "oauth", "refresh": "synthetic-pi"}}))
    with closing(sqlite3.connect(database)) as connection:
        connection.execute("insert into auth_credentials values (2, 'openai-codex', 'oauth', '{}', NULL)")
        connection.commit()
    pi_bytes, before = pi.read_bytes(), database.read_bytes()
    assert state() == {**expected, "pi": True, "omp": True}
    assert pi.read_bytes() == pi_bytes and database.read_bytes() == before, "A leitura de estado escreveu"
    assert_rust_ownership(fixture.reference.calls())


def test_secondary_native_profile_is_strict_and_resolved_by_rust(managed_device):
    from accounts_contract import RustCodex
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    _seed_vault(root)
    profile = root / ".omp/profiles/work/agent/agent.db"
    profile.parent.mkdir(parents=True)
    with closing(sqlite3.connect(profile)) as connection:
        connection.execute("create table auth_credentials (id integer primary key, provider text, credential_type text, data text, identity_key text)")
        connection.execute("insert into auth_credentials values (1, 'openai-codex', 'oauth', '{}', 'work')")
        connection.commit()
    profile_bytes = profile.read_bytes()
    fixture.server.close()
    fixture.server = RustCodex(fixture.reference, device_url=fixture.device_url,
                               extra_environment={"OMP_PROFILE": " work "})
    state = fixture.server.request("GET", "/api/credenciais/codex")
    assert state.status_code == 200 and state.json()["omp"] is True, "O perfil omp não foi resolvido pelo Rust"
    fixture.server.close()
    fixture.server = RustCodex(fixture.reference, device_url=fixture.device_url,
                               extra_environment={"OMP_PROFILE": "../fora", "PI_PROFILE": "work"})
    state = fixture.server.request("GET", "/api/credenciais/codex")
    assert state.status_code == 503, "perfil inválido foi apresentado como destino deslogado"
    assert state.json()["detail"]["code"] == "device_bridge_unavailable"
    result = _private(fixture.server, "propagate")
    assert result["pi"] == {"ok": True, "motivo": str(root / ".pi/agent/auth.json")}
    assert result["omp"] == {"ok": False, "motivo": "armazenamento-indisponivel"}
    assert _omp_rows(root / ".omp/agent/agent.db") == [], "perfil inválido caiu na raiz padrão"
    assert profile.read_bytes() == profile_bytes


def test_secondary_native_never_serializes_database_error_content(managed_device):
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    _seed_vault(root, refresh="fixture-secret-not-public")
    with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as connection:
        connection.execute("CREATE TRIGGER deny_insert BEFORE INSERT ON auth_credentials "
                           "BEGIN SELECT RAISE(FAIL, 'fixture-secret-not-public'); END")
    result = _private(fixture.server, "propagate")
    assert result["omp"] == {"ok": False, "motivo": "sqlite-indisponivel"}
    assert "fixture-secret-not-public" not in json.dumps(result), "o erro SQLite transportou conteúdo privado"
    assert result["pi"]["ok"], "falha no omp descartou sucesso independente do Pi"


def _wait_secondary_io(fixture, pi):
    # O Pi é gravado antes do omp: Pi escrito com o omp ainda vazio é o writer parado no SQLite.
    deadline = time.monotonic() + 20
    while "openai-codex" not in json.loads(pi.read_text() or "{}"):
        assert time.monotonic() < deadline, "O writer nativo não chegou ao I/O"
        fixture.server.request("GET", "/api/credenciais/codex/login")


@pytest.mark.parametrize("start", [True, False], ids=["device-start", "consumer-propagate"])
def test_native_writer_keeps_guard_and_shutdown_while_sqlite_is_busy(managed_device, start):
    import subprocess
    import threading
    from accounts_contract import assert_rust_ownership
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    pi = root / ".pi/agent/auth.json"
    pi.write_text("{}")
    database = root / ".omp/agent/agent.db"
    lock = sqlite3.connect(database, isolation_level=None)
    lock.execute("BEGIN EXCLUSIVE")
    cancelled = []
    cancel = None
    try:
        if start:
            fixture.grant.set()
            attempt = fixture.server.request("POST", "/api/credenciais/codex/login")
            assert attempt.status_code == 200, attempt.json()
        else:
            _seed_vault(root)
            # O chamador abandona a espera HTTP; a escrita registrada continua dona da conta.
            with pytest.raises(OSError):
                _private(fixture.server, "propagate", timeout=0.5)
        _wait_secondary_io(fixture, pi)
        assert fixture.server.request("POST", "/api/codex-contas/default/login").status_code == 409
        assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 409
        if start:
            def cancel_attempt():
                try:
                    cancelled.append(fixture.server.request(
                        "DELETE", "/api/credenciais/codex/login?attempt_id=" + attempt.json()["attempt_id"]))
                except OSError:
                    # O shutdown pedido em seguida pode fechar a conexão do cancelamento em voo.
                    cancelled.append(None)
            cancel = threading.Thread(target=cancel_attempt)
            cancel.start()
            cancel.join(timeout=0.5)
            assert cancel.is_alive(), "O cancelamento não esperou o término real da escrita"
        fixture.server.process.stdin.close()
        with pytest.raises(subprocess.TimeoutExpired):
            fixture.server.process.wait(timeout=1)
        assert lock.execute("select count(*) from auth_credentials").fetchone()[0] == 0
    finally:
        lock.execute("ROLLBACK")
        lock.close()
    fixture.server.process.wait(timeout=15)
    if cancel is not None:
        cancel.join(timeout=5)
        assert not cancel.is_alive()
        assert cancelled[0] is None or cancelled[0].json() == {"etapa": "idle"}
    assert json.loads(pi.read_text())["openai-codex"]["refresh"] == "fixture-refresh"
    assert json.loads((root / ".codex/auth.json").read_text())["tokens"]["refresh_token"] == "fixture-refresh"
    assert len(_omp_rows(database)) == 1, "O writer não terminou o omp depois de liberado"
    assert_rust_ownership(fixture.reference.calls())


@pytest.mark.parametrize("start", [True, False], ids=["device-start", "consumer-propagate"])
def test_native_writer_reports_sanitized_sqlite_busy_expiry(managed_device, start):
    fixture = managed_device
    _native_only(fixture)
    root = fixture.reference.root
    pi = root / ".pi/agent/auth.json"
    pi.write_text("{}")
    database = root / ".omp/agent/agent.db"
    lock = sqlite3.connect(database, isolation_level=None)
    lock.execute("BEGIN EXCLUSIVE")
    try:
        if start:
            fixture.grant.set()
            assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
            result = _wait_legacy(fixture.server, "concluido")["resultado"]
        else:
            _seed_vault(root)
            result = _private(fixture.server, "propagate")
        assert result["omp"] == {"ok": False, "motivo": "sqlite-indisponivel"}
        assert result["pi"] == {"ok": True, "motivo": str(pi)}
        # O término por busy timeout devolve a conta: outro login começa normalmente.
        fixture.grant.clear()
        again = fixture.server.request("POST", "/api/credenciais/codex/login")
        assert again.status_code == 200, again.json()
        assert fixture.server.request("DELETE", "/api/credenciais/codex/login").json() == {"etapa": "idle"}
    finally:
        lock.execute("ROLLBACK")
        lock.close()
    assert _omp_rows(database) == []
