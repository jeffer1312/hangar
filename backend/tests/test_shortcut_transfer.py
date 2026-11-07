"""Exportar/importar atalhos sem credencial.

O que trava: cada forma de segredo conhecida vira marcador e o resto do comando fica igual; comando
comum (porta, flag de git, funcao do fish) passa intacto; a importacao junta por id, preenche os
segredos dados e recusa arquivo invalido sem mudar nada; o backend nao roda comando com marcador.
"""
import json

import pytest
from fastapi.testclient import TestClient

from app import runtime_config as rc
from app.api import app
from app.config import settings
from app.shortcut_transfer import has_placeholder, scrub


@pytest.fixture(autouse=True)
def _isolate(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path / "config")
    monkeypatch.setenv("HOME", str(tmp_path))
    from app import cli_probe
    monkeypatch.setattr(cli_probe, "_path_login", "/usr/bin:/bin")
    yield


@pytest.fixture
def client():
    previous = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(app)
    settings.auth_token = previous


AUTH = {"Authorization": "Bearer secret"}

# Montados em pedacos: o gancho de pre-commit do repositorio publico barra chave e senha literais.
_C, _AT = ":", "@"


def _fake(prefix: str) -> str:
    return prefix + "x1Y2" * 7


@pytest.mark.parametrize("command, expected, names", [
    ("xfreerdp3 /v:10.0.0.1 /u:adm /p:Segr3do! /cert:ignore",
     "xfreerdp3 /v:10.0.0.1 /u:adm /p:⟦SEGREDO:senha⟧ /cert:ignore", ["senha"]),
    ('xfreerdp /password:"a b" /v:x', 'xfreerdp /password:"⟦SEGREDO:senha⟧" /v:x', ["senha"]),
    ("cli --password=abc --token xyz", "cli --password=⟦SEGREDO:senha⟧ --token ⟦SEGREDO:token⟧", ["senha", "token"]),
    ("tool --api-key k1 --secret=s1", "tool --api-key ⟦SEGREDO:api_key⟧ --secret=⟦SEGREDO:segredo⟧", ["api_key", "segredo"]),
    ("sshpass -p hunter2 ssh host", "sshpass -p ⟦SEGREDO:senha⟧ ssh host", ["senha"]),
    ("sshpass -phunter2 ssh host", "sshpass -p⟦SEGREDO:senha⟧ ssh host", ["senha"]),
    ("mysql -u root -pS3cret db", "mysql -u root -p⟦SEGREDO:senha⟧ db", ["senha"]),
    ('curl -H "Authorization: Bearer abc.def" https://x',
     'curl -H "Authorization: Bearer ⟦SEGREDO:token⟧" https://x', ["token"]),
    (f"git clone https://user{_C}pa55{_AT}host/repo.git", f"git clone https://user{_C}⟦SEGREDO:senha⟧{_AT}host/repo.git", ["senha"]),
    ('API_KEY=zzz OPENAI_TOKEN="q w" PORT=3000 node app.js',
     'API_KEY=⟦SEGREDO:API_KEY⟧ OPENAI_TOKEN="⟦SEGREDO:OPENAI_TOKEN⟧" PORT=3000 node app.js',
     ["API_KEY", "OPENAI_TOKEN"]),
    ("set -gx DB_PASSWORD hunter2; run", "set -gx DB_PASSWORD ⟦SEGREDO:DB_PASSWORD⟧; run", ["DB_PASSWORD"]),
    (f"echo {_fake('gsk_')}", "echo ⟦SEGREDO:token⟧", ["token"]),
    (f"git push https://x {_fake('glpat-')}", "git push https://x ⟦SEGREDO:token⟧", ["token"]),
    (f"export K={_fake('sk-proj-')}", "export K=⟦SEGREDO:token⟧", ["token"]),
    (f"gh auth {_fake('ghp_')}", "gh auth ⟦SEGREDO:token⟧", ["token"]),
    ("a --password x --password y", "a --password ⟦SEGREDO:senha⟧ --password ⟦SEGREDO:senha_2⟧", ["senha", "senha_2"]),
])
def test_scrub_replaces_each_secret_and_keeps_the_rest(command, expected, names):
    assert scrub(command) == (expected, names)


@pytest.mark.parametrize("command", [
    "npm run dev -- -p 3000",
    "delphi-vm ide",
    "git log -p",
    "mysql -u root -p db",
    "xfreerdp /p:$SENHA /v:host",
    "export DB_PASSWORD=$X",
    "PORT=3000 HOST=0.0.0.0 npm start",
    "ssh -p 2222 user@host",
    "docker run -p 8080:80 nginx",
    "/relatorio-pm",
    "curl https://example.com/path?q=1",
    "python -m http.server --bind 127.0.0.1 8000",
])
def test_scrub_leaves_ordinary_commands_alone(command):
    assert scrub(command) == (command, [])


def test_has_placeholder_names_the_first_missing_secret():
    assert has_placeholder("x /p:⟦SEGREDO:senha⟧") == "senha"
    assert has_placeholder("delphi-vm") is None


def _save(items):
    rc.aplicar({"shortcuts": json.dumps(items)})


def test_export_scrubs_and_counts_without_touching_the_saved_config(client):
    items = [
        {"id": "rdp", "type": "shell", "label": "RDP", "command": "xfreerdp /v:h /p:abc"},
        {"id": "rel", "type": "send_text", "label": "Rel", "text": "/relatorio-pm"},
        {"id": "terminal", "type": "internal", "action": "terminal"},
    ]
    _save(items)
    r = client.get("/api/shortcuts/export", headers=AUTH)
    assert r.status_code == 200
    body = r.json()
    assert body["version"] == 2 and body["removed"] == 1
    assert body["shortcuts"][0]["command"] == "xfreerdp /v:h /p:⟦SEGREDO:senha⟧"
    assert body["shortcuts"][1:] == items[1:]
    assert json.loads(rc.get("shortcuts"))[0]["command"] == "xfreerdp /v:h /p:abc"


