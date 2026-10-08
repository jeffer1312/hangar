"""Login OAuth do ChatGPT feito pelo app e espalhado pros CLIs (app/oauth_codex.py).

O que trava: o fluxo de dispositivo termina com o cofre gravado (0600) e os três stores escritos
no formato de cada um; store que já tem login é mantido; CLI ausente é `nao-instalado`, não erro.
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
    o._tentativa = None
    return tmp_path


def test_fluxo_de_dispositivo_grava_cofre_e_os_tres_stores(casa, monkeypatch):
    respostas = iter([
        (200, {"device_auth_id": "d1", "user_code": "ABCD-1234", "interval": "0"}),
        (403, {}),
        (200, {"authorization_code": "c", "code_verifier": "v"}),
        (200, {"access_token": _jwt(), "refresh_token": "r1", "id_token": "i1", "expires_in": 10}),
    ])
    monkeypatch.setattr(o, "_http", lambda url, corpo, form: next(respostas))
    passo = o.iniciar(casa)
    assert passo["etapa"] == "aguardando" and passo["user_code"] == "ABCD-1234"
    assert passo["url"] == o.VERIFICATION_URL
    for _ in range(100):
        if o.passo()["etapa"] != "aguardando":
            break
        time.sleep(0.05)
    p = o.passo()
    assert p["etapa"] == "concluido", p
    _assert_private_file(o.cofre())
    assert {k: v["ok"] for k, v in p["resultado"].items()} == {"codex": True, "pi": True, "omp": True}
    codex = json.loads((casa / ".codex" / "auth.json").read_text())
    assert codex["auth_mode"] == "chatgpt" and codex["tokens"]["account_id"] == "acc-1"
    pi = json.loads((casa / ".pi" / "agent" / "auth.json").read_text())["openai-codex"]
    assert pi == {"type": "oauth", "access": _jwt(), "refresh": "r1", "expires": 4102444800000, "accountId": "acc-1"}
    con = sqlite3.connect(casa / ".omp" / "agent" / "agent.db")
    prov, tipo, dados, ident = con.execute("select provider, credential_type, data, identity_key from auth_credentials").fetchone()
    assert (prov, tipo, ident) == ("openai-codex", "oauth", "acc-1")
    assert json.loads(dados)["refresh"] == "r1" and "type" not in json.loads(dados)
    assert o.estado(casa) == {"cofre": True, "plano": "plus", "expira_em": 4102444800000,
                              "codex": True, "pi": True, "omp": True}


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
    server = RustCodex(reference, device_url=f"http://127.0.0.1:{oauth.server_port}")
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


def test_legacy_private_secondary_hook_rejects_secret_payload_and_stale_instance(managed_device):
    from urllib.request import Request
    from urllib.error import HTTPError
    fixture = managed_device
    vault = fixture.reference.root / ".hangar/auth/openai-codex.json"
    vault.parent.mkdir(parents=True)
    vault.write_text(json.dumps({"access": _jwt(), "refresh": "fixture-refresh", "id_token": "",
                                "expires_ms": 1, "account_id": "acc-1", "plano": "plus"}))
    def call(body, *, secret="contract-internal", instance="contract-instance"):
        request = Request(fixture.reference.base_url + "/internal/accounts/device-propagate",
                          data=json.dumps(body).encode(), method="POST", headers={
                              "x-hangar-internal": secret, "x-hangar-runtime-instance": instance,
                              "x-hangar-operation-id": "fixture-operation"})
        try:
            response = fixture.reference.opener.open(request, timeout=5)
        except HTTPError as error:
            response = error
        with response:
            return response.status, json.loads(response.read())
    assert call({}, secret="wrong")[0] == 404
    assert call({}, instance="old")[0] == 404
    assert call({"token": "synthetic"})[0] == 400
    assert not (fixture.reference.root / ".pi/agent/auth.json").exists()
    status, result = call({})
    deadline = time.monotonic() + 5
    while result.get("status") == "pending":
        assert time.monotonic() < deadline, "A operação privada não terminou"
        status, result = call({})
    assert result["instance"] == "contract-instance" and result["operation_id"] == "fixture-operation"
    assert result["status"] == "completed"
    result = result["result"]
    assert status == 200 and result["pi"]["ok"] and result["omp"]["ok"]
    assert set(result) == {"pi", "omp"}
    assert not (fixture.reference.root / ".codex/auth.json").exists(), "o gancho privado escreveu Codex"


@pytest.mark.parametrize("managed_device", [False], indirect=True)
def test_legacy_python_consumers_delegate_and_pending_never_falls_back(managed_device):
    fixture = managed_device
    reference = fixture.reference
    reference.request("POST", "/__contract__/claude-owner", {
        "address": fixture.server.request("GET", "/__hangar_server/health").json()["terminal_address"], "mode": "rust"})
    attempt = reference.request("POST", "/api/credenciais/codex/login")
    assert attempt.status_code == 200, attempt.json()
    assert attempt.json()["etapa"] == "aguardando"
    assert fixture.server.request("GET", "/api/credenciais/codex/login").json() == attempt.json()
    assert reference.request("DELETE", "/api/credenciais/codex/login").json() == {"etapa": "idle"}
    reference.request("POST", "/__contract__/claude-owner", {"address": "127.0.0.1:1", "mode": "pending"})
    for method, path in [("POST", "/api/credenciais/codex/login"), ("GET", "/api/credenciais/codex/login"),
                         ("DELETE", "/api/credenciais/codex/login"), ("GET", "/api/credenciais/codex")]:
        response = reference.request(method, path)
        assert response.status_code == 503
        assert response.json()["detail"]["code"] == "account_device_bridge_unavailable"
    assert not (reference.root / ".codex/auth.json").exists()
    assert not (reference.root / ".hangar/auth/openai-codex.json").exists()


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


def test_legacy_state_reports_private_bridge_failure_instead_of_logged_out(managed_device):
    fixture = managed_device
    fixture.reference.request("POST", "/__contract__/runtime-instance", {"instance": "obsolete"})
    response = fixture.server.request("GET", "/api/credenciais/codex")
    assert response.status_code == 503, "falha da ponte foi apresentada como destinos deslogados"
    assert response.json()["detail"]["code"] == "device_bridge_unavailable"


def test_secondary_propagation_never_serializes_database_error_content(casa, monkeypatch):
    monkeypatch.setenv("USERPROFILE", str(casa))
    monkeypatch.delenv("PI_CODING_AGENT_DIR", raising=False)
    monkeypatch.setattr(__import__("pathlib").Path, "home", lambda: casa)
    tokens = o.Tokens(access=_jwt(), refresh="fixture-secret-not-public", id_token="", expires_ms=1, account_id="fixture")
    o.salvar_cofre(tokens)
    with closing(sqlite3.connect(casa / ".omp/agent/agent.db")) as con:
        con.execute("CREATE TRIGGER deny_insert BEFORE INSERT ON auth_credentials BEGIN SELECT RAISE(FAIL, 'fixture-secret-not-public'); END")
    assert o._omp_db(None) == casa / ".omp/agent/agent.db"
    result = o.propagate_secondary()
    assert not result["omp"]["ok"]
    assert "fixture-secret-not-public" not in json.dumps(result), "o erro SQLite transportou conteúdo privado"
    assert result["pi"]["ok"], "falha no omp descartou sucesso independente do Pi"


@pytest.mark.asyncio
async def test_secondary_job_registration_survives_response_loss_and_closing(casa, monkeypatch):
    import asyncio
    import threading
    from pathlib import Path
    from types import SimpleNamespace
    from fastapi import FastAPI
    from app import internal_api, runtime_coordinator

    monkeypatch.setattr(Path, "home", classmethod(lambda cls: casa))
    monkeypatch.setenv("CODEX_HOME", str(casa / ".codex"))
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="job-instance", mode="rust"))
    monkeypatch.setattr(internal_api, "_secret", "job-secret")
    o.salvar_cofre(o.Tokens(access=_jwt(), refresh="fixture", id_token="", expires_ms=1, account_id="acc-1"))
    pi = casa / ".pi/agent/auth.json"
    pi.write_text("{}")
    entered, release = threading.Event(), threading.Event()
    reads = []
    read_text = Path.read_text
    def retained_read(path, *args, **kwargs):
        if path == pi and not release.is_set():
            reads.append(path)
            entered.set()
            assert release.wait(30), "O escritor privado não foi liberado"
        return read_text(path, *args, **kwargs)
    monkeypatch.setattr(Path, "read_text", retained_read)
    app = FastAPI()
    app.include_router(internal_api.router)
    class LocalClient:
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            return False
        async def post(self, path, *, content="{}", headers=None, **options):
            body = (json.dumps(options["json"]) if "json" in options else content).encode()
            scope = {"type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1",
                     "method": "POST", "scheme": "http", "path": path, "raw_path": path.encode(),
                     "root_path": "", "query_string": b"", "server": ("fixture", 80), "client": ("127.0.0.1", 1),
                     "headers": [(key.encode(), value.encode()) for key, value in headers.items()]}
            sent = []
            received = False
            async def receive():
                nonlocal received
                if not received:
                    received = True
                    return {"type": "http.request", "body": body, "more_body": False}
                await asyncio.Future()
            async def send(message):
                sent.append(message)
            await app(scope, receive, send)
            status = next(message["status"] for message in sent if message["type"] == "http.response.start")
            response = b"".join(message.get("body", b"") for message in sent if message["type"] == "http.response.body")
            return SimpleNamespace(status_code=status, json=lambda: json.loads(response))
    headers = {"x-hangar-internal": "job-secret", "x-hangar-runtime-instance": "job-instance",
               "x-hangar-operation-id": "fixture-operation"}
    close = None
    async with LocalClient() as client:
        try:
            try:
                response = await asyncio.wait_for(client.post("/internal/accounts/device-propagate", content="{}", headers=headers), 1)
            except TimeoutError:
                response = None
            await asyncio.to_thread(entered.wait, 5)
            assert entered.is_set(), "A montagem não alcançou a leitura real Pi"
            assert response is not None, "O aceite privado depende do término da thread"
            assert response.json() == {"instance": "job-instance", "operation_id": "fixture-operation", "status": "pending"}
            # A resposta anterior pode se perder: o mesmo identificador conserva o mesmo writer.
            retry = await client.post("/internal/accounts/device-propagate", content="{}", headers=headers)
            assert retry.json() == response.json()
            assert len(reads) == 1, "Retentativa criou outro escritor"
            facts = await client.post("/internal/accounts/facts", headers=headers,
                                      json={"keys": [{"provider": "codex", "canonical_home": str((casa / ".codex").resolve())}]})
            assert __import__("os").getpid() in facts.json()[0]["facts"]["pids"], "Os fatos esqueceram o escritor privado vivo"
            wrong = await client.post("/internal/accounts/device-propagate", content="{}", headers={**headers, "x-hangar-runtime-instance": "old"})
            assert wrong.status_code == 404
            close = asyncio.create_task(client.post("/internal/accounts/device-propagate/close", content="{}", headers=headers))
            await asyncio.to_thread(entered.wait, 1)
            assert not close.done()
            # A consulta existente funciona em closing; ingresso novo deve ser recusado.
            fresh = await client.post("/internal/accounts/device-propagate", content="{}", headers={**headers, "x-hangar-operation-id": "new-operation"})
            assert fresh.status_code == 503, "Closing não fechou o ingresso"
            assert not close.done(), "Closing terminou com writer retido"
            assert (await client.post("/internal/accounts/device-propagate", content="{}", headers=headers)).json()["status"] == "pending"
            with closing(sqlite3.connect(casa / ".omp/agent/agent.db")) as connection:
                assert connection.execute("select count(*) from auth_credentials").fetchone()[0] == 0
        finally:
            release.set()
            if close is not None:
                assert (await asyncio.wait_for(close, 5)).json() == {"instance": "job-instance", "status": "closed"}
        final = (await client.post("/internal/accounts/device-propagate", content="{}", headers=headers)).json()
        assert final["instance"] == "job-instance" and final["operation_id"] == "fixture-operation"
        assert final["status"] == "completed" and final["result"]["pi"]["ok"] and final["result"]["omp"]["ok"]
        assert len(reads) == 1
        facts = await client.post("/internal/accounts/facts", headers=headers,
                                  json={"keys": [{"provider": "codex", "canonical_home": str((casa / ".codex").resolve())}]})
        assert __import__("os").getpid() not in facts.json()[0]["facts"]["pids"]
        with closing(sqlite3.connect(casa / ".omp/agent/agent.db")) as connection:
            assert connection.execute("select count(*) from auth_credentials").fetchone()[0] == 1
        assert json.loads(pi.read_text())["openai-codex"]["refresh"] == "fixture"


@pytest.mark.parametrize("start", [True, False], ids=["device-start", "consumer-propagate"])
def test_real_shutdown_drains_secondary_writer_after_transport_loss(managed_device, start):
    import subprocess
    from concurrent.futures import ThreadPoolExecutor
    fixture = managed_device
    root = fixture.reference.root
    (root / ".pi/agent/auth.json").write_text("{}")
    io = fixture.reference.block("secondary_io")
    loss = fixture.reference.block("secondary_response_loss")
    future = None
    with ThreadPoolExecutor(max_workers=1) as executor:
        try:
            if start:
                fixture.grant.set()
                assert fixture.server.request("POST", "/api/credenciais/codex/login").status_code == 200
            else:
                vault = root / ".hangar/auth/openai-codex.json"
                vault.parent.mkdir(parents=True)
                vault.write_text(json.dumps({"access": _jwt(), "refresh": "fixture-refresh", "id_token": "fixture-id",
                                            "expires_ms": 4102444800000, "account_id": "acc-1", "plano": "plus"}))
                fixture.reference.request("POST", "/__contract__/claude-owner", {
                    "address": fixture.server.request("GET", "/__hangar_server/health").json()["terminal_address"], "mode": "rust"})
                future = executor.submit(fixture.reference.request, "POST", "/__contract__/device-repair", {"action": "oauth"})
            assert io.entered.wait(), "O fluxo não chegou ao I/O real do escritor Pi"
            assert fixture.server.request("POST", "/api/codex-contas/default/login").status_code == 409
            fixture.server.process.stdin.close()
            with pytest.raises(subprocess.TimeoutExpired):
                fixture.server.process.wait(timeout=12)
            with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as connection:
                assert connection.execute("select count(*) from auth_credentials").fetchone()[0] == 0
            assert loss.entered.wait(), "A montagem não perdeu a resposta após aceitar o trabalho"
        finally:
            io.release.set()
            fixture.server.process.wait(timeout=10)
            if future is not None:
                try:
                    future.result(timeout=5)
                except OSError:
                    # A porta privada fecha no shutdown; esse erro não confirma término do writer.
                    pass
        assert json.loads((root / ".pi/agent/auth.json").read_text())["openai-codex"]["refresh"] == "fixture-refresh"
        assert json.loads((root / ".codex/auth.json").read_text())["tokens"]["refresh_token"] == "fixture-refresh"
        with closing(sqlite3.connect(root / ".omp/agent/agent.db")) as connection:
            assert connection.execute("select count(*) from auth_credentials").fetchone()[0] == 1
        calls = [row for row in fixture.reference.calls() if row["operation"] == "bridge.propagate_device_login" and row.get("operation_id")]
        assert len(calls) >= 2
        assert len({row["operation_id"] for row in calls}) == 1, "A retentativa duplicou a operação"