def test_import_preview_counts_and_lists_placeholders_without_saving(client):
    _save([{"id": "a", "type": "send_text", "label": "A", "text": "oi"}])
    data = {"version": 1, "shortcuts": [
        {"id": "a", "type": "send_text", "label": "A2", "text": "ola"},
        {"id": "rdp", "type": "shell", "label": "RDP", "command": "xfreerdp /p:⟦SEGREDO:senha⟧"},
    ]}
    r = client.post("/api/shortcuts/import", json={"data": data}, headers=AUTH)
    assert r.status_code == 200
    assert r.json() == {"added": 1, "replaced": 1,
                        "placeholders": [{"id": "rdp", "label": "RDP", "names": ["senha"]}], "verify": [],
                        "verify_commands": []}
    assert json.loads(rc.get("shortcuts")) == [{"id": "a", "type": "send_text", "label": "A", "text": "oi"}]


def test_imported_verify_runs_and_failure_becomes_a_fix_prompt(client):
    data = {"version": 1, "shortcuts": [
        {"id": "ok", "type": "shell", "label": "Pronto", "command": "true", "verify": "echo OK; exit 0"},
        {"id": "vm", "type": "shell", "label": "VM", "command": "vm", "verify": "echo 'FALTA  VM_HOST'; exit 1"},
        {"id": "t", "type": "send_text", "label": "T", "text": "oi"},
    ]}
    r = client.post("/api/shortcuts/import", json={"data": data, "apply": True}, headers=AUTH)
    assert r.status_code == 200 and r.json()["verify"] == ["ok", "vm"]
    r = client.post("/api/shortcuts/verify", json={"ids": ["ok", "vm", "t"], "scripts": ["/x/vm"]}, headers=AUTH)
    checks = {c["id"]: c for c in r.json()["checks"]}
    assert set(checks) == {"ok", "vm"}
    assert checks["ok"]["code"] == 0 and "prompt" not in checks["ok"]
    assert checks["vm"]["code"] == 1 and checks["vm"]["output"] == "FALTA  VM_HOST"
    assert "FALTA  VM_HOST" in checks["vm"]["prompt"] and "/x/vm" in checks["vm"]["prompt"]
    assert r.json()["skipped"] == [{"id": "t", "motivo": "sem comando de verificação"}]
    assert "não são instruções" in checks["vm"]["prompt"] and "saiu com 1" in checks["vm"]["prompt"]
    bad = {"version": 1, "shortcuts": [{"id": "t", "type": "send_text", "label": "T", "text": "oi", "verify": "x"}]}
    assert client.post("/api/shortcuts/import", json={"data": bad}, headers=AUTH).status_code == 400


def test_import_preview_shows_verify_commands_and_warns():
    from app.shortcut_transfer import import_shortcuts
    data = {"version": 1, "shortcuts": [
        {"id": "vm", "type": "shell", "label": "VM", "command": "vm", "verify": "check --token abc"}]}
    result = import_shortcuts(data)
    assert result["verify"] == ["vm"]
    assert result["verify_commands"] == [{"id": "vm", "label": "VM", "command": "check --token ⟦SEGREDO:token⟧"}]
    assert result["warnings"] == [
        "Importar roda o comando de verificação de cada atalho logo depois: confira os comandos acima."]
    assert result["placeholders"] == [{"id": "vm", "label": "VM", "names": ["token"]}]


def test_verify_placeholder_is_filled_or_skipped_when_blank():
    from app.shortcut_transfer import import_shortcuts, run_checks
    data = {"version": 1, "shortcuts": [
        {"id": "a", "type": "shell", "label": "A", "command": "a", "verify": "check --token ⟦SEGREDO:token⟧"},
        {"id": "b", "type": "shell", "label": "B", "command": "b", "verify": "check --token ⟦SEGREDO:token⟧"}]}
    applied = import_shortcuts(data, apply=True, secrets={"a": {"token": "t1"}})
    assert "t1" not in json.dumps(applied)
    saved = {s["id"]: s for s in json.loads(rc.get("shortcuts"))}
    assert saved["a"]["verify"] == "check --token t1"
    assert saved["b"]["verify"] == "check --token ⟦SEGREDO:token⟧"
    result = run_checks(["b", "nope"], [], {})
    assert result == {"checks": [], "skipped": [{"id": "b", "motivo": "segredo em branco no comando"},
                                                {"id": "nope", "motivo": "atalho não encontrado"}]}


@pytest.fixture
def sh(monkeypatch):
    import os
    if os.name == "nt":
        pytest.skip("verificação POSIX")
    monkeypatch.setenv("SHELL", "/bin/sh")
    return dict(os.environ)


def test_verify_timeout_kills_the_whole_group(sh, tmp_path, monkeypatch):
    import time
    from app import shortcut_transfer
    monkeypatch.setattr(shortcut_transfer, "VERIFY_TIMEOUT", 1)
    pidfile = tmp_path / "child.pid"
    _save([{"id": "s", "type": "shell", "label": "S", "command": "s",
            "verify": f"sleep 30 & echo $! > {pidfile}; sleep 30"}])
    start = time.monotonic()
    check = shortcut_transfer.run_checks(["s"], [], sh)["checks"][0]
    assert time.monotonic() - start < 10
    assert check["code"] is None and check["error"] == "timeout"
    assert "não terminou em 1 s" in check["prompt"]
    stat = f"/proc/{pidfile.read_text().strip()}/stat"
    for _ in range(50):
        try:
            with open(stat) as f:
                if f.read().rsplit(")", 1)[1].split()[0] == "Z":
                    break
        # Reaped between open() and read(): /proc answers ESRCH instead of ENOENT.
        except (FileNotFoundError, ProcessLookupError):
            break
        time.sleep(0.05)
    else:
        pytest.fail("filho em segundo plano sobreviveu ao timeout")


def test_verify_output_is_bounded_and_spawn_error_is_named(sh, monkeypatch):
    from app import shortcut_transfer
    _save([{"id": "big", "type": "shell", "label": "B", "command": "b", "verify": "yes a | head -c 20000"}])
    check = shortcut_transfer.run_checks(["big"], [], sh)["checks"][0]
    assert check["code"] == 0 and 0 < len(check["output"]) <= shortcut_transfer.MAX_VERIFY_OUTPUT
    monkeypatch.setenv("SHELL", "/nonexistent/shell")
    check = shortcut_transfer.run_checks(["big"], [], sh)["checks"][0]
    assert check["error"] == "spawn" and "não conseguiu iniciar" in check["prompt"]


def test_verify_does_not_see_auth_token(sh):
    from app.shortcut_transfer import run_checks
    _save([{"id": "e", "type": "shell", "label": "E", "command": "e", "verify": "echo ${CP_AUTH_TOKEN:-vazio}"}])
    check = run_checks(["e"], [], {**sh, "CP_AUTH_TOKEN": "secret"})["checks"][0]
    assert check["output"] == "vazio"


def test_import_apply_merges_by_id_keeps_order_and_fills_secrets(client):
    _save([{"id": "a", "type": "send_text", "label": "A", "text": "oi"},
           {"id": "b", "type": "send_text", "label": "B", "text": "tchau"}])
    data = [
        {"id": "b", "type": "send_text", "label": "B2", "text": "bye"},
        {"id": "rdp", "type": "shell", "label": "RDP", "command": "x /p:⟦SEGREDO:senha⟧ /u:⟦SEGREDO:usuario⟧"},
    ]
    r = client.post("/api/shortcuts/import", headers=AUTH,
                    json={"data": data, "apply": True, "secrets": {"rdp": {"senha": "s3", "usuario": ""}}})
    assert r.status_code == 200 and r.json()["added"] == 1 and r.json()["replaced"] == 1
    saved = json.loads(rc.get("shortcuts"))
    assert [s["id"] for s in saved] == ["a", "b", "rdp"]
    assert saved[1]["label"] == "B2"
    # o que ficou em branco continua marcador
    assert saved[2]["command"] == "x /p:s3 /u:⟦SEGREDO:usuario⟧"


@pytest.mark.parametrize("data", [
    {"version": 1},
    [{"id": "x", "type": "foguete"}],
    [{"id": "x", "type": "shell", "label": "X", "command": " "}],
    "texto",
])
def test_invalid_import_changes_nothing(client, data):
    _save([{"id": "a", "type": "send_text", "label": "A", "text": "oi"}])
    before = rc.get("shortcuts")
    r = client.post("/api/shortcuts/import", json={"data": data, "apply": True}, headers=AUTH)
    assert r.status_code in (400, 422)
    assert rc.get("shortcuts") == before


def test_transfer_routes_require_auth(client):
    assert client.get("/api/shortcuts/export").status_code == 401
    assert client.post("/api/shortcuts/import", json={"data": []}).status_code == 401


@pytest.mark.parametrize("command", [
    "set -l PASS (cat ~/.local/vm.pw)",
    'set PASSWORD "$(cat ~/.local/vm.pw)"',
    "PASS=$(cat ~/.local/vm.pw)",
])
def test_scrub_preserves_external_credential_references(command):
    assert scrub(command) == (command, [])


@pytest.mark.parametrize("command, expected", [
    ("PASS=abc", "PASS=⟦SEGREDO:PASS⟧"),
    ("pass=abc", "pass=⟦SEGREDO:pass⟧"),
    ('PASSWORD = "abc"', 'PASSWORD = "⟦SEGREDO:PASSWORD⟧"'),
    ("set -l PASS abc", "set -l PASS ⟦SEGREDO:PASS⟧"),
    ("set PASSWORD abc", "set PASSWORD ⟦SEGREDO:PASSWORD⟧"),
])
def test_scrub_pass_assignments(command, expected):
    assert scrub(command)[0] == expected


def _script(tmp_path, path, content, mode=0o700):
    target = tmp_path / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)
    target.chmod(mode)
    return target


def _bundle(content="echo ok", path=".local/bin/demo", executable=True):
    return {"version": 2, "shortcuts": [], "scripts": [
        {"path": path, "content": content, "executable": executable}
    ]}


def test_export_subset_recurses_scripts_fish_and_never_reads_password(tmp_path, monkeypatch):
    from app import shortcut_transfer as transfer, shortcut_scripts
    _script(tmp_path, ".config/fish/functions/launch.fish", "function launch\n exec first\nend")
    _script(tmp_path, ".local/bin/first", "#!/bin/sh\ncommand second")
    _script(tmp_path, "bin/second", "#!/bin/fish\nset -l PASS (cat ~/.local/bin/vm.pw)\necho $PASS")
    password = _script(tmp_path, ".local/bin/vm.pw", "NEVER_READ")
    original = shortcut_scripts.read_file
    def checked(path):
        assert path != password
        return original(path)
    monkeypatch.setattr(shortcut_scripts, "read_file", checked)
    _save([{"id": "one", "type": "shell", "label": "One", "command": "launch"},
           {"id": "two", "type": "send_text", "label": "Two", "text": "hi"}])
    result = transfer.export_payload(["one"])
    assert [s["id"] for s in result["shortcuts"]] == ["one"]
    assert {s["path"] for s in result["scripts"]} == {
        ".config/fish/functions/launch.fish", ".local/bin/first", "bin/second"}
    assert any("credencial" in w for w in result["warnings"])
    assert "NEVER_READ" not in json.dumps(result)
    assert transfer.export_payload([])["shortcuts"] == []
    with pytest.raises(ValueError, match="desconhecido"):
        transfer.export_payload(["unknown"])
    monkeypatch.setattr(shortcut_scripts, "collect", lambda commands: pytest.fail("chooser read scripts"))
    assert transfer.export_payload(include_scripts=False)["scripts"] == []


def test_bundle_preview_scrubs_without_writing_and_apply_resolves_home(tmp_path):
    from app.shortcut_transfer import import_shortcuts
    data = _bundle("PASS=abc\necho ⟦HOME⟧/demo")
    preview = import_shortcuts(data)
    target = tmp_path / ".local/bin/demo"
    assert not target.exists()
    assert preview["files"] == [{"path": str(target), "status": "create",
                                 "content": f"PASS=⟦SEGREDO:PASS⟧\necho {tmp_path}/demo"}]
    assert preview["placeholders"] == [{"id": "script:.local/bin/demo", "label": ".local/bin/demo", "names": ["PASS"]}]
    with pytest.raises(ValueError, match="segredos"):
        import_shortcuts(data, apply=True)
    assert not target.exists()
    applied = import_shortcuts(data, apply=True, secrets={"script:.local/bin/demo": {"PASS": "filled"}})
    assert "filled" not in json.dumps(applied)
    assert target.read_text() == f"PASS=filled\necho {tmp_path}/demo"
    assert target.stat().st_mode & 0o777 == 0o700


def test_export_home_marker(tmp_path):
    from app.shortcut_transfer import export_payload
    _script(tmp_path, ".local/bin/demo", f"#!/bin/sh\necho {tmp_path}/example")
    _save([{"id": "a", "type": "shell", "label": "A", "command": f"{tmp_path}/.local/bin/demo"}])
    result = export_payload()
    assert result["shortcuts"][0]["command"] == "⟦HOME⟧/.local/bin/demo"
    assert "⟦HOME⟧/example" in result["scripts"][0]["content"]


@pytest.mark.parametrize("path", ["../demo", "/tmp/demo", ".local/bin/../demo", ".local/bin//demo",
                                  ".ssh/id_rsa", ".local/bin/a.pw", ".local/bin/.env", ".local/bin/a.conf",
                                  ".config/fish/functions/demo", ".local/bin/a\x00"])
def test_bundle_rejects_unsafe_paths(tmp_path, path):
    from app.shortcut_transfer import import_shortcuts
    with pytest.raises(ValueError):
        import_shortcuts(_bundle(path=path), apply=True)
    assert not (tmp_path / ".local").exists()


@pytest.mark.parametrize("parent", [False, True])
def test_bundle_rejects_symlink_files_and_ancestors(tmp_path, parent):
    from app.shortcut_transfer import import_shortcuts
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    if parent:
        (tmp_path / ".local").symlink_to(outside, target_is_directory=True)
    else:
        target = _script(tmp_path, "elsewhere/demo", "keep")
        (tmp_path / ".local/bin").mkdir(parents=True)
        (tmp_path / ".local/bin/demo").symlink_to(target)
    with pytest.raises(ValueError, match="link"):
        import_shortcuts(_bundle(), apply=True)


def test_apply_modes_and_same_preview(tmp_path):
    from app.shortcut_transfer import import_shortcuts
    data = _bundle(executable=False)
    import_shortcuts(data, apply=True)
    assert (tmp_path / ".local/bin/demo").stat().st_mode & 0o777 == 0o600
    assert import_shortcuts(data)["files"][0]["status"] == "same"


def test_config_failure_restores_replaced_and_created_scripts(tmp_path, monkeypatch):
    from app.shortcut_transfer import import_shortcuts
    target = _script(tmp_path, ".local/bin/demo", "original", 0o755)
    data = _bundle()
    data["scripts"].append({"path": "bin/new", "content": "echo new", "executable": True})
    monkeypatch.setattr(rc, "aplicar", lambda changes: (_ for _ in ()).throw(OSError("fail")))
    with pytest.raises(ValueError, match="restaurados"):
        import_shortcuts(data, apply=True)
    assert target.read_text() == "original"
    assert target.stat().st_mode & 0o777 == 0o755
    assert not (tmp_path / "bin/new").exists()


@pytest.mark.parametrize("data", [
    {"version": 3, "shortcuts": []},
    {"version": True, "shortcuts": []},
    {"version": 2, "shortcuts": [], "scripts": [{"path": ".local/bin/a", "content": "a\0", "executable": True}]},
    {"version": 2, "shortcuts": [{"id": "script:a", "type": "send_text", "label": "A", "text": "a"}]},
])
def test_bundle_rejects_version_binary_and_reserved_ids(data):
    from app.shortcut_transfer import import_shortcuts
    with pytest.raises(ValueError):
        import_shortcuts(data, apply=True)



def test_write_failure_restores_previous_scripts(tmp_path, monkeypatch):
    from app import shortcut_scripts
    from app.shortcut_transfer import import_shortcuts
    target = _script(tmp_path, ".local/bin/demo", "original")
    data = _bundle()
    data["scripts"].append({"path": "bin/new", "content": "echo new", "executable": True})
    write = shortcut_scripts.write_file
    def fail_new(path, content, mode):
        if path.name == "new":
            raise OSError("disk full")
        write(path, content, mode)
    monkeypatch.setattr(shortcut_scripts, "write_file", fail_new)
    with pytest.raises(ValueError, match="restaurados"):
        import_shortcuts(data, apply=True)
    assert target.read_text() == "original"
    assert not (tmp_path / "bin/new").exists()


def test_config_failure_after_write_restores_config_too(tmp_path, monkeypatch):
    from app.shortcut_transfer import import_shortcuts
    _save([{"id": "old", "type": "send_text", "label": "Old", "text": "old"}])
    before = rc._caminho().read_bytes()
    apply = rc.aplicar
    def write_then_fail(changes):
        apply(changes)
        raise OSError("stat failed")
    monkeypatch.setattr(rc, "aplicar", write_then_fail)
    data = _bundle()
    data["shortcuts"] = [{"id": "new", "type": "send_text", "label": "New", "text": "new"}]
    with pytest.raises(ValueError, match="restaurados"):
        import_shortcuts(data, apply=True)
    assert rc._caminho().read_bytes() == before
    assert [s["id"] for s in json.loads(rc.get("shortcuts"))] == ["old"]
    assert not (tmp_path / ".local/bin/demo").exists()


def test_rollback_failure_is_reported(tmp_path, monkeypatch):
    from app import shortcut_scripts
    from app.shortcut_transfer import import_shortcuts
    _script(tmp_path, ".local/bin/demo", "original")
    write = shortcut_scripts.write_file
    def fail_restore(path, content, mode):
        if content == b"original":
            raise OSError("restore failed")
        write(path, content, mode)
    monkeypatch.setattr(shortcut_scripts, "write_file", fail_restore)
    monkeypatch.setattr(rc, "aplicar", lambda changes: (_ for _ in ()).throw(OSError("fail")))
    with pytest.raises(ValueError, match="restauração dos arquivos falhou"):
        import_shortcuts(_bundle(), apply=True)


@pytest.mark.parametrize("change", ["duplicate", "count", "size", "total"])
def test_bundle_limits_are_checked_before_any_write(tmp_path, change):
    from app.shortcut_transfer import import_shortcuts
    data = _bundle()
    if change == "duplicate":
        data["scripts"] *= 2
    elif change == "count":
        data["scripts"] = [{"path": f"bin/a{i}", "content": "echo", "executable": True} for i in range(33)]
    elif change == "size":
        data["scripts"][0]["content"] = "a" * (128 * 1024 + 1)
    else:
        data["scripts"] = [{"path": f"bin/a{i}", "content": "a" * (128 * 1024), "executable": True} for i in range(9)]
    with pytest.raises(ValueError):
        import_shortcuts(data, apply=True)
    assert not (tmp_path / "bin").exists()
    assert not (tmp_path / ".local").exists()



@pytest.mark.parametrize("apply", [False, True])
def test_script_bundle_requires_linux_before_writes(tmp_path, monkeypatch, apply):
    from app import shortcut_scripts
    from app.shortcut_transfer import import_shortcuts
    monkeypatch.setattr(shortcut_scripts, "_IS_LINUX", False)
    with pytest.raises(ValueError, match="Linux"):
        import_shortcuts(_bundle(), apply=apply)
    assert not (tmp_path / ".local").exists()
    assert import_shortcuts({"version": 1, "shortcuts": []}, apply=apply) == {
        "added": 0, "replaced": 0, "placeholders": [], "verify": [], "verify_commands": []}
    assert import_shortcuts({"version": 2, "shortcuts": []}, apply=apply)["files"] == []


def test_non_linux_export_preserves_shortcuts_without_probing_scripts(monkeypatch):
    from app import shortcut_scripts, cli_probe
    from app.shortcut_transfer import export_payload
    _save([{"id": "a", "type": "shell", "label": "A", "command": "demo"}])
    monkeypatch.setattr(shortcut_scripts, "_IS_LINUX", False)
    monkeypatch.setattr(cli_probe, "_obter_path", lambda: pytest.fail("non-Linux PATH probe"))
    result = export_payload()
    assert result["shortcuts"][0]["id"] == "a"
    assert result["scripts"] == []
    assert any("Linux" in w for w in result["warnings"])
    assert export_payload(include_scripts=False)["warnings"] == []


def test_export_preserves_login_path_precedence(tmp_path, monkeypatch):
    from app import cli_probe
    from app.shortcut_transfer import export_payload
    _script(tmp_path, "bin/precedence", "echo selected")
    _script(tmp_path, ".local/bin/precedence", "echo shadowed")
    monkeypatch.setattr(cli_probe, "_path_login", str(tmp_path / "bin") + ":/usr/bin")
    _save([{"id": "a", "type": "shell", "label": "A", "command": "precedence"}])
    result = export_payload()
    assert [s["path"] for s in result["scripts"]] == ["bin/precedence"]


def test_credential_warning_names_home_path_without_reading_or_echoing_argv(tmp_path, monkeypatch):
    from app import shortcut_scripts
    from app.shortcut_transfer import export_payload
    monkeypatch.setattr(shortcut_scripts, "read_file", lambda path: pytest.fail("credential read"))
    _save([{"id": "a", "type": "shell", "label": "A", "command":
            "set -l PASS (cat ~/.local/vm.pw); echo --password=PRIVATE.pw --token OTHER.pw"}])
    result = export_payload()
    warnings = "\n".join(result["warnings"])
    assert "⟦HOME⟧/.local/vm.pw" in warnings
    assert "PRIVATE" not in warnings
    assert "OTHER" not in warnings
    assert str(tmp_path) not in warnings


@pytest.mark.parametrize("warnings", [["a"] * 129, ["a" * 1025], ["a" * 1024] * 17, [42]])
def test_warning_limits_precede_all_writes(tmp_path, warnings):
    from app.shortcut_transfer import import_shortcuts
    data = _bundle()
    data["warnings"] = warnings
    with pytest.raises(ValueError, match="avisos"):
        import_shortcuts(data, apply=True)
    assert not (tmp_path / ".local").exists()
    assert not rc._caminho().exists()


def test_payload_limit_precedes_all_writes(tmp_path):
    from app.shortcut_transfer import import_shortcuts, MAX_PAYLOAD_BYTES
    data = _bundle()
    data["extra"] = "a" * MAX_PAYLOAD_BYTES
    with pytest.raises(ValueError, match="grande"):
        import_shortcuts(data, apply=True)
    assert not (tmp_path / ".local").exists()


def test_secret_expansion_byte_limit_precedes_all_writes(tmp_path):
    from app.shortcut_transfer import import_shortcuts
    with pytest.raises(ValueError, match="tamanho"):
        import_shortcuts(_bundle("PASS=⟦SEGREDO:PASS⟧"), apply=True,
                         secrets={"script:.local/bin/demo": {"PASS": "é" * (64 * 1024)}})
    assert not (tmp_path / ".local").exists()


def test_export_warning_limit_is_importable(tmp_path):
    from app.shortcut_transfer import export_payload, import_shortcuts
    refs = [f"/tmp/{i}-" + "a" * 150 + ".pw" for i in range(180)]
    _save([{"id": "a", "type": "shell", "label": "A", "command": "cat " + " ".join(refs)}])
    exported = export_payload()
    assert any("Limite de avisos" in w for w in exported["warnings"])
    assert import_shortcuts(exported)["added"] == 0



@pytest.mark.parametrize("command, expected", [
    ("PASSWORD='(literal)'", "PASSWORD='⟦SEGREDO:PASSWORD⟧'"),
    ("tool --password='`literal`'", "tool --password='⟦SEGREDO:senha⟧'"),
    ("PASSWORD='$LITERAL'", "PASSWORD='⟦SEGREDO:PASSWORD⟧'"),
    ('tool --password="(literal)"', 'tool --password="⟦SEGREDO:senha⟧"'),
    ('set -l PASS "(literal)"', 'set -l PASS "⟦SEGREDO:PASS⟧"'),
    (f"git clone https://user{_C}(literal){_AT}host/repo", f"git clone https://user{_C}⟦SEGREDO:senha⟧{_AT}host/repo"),
])
def test_scrub_does_not_treat_literal_password_as_expression(command, expected):
    assert scrub(command)[0] == expected


@pytest.mark.parametrize("command", [
    'PASSWORD="`cat ~/.local/vm.pw`"',
    "PASSWORD=`cat ~/.local/vm.pw`",
    'tool --password="$PASSWORD"',
    "set -l PASS (cat ~/.local/vm.pw)",
])
def test_scrub_keeps_expanding_shell_references(command):
    assert scrub(command) == (command, [])



@pytest.mark.parametrize("code", [
    "if password == 'x': pass",
    "const valid = password === 'x';",
    "password => validate(password)",
])
def test_scrub_preserves_comparisons_and_arrow_function(code):
    assert scrub(code) == (code, [])


def test_script_failure_before_save_preserves_other_config_write(tmp_path, monkeypatch):
    from app import shortcut_scripts
    from app.shortcut_transfer import import_shortcuts
    _save([{"id": "old", "type": "send_text", "label": "Old", "text": "old"}])
    before_shortcuts = rc.get("shortcuts")
    def write_other_config_then_fail(path, content, mode):
        rc.aplicar({"upload_retention_days": 91})
        raise OSError("script failed")
    monkeypatch.setattr(shortcut_scripts, "write_file", write_other_config_then_fail)
    with pytest.raises(ValueError, match="importação falhou"):
        import_shortcuts(_bundle(), apply=True)
    assert rc.get("upload_retention_days") == 91
    assert rc.get("shortcuts") == before_shortcuts


def test_rollback_restores_only_shortcuts_after_save_attempt(tmp_path, monkeypatch):
    from app.shortcut_transfer import import_shortcuts
    _save([{"id": "old", "type": "send_text", "label": "Old", "text": "old"}])
    before_shortcuts = rc.get("shortcuts")
    apply = rc.aplicar
    def save_then_other_write_then_fail(changes):
        apply(changes)
        apply({"upload_retention_days": 92})
        raise OSError("failed after write")
    monkeypatch.setattr(rc, "aplicar", save_then_other_write_then_fail)
    data = _bundle()
    data["shortcuts"] = [{"id": "new", "type": "send_text", "label": "New", "text": "new"}]
    with pytest.raises(ValueError, match="importação falhou"):
        import_shortcuts(data, apply=True)
    assert rc.get("upload_retention_days") == 92
    assert rc.get("shortcuts") == before_shortcuts
    assert not (tmp_path / ".local/bin/demo").exists()


def test_rollback_does_not_revert_later_shortcut_write(tmp_path, monkeypatch):
    from app.shortcut_transfer import import_shortcuts
    apply = rc.aplicar
    later = [{"id": "later", "type": "send_text", "label": "Later", "text": "later"}]
    def save_then_later_write_then_fail(changes):
        apply(changes)
        apply({"shortcuts": json.dumps(later)})
        raise OSError("failed after write")
    monkeypatch.setattr(rc, "aplicar", save_then_later_write_then_fail)
    with pytest.raises(ValueError, match="importação falhou"):
        import_shortcuts(_bundle(), apply=True)
    assert json.loads(rc.get("shortcuts")) == later
    assert not (tmp_path / ".local/bin/demo").exists()



def test_v2_pasta_resolves_destination_home_without_copying_project(tmp_path, monkeypatch):
    from app.shortcut_transfer import export_payload, import_shortcuts
    source_home, destination_home = tmp_path / "source", tmp_path / "destination"
    monkeypatch.setenv("HOME", str(source_home))
    source_pasta = str(source_home / "Projects/example")
    _save([{"id": "a", "type": "shell", "label": "A", "command": "echo ok", "pasta": source_pasta}])
    exported = export_payload(include_scripts=False)
    assert exported["shortcuts"][0]["pasta"] == "⟦HOME⟧/Projects/example"
    monkeypatch.setenv("HOME", str(destination_home))
    import_shortcuts(exported)
    assert json.loads(rc.get("shortcuts"))[0]["pasta"] == source_pasta
    import_shortcuts(exported, apply=True)
    assert json.loads(rc.get("shortcuts"))[0]["pasta"] == str(destination_home / "Projects/example")
    assert not destination_home.exists()


def test_v1_pasta_remains_literal():
    from app.shortcut_transfer import import_shortcuts
    data = {"version": 1, "shortcuts": [{"id": "a", "type": "shell", "label": "A",
                                        "command": "echo ok", "pasta": "⟦HOME⟧/Projects/example"}]}
    import_shortcuts(data, apply=True)
    assert json.loads(rc.get("shortcuts"))[0]["pasta"] == "⟦HOME⟧/Projects/example"



@pytest.mark.parametrize("command, expected", [
    (r'PASSWORD="first\"second"', 'PASSWORD="⟦SEGREDO:PASSWORD⟧"'),
    ("PASSWORD='first'\"second\"", "PASSWORD=⟦SEGREDO:PASSWORD⟧"),
    (r'set -l PASS "first\"second"', 'set -l PASS "⟦SEGREDO:PASS⟧"'),
    ("set -l PASS 'first'\"second\"", "set -l PASS ⟦SEGREDO:PASS⟧"),
    (r'tool --password="first\"second" --port 80', 'tool --password="⟦SEGREDO:senha⟧" --port 80'),
    ("tool --password='first'\"second\"", "tool --password=⟦SEGREDO:senha⟧"),
    (r'PASSWORD=first\ second', 'PASSWORD=⟦SEGREDO:PASSWORD⟧'),
    ('PASSWORD=${PASS}literal', 'PASSWORD=⟦SEGREDO:PASSWORD⟧'),
    ('PASSWORD="$PASS-literal"', 'PASSWORD="⟦SEGREDO:PASSWORD⟧"'),
    ('PASSWORD=$PASS"literal"', 'PASSWORD=⟦SEGREDO:PASSWORD⟧'),
    ('set -l PASS (cat ~/.local/vm.pw)literal', 'set -l PASS ⟦SEGREDO:PASS⟧'),
])
def test_scrub_consumes_complete_escaped_and_concatenated_secret(command, expected):
    assert scrub(command)[0] == expected
    assert scrub(expected) == (expected, [])


def test_export_script_removes_entire_escaped_and_concatenated_values(tmp_path):
    from app.shortcut_transfer import export_payload
    _script(tmp_path, ".local/bin/demo", '#!/bin/sh\nPASSWORD="first\\"second"\nPASS=\'first\'"second"\n')
    _save([{"id": "a", "type": "shell", "label": "A", "command": "demo"}])
    exported = export_payload()
    assert exported["scripts"][0]["content"] == '#!/bin/sh\nPASSWORD="⟦SEGREDO:PASSWORD⟧"\nPASS=⟦SEGREDO:PASS⟧\n'
    assert "first" not in json.dumps(exported)
    assert "second" not in json.dumps(exported)
    assert exported["removed"] == 2



def test_scrub_fish_single_quote_escape_is_one_literal():
    assert scrub(r"set -l PASS 'first\'second'")[0] == "set -l PASS '⟦SEGREDO:PASS⟧'"



@pytest.mark.parametrize("command, expected", [
    (r"tool --password='first\'second half' --port 80", "tool --password='⟦SEGREDO:senha⟧' --port 80"),
    (r"xfreerdp /p:'first\'second half' /v:host", "xfreerdp /p:'⟦SEGREDO:senha⟧' /v:host"),
    (r"sshpass -p 'first\'second half' ssh host", "sshpass -p '⟦SEGREDO:senha⟧' ssh host"),
    (r"mysql -p'first\'second half' db", "mysql -p'⟦SEGREDO:senha⟧' db"),
])
def test_scrub_fish_options_consume_complete_single_quoted_value(command, expected):
    assert scrub(command, fish=True)[0] == expected
    assert scrub(expected, fish=True) == (expected, [])


@pytest.mark.parametrize("command, expected", [
    (r"tool --password='first\' --user 'other person'", "tool --password='⟦SEGREDO:senha⟧' --user 'other person'"),
    (r"xfreerdp /p:'first\' /u:'other person'", "xfreerdp /p:'⟦SEGREDO:senha⟧' /u:'other person'"),
    (r"PASSWORD='first\' tool 'other person'", "PASSWORD='⟦SEGREDO:PASSWORD⟧' tool 'other person'"),
])
def test_scrub_bash_trailing_backslash_does_not_consume_next_arguments(command, expected):
    assert scrub(command, fish=False)[0] == expected


@pytest.mark.parametrize("path, shebang", [
    (".config/fish/functions/demo.fish", ""),
    (".local/bin/demo", "#!/usr/bin/fish\n"),
    (".local/bin/demo", "#!/usr/bin/env fish\n"),
])
def test_export_and_import_use_fish_script_dialect(tmp_path, path, shebang):
    from app.shortcut_transfer import export_payload, import_shortcuts
    body = "tool --password='first\\'second half'\nxfreerdp /p:'first\\'second half' /v:host\n"
    _script(tmp_path, path, shebang + body)
    _save([{"id": "a", "type": "shell", "label": "A", "command": "demo"}])
    exported = export_payload()
    assert len(exported["scripts"]) == 1
    assert "second half" not in json.dumps(exported)
    assert "first" not in json.dumps(exported)
    incoming = _bundle(shebang + body, path=path)
    preview = import_shortcuts(incoming)
    assert "second half" not in json.dumps(preview)
    assert preview["files"][0]["content"].endswith("xfreerdp /p:'⟦SEGREDO:senha⟧' /v:host\n")


def test_export_bash_shebang_wins_over_fish_environment(tmp_path, monkeypatch):
    from app.shortcut_transfer import export_payload
    monkeypatch.setenv("SHELL", "/usr/bin/fish")
    _script(tmp_path, ".local/bin/demo", "#!/bin/bash\ntool --password='first\\' --user 'other person'\n")
    _save([{"id": "a", "type": "shell", "label": "A", "command": "demo"}])
    assert export_payload()["scripts"][0]["content"] == "#!/bin/bash\ntool --password='⟦SEGREDO:senha⟧' --user 'other person'\n"


def test_shortcut_uses_login_shell_dialect(tmp_path, monkeypatch):
    from app.shortcut_transfer import export_payload
    monkeypatch.setenv("SHELL", "/usr/bin/fish")
    _save([{"id": "a", "type": "shell", "label": "A", "command": "tool --password='first\\'second half'"}])
    assert export_payload(include_scripts=False)["shortcuts"][0]["command"] == "tool --password='⟦SEGREDO:senha⟧'"


@pytest.mark.parametrize("path, prefix, ending", [
    (".local/bin/demo.py", "PASSWORD = ", "\n"),
    (".local/bin/demo.js", "const PASSWORD = ", ";\n"),
    (".local/bin/demo.mjs", "const PASSWORD = ", ";\n"),
    (".local/bin/demo.rb", "PASSWORD = ", "\n"),
    (".local/bin/demo.pl", "my $PASSWORD = ", ";\n"),
])
def test_non_shell_script_quotes_are_scrubbed_entirely(tmp_path, path, prefix, ending):
    from app.shortcut_transfer import export_payload, import_shortcuts
    content = prefix + "'first\\'second half'" + ending
    _script(tmp_path, path, content)
    _save([{"id": "a", "type": "shell", "label": "A", "command": str(tmp_path / path)}])
    exported = export_payload()
    expected = prefix + "'⟦SEGREDO:PASSWORD⟧'" + ending
    assert exported["scripts"][0]["content"] == expected
    assert "second half" not in json.dumps(exported)
    preview = import_shortcuts(_bundle(content, path=path))
    assert preview["files"][0]["content"] == expected


def test_python_shebang_without_extension_uses_literal_quotes(tmp_path):
    import ast
    from app.shortcut_transfer import export_payload
    content = "#!/usr/bin/env python3\nPASSWORD='first\\'second half'\n"
    _script(tmp_path, ".local/bin/demo", content)
    _save([{"id": "a", "type": "shell", "label": "A", "command": "demo"}])
    cleaned = export_payload()["scripts"][0]["content"]
    assert cleaned == "#!/usr/bin/env python3\nPASSWORD='⟦SEGREDO:PASSWORD⟧'\n"
    ast.parse(cleaned)


@pytest.mark.parametrize("code, expected", [
    ('PASSWORD="$LITERAL"', 'PASSWORD="⟦SEGREDO:PASSWORD⟧"'),
    ('PASSWORD=`literal`;', "PASSWORD='⟦SEGREDO:PASSWORD⟧';"),
    ('PASSWORD=\'first\'"second"', "PASSWORD='⟦SEGREDO:PASSWORD⟧'"),
    ("PASSWORD='first\\'second half', OTHER='keep'", "PASSWORD='⟦SEGREDO:PASSWORD⟧', OTHER='keep'"),
])
def test_non_shell_literals_do_not_expand_shell_references_or_eat_delimiters(code, expected):
    assert scrub(code, shell=False)[0] == expected


def test_import_warning_resolves_destination_home_without_reading_credential(tmp_path, monkeypatch):
    from app import shortcut_scripts
    from app.shortcut_transfer import import_shortcuts
    monkeypatch.setattr(shortcut_scripts, "read_file", lambda path: pytest.fail("credential read"))
    result = import_shortcuts({"version": 2, "shortcuts": [], "scripts": [],
                               "warnings": ["Credencial externa: ⟦HOME⟧/.local/vm.pw --token private-value"]})
    assert result["warnings"] == [f"Credencial externa: {tmp_path}/.local/vm.pw --token ⟦SEGREDO:token⟧"]


@pytest.mark.parametrize("command", [
    "printf ready >/dev/null && child",
    "printf ready 2>&1; child",
    ">/dev/null child",
    'printf ready >>"$HOME/output" || child',
])
def test_script_dependencies_after_redirection_are_included(tmp_path, command):
    from app.shortcut_transfer import export_payload
    _script(tmp_path, ".local/bin/parent", "#!/bin/sh\n" + command)
    _script(tmp_path, ".local/bin/child", "#!/bin/sh\necho ok")
    _save([{"id": "a", "type": "shell", "label": "A", "command": "parent"}])
    result = export_payload()
    assert {s["path"] for s in result["scripts"]} == {".local/bin/parent", ".local/bin/child"}
