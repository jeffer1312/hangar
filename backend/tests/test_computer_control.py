"""Tela do hangar-computer-control: grava em todos os .claude.json, desligar tira de todos e religar
volta com a configuração de antes."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from types import SimpleNamespace
from urllib.parse import urlsplit

import pytest

from app import computer_control as cc


@pytest.fixture
def home(tmp_path, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(cc.Path, "home", lambda: tmp_path)
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    conta = tmp_path / ".claude-x"
    conta.mkdir()
    (conta / ".claude.json").write_text(json.dumps({"oauthAccount": {"a": 1}}), encoding="utf-8")
    monkeypatch.setattr(cc, "list_config_dirs", lambda: [SimpleNamespace(path=str(conta))])
    projeto = tmp_path / "Projetos" / cc.NAME
    (projeto / ".venv" / "bin").mkdir(parents=True)
    (projeto / ".venv" / "bin" / "python").write_text("")
    (projeto / "servidor_mcp.py").write_text("")
    (projeto / "target" / "release").mkdir(parents=True)
    (projeto / "target" / "release" / cc.NAME).write_bytes(b"local-binary")
    (projeto / "alvo-agent.json").write_text("{}")
    (tmp_path / ".claude.json").write_text("{}", encoding="utf-8")
    return tmp_path


def _pedido(home, **extra):
    projeto = home / "Projetos" / cc.NAME
    return {"enabled": True, "project_dir": str(projeto), "agent_config": str(projeto / "alvo-agent.json"),
            "llm_url": "http://x/v1/chat/completions", "llm_model": "m1", "llm_effort": "high",
            "llm_key": "chave-do-llm-123", **extra}


def _entrada(p):
    return (json.loads(p.read_text()).get("mcpServers") or {}).get(cc.NAME)


def test_liga_desliga_e_religa_sem_perder_a_configuracao(home):
    from app import runtime_config as rc

    rc.aplicar({"jev_api_key": "chave-do-jev-456"})
    principal, conta = home / ".claude.json", home / ".claude-x" / ".claude.json"
    cc.save(_pedido(home))
    assert _entrada(principal) == _entrada(conta)
    assert _entrada(principal)["env"]["LLM_MODEL"] == "m1"
    assert json.loads(conta.read_text())["oauthAccount"] == {"a": 1}   # resto do arquivo intocado

    estado = cc.save({"enabled": False})
    assert _entrada(principal) is None and _entrada(conta) is None
    assert estado["enabled"] is False and estado["llm_model"] == "m1"   # a tela não perde os campos

    cc.save(_pedido(home, llm_key=None))   # chave vazia = mantém a guardada
    env = _entrada(conta)["env"]
    assert env["LLM_PROXY_KEY"] == "chave-do-llm-123" and env["TYPESAFE_API_KEY"] == "chave-do-jev-456"
    assert env["JEV_ENDPOINT"] == rc.JEV_TYPESAFE_URL and env["JEV_MODEL"] == "jev-latest"


def test_estado_nunca_devolve_a_chave_inteira(home):
    from app import runtime_config as rc

    rc.aplicar({"jev_api_key": "chave-do-jev-456"})
    cc.save(_pedido(home))
    texto = json.dumps(cc.state())
    assert "chave-do-llm-123" not in texto and "chave-do-jev-456" not in texto


def test_cria_alvo_ssh_e_recusa_repetido_e_local_sem_programa(home, monkeypatch):
    projeto = str(home / "Projetos" / cc.NAME)
    estado = cc.create_target({"project_dir": projeto, "name": "delphi-03", "transport": "ssh",
                               "host": "delphi-03", "proxy_command": "", "request_timeout": 40})
    alvo = next(t for t in estado["targets"] if t["name"] == "delphi-03")
    cfg = json.loads(open(alvo["path"]).read())
    assert cfg["transport"] == "ssh" and cfg["host"] == "delphi-03" and cfg["request_timeout"] == 40
    with pytest.raises(cc.ComputerControlError) as e:
        cc.create_target({"project_dir": projeto, "name": "delphi-03", "transport": "ssh", "host": "x"})
    assert e.value.code == "erro_computer_control_target_exists"
    monkeypatch.setattr(cc.os, "name", "posix")
    (home / "Projetos" / cc.NAME / "target" / "release" / cc.NAME).unlink()
    with pytest.raises(cc.ComputerControlError) as e:
        cc.create_target({"project_dir": projeto, "name": "eu", "transport": "local"})
    assert e.value.code == "erro_computer_control_local_only_windows"


def test_install_falls_back_to_uvx_when_release_has_no_binary(home, monkeypatch):
    projeto = home / "Projetos" / cc.NAME
    (projeto / "alvo-agent.json").write_text(json.dumps(
        {"transport": "ssh", "host": "h", "agent_path": str(projeto / "dist" / "windows-agent.exe")}))
    cc.save(_pedido(home))

    def falso_get(url, timeout):
        return json.dumps({"tag_name": "v0.1.0"}).encode() if "api.github.com" in url else b"MZ-exe"
    monkeypatch.setattr(cc, "_get", falso_get)
    monkeypatch.setattr(cc.shutil, "which", lambda n: "/usr/bin/uvx" if n == "uvx" else None)

    estado = cc.install()
    entrada = _entrada(home / ".claude-x" / ".claude.json")
    assert entrada["command"] == "/usr/bin/uvx"
    assert entrada["args"] == ["--from", f"git+https://github.com/{cc.REPO}@v0.1.0", cc.NAME]
    alvos = home / ".hangar" / "computer-control" / "targets"
    assert entrada["env"]["HCC_AGENTS_DIR"] == str(alvos)
    assert entrada["env"]["HCC_AGENT_CONFIG"] == str(alvos / "alvo-agent.json")
    assert "PYTHONPATH" not in entrada["env"] and entrada["env"]["LLM_MODEL"] == "m1"
    assert json.loads((alvos / "alvo-agent.json").read_text())["agent_path"] == str(
        home / ".hangar" / "computer-control" / "windows-agent.exe")
    assert (projeto / "alvo-agent.json").is_file()   # a pasta local não é apagada
    assert estado["mode"] == "package" and estado["installed_tag"] == "v0.1.0"
    assert estado["legacy_package"] is True
    assert "pacote Python" in estado["detail"]


def test_preserva_variavel_que_a_tela_nao_controla(home):
    principal = home / ".claude.json"
    cc.save(_pedido(home))
    dados = json.loads(principal.read_text())
    dados["mcpServers"][cc.NAME]["env"]["VIRTUAL_ENV"] = ""
    principal.write_text(json.dumps(dados))
    cc.save(_pedido(home))
    assert _entrada(principal)["env"]["VIRTUAL_ENV"] == ""


@pytest.mark.parametrize("configure_key", [False, True])
def test_first_install_without_checkout_creates_package_target(tmp_path, monkeypatch, configure_key):
    from app import runtime_config as rc

    monkeypatch.setattr(cc.Path, "home", lambda: tmp_path)
    monkeypatch.setattr(cc, "list_config_dirs", lambda: [])
    monkeypatch.setattr(cc, "_cliproxy_running", lambda: False)
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    monkeypatch.setattr(cc.shutil, "which", lambda name: "/bin/uvx" if name == "uvx" else None)
    monkeypatch.setattr(cc, "_get", lambda url, timeout: (
        b'{"tag_name":"v1.0.0"}' if "api.github.com" in url else b"MZ-agent"))
    assert cc.state()["mode"] == "package" and not cc.state()["package_exists"]
    if configure_key:
        rc.aplicar({"jev_api_key": "windows-secret-1234"})
        assert not cc.state()["enabled"]
        assert not cc._main_file().exists()

    installed = cc.install()
    assert installed["mode"] == "package" and installed["package_exists"]
    assert installed["enabled"] is False and installed["targets"] == []
    created = cc.create_target({"name": "desktop", "transport": "ssh", "host": "desktop"})
    target = cc._package_targets() / "desktop-agent.json"
    assert created["mode"] == "package" and created["enabled"] is False
    assert created["agent_config"] == str(target)
    assert json.loads(target.read_text())["agent_path"] == str(cc._package_exe())
    enabled = cc.save({"enabled": True, "agent_config": str(target), "llm_url": cc.PRESET_URL})
    assert enabled["enabled"] and enabled["mode"] == "package"
    if configure_key:
        assert _entrada(cc._main_file())["env"]["TYPESAFE_API_KEY"] == "windows-secret-1234"
        assert "windows-secret-1234" not in json.dumps(enabled)


def test_installed_package_does_not_override_existing_local_entry(home):
    cc.save(_pedido(home))
    cc._install_dir().mkdir(parents=True)
    cc._package_exe().write_bytes(b"MZ-agent")
    (cc._install_dir() / "install.json").write_text('{"tag":"v1.0.0","uvx":"uvx"}')
    assert cc.state()["mode"] == "local"
    assert cc.save({"enabled": False})["mode"] == "local"
    state = cc.create_target({"name": "still-local", "transport": "ssh", "host": "desktop"})
    assert state["mode"] == "local"
    assert (home / "Projetos" / cc.NAME / "still-local-agent.json").is_file()


def test_jev_config_reaches_active_and_parked_entries_only_in_jev_vars(home):
    from app import runtime_config as rc

    cc.save(_pedido(home))
    cc.save({"enabled": False})
    cc.save(_pedido(home))
    before = {path: json.loads(path.read_text()) for path in [*cc._config_files(), cc._parked_file()]}
    rc.aplicar({"jev_api_key": "sk-or-segredo"})
    for path, original in before.items():
        entry = original if path == cc._parked_file() else original["mcpServers"][cc.NAME]
        entry["env"].update({"TYPESAFE_API_KEY": "sk-or-segredo", "JEV_ENDPOINT": rc.JEV_OPENROUTER_URL,
                             "JEV_MODEL": "~typesafe/jev-latest"})
        assert json.loads(path.read_text()) == original
    state = cc.state()
    assert state["jev_key_set"] and state["jev_model"] == "~typesafe/jev-latest"
    assert "sk-or-segredo" not in json.dumps(state) and "jev_windows_api_key" not in rc.estado()
    rc.aplicar({"jev_model": "typesafe/jev-1.13-20260917"})
    assert _entrada(cc._main_file())["env"]["JEV_MODEL"] == "typesafe/jev-1.13-20260917"
    rc.aplicar({}, remover={"jev_api_key"})
    assert not {"TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL"} & _entrada(cc._main_file())["env"].keys()


def test_migrate_turns_the_computer_use_key_into_the_jev_key_once(home):
    from app import runtime_config as rc

    cc.save(_pedido(home))
    for path in cc._config_files():
        data = json.loads(path.read_text())
        data["mcpServers"][cc.NAME]["env"]["TYPESAFE_API_KEY"] = "old-windows-key"
        path.write_text(json.dumps(data))
    cc.migrate_jev()
    assert rc.get("jev_api_key") == "old-windows-key"
    assert _entrada(cc._main_file())["env"]["JEV_MODEL"] == "jev-latest"
    rc.aplicar({"jev_api_key": "page-key"})
    cc.migrate_jev()
    assert rc.get("jev_api_key") == "page-key"
    assert _entrada(cc._main_file())["env"]["TYPESAFE_API_KEY"] == "page-key"


def test_migrate_reads_legacy_settings_key(home):
    from app import runtime_config as rc

    settings = home / ".claude" / "settings.json"
    settings.parent.mkdir()
    settings.write_text(json.dumps({"env": {"TYPESAFE_API_KEY": "legacy-typesafe-key"}}))
    cc.migrate_jev()
    assert rc.get("jev_api_key") == "legacy-typesafe-key"
    assert not cc._parked_file().exists()   # sem MCP configurado, nada é criado


def test_key_removed_on_the_jev_page_does_not_come_back_on_restart(home):
    from app import runtime_config as rc

    settings = home / ".claude" / "settings.json"
    settings.parent.mkdir()
    settings.write_text(json.dumps({"env": {"TYPESAFE_API_KEY": "legacy-typesafe-key"}}))
    cc.save(_pedido(home))
    cc.migrate_jev()
    rc.aplicar({}, remover={"jev_api_key"})
    cc.migrate_jev()
    assert not rc.get("jev_api_key")
    assert "TYPESAFE_API_KEY" not in _entrada(cc._main_file())["env"]


@pytest.mark.parametrize("relative,content", [
    (".claude.json", "{invalid-secret"),
    (".claude.json", '{"mcpServers": "invalid-secret"}'),
    (".hangar/computer-control.json", "[\"invalid-secret\"]"),
])
def test_invalid_computer_use_config_keeps_jev_settings_saved(home, relative, content):
    from app import runtime_config as rc

    path = home / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    assert "upload_retention_days" in rc.estado()
    with pytest.raises(ValueError, match="Computer Use"):
        rc.aplicar({"jev_api_key": "replacement-secret"})
    assert rc.get("jev_api_key") == "replacement-secret"
    assert path.read_text() == content


@pytest.fixture
def rust_release(monkeypatch):
    assets = {"hangar-computer-control-linux-x86_64": b"linux-binary", "windows-agent.exe": b"MZ-agent"}

    def get(url, timeout):
        if url == f"https://api.github.com/repos/{cc.REPO}/releases/latest":
            return json.dumps({"tag_name": "v2.0.0", "assets": [{"name": name} for name in assets]}).encode()
        prefix = f"https://github.com/{cc.REPO}/releases/download/v2.0.0/"
        assert url.startswith(prefix)
        return assets[url.removeprefix(prefix)]

    monkeypatch.setattr(cc, "_get", get)
    return assets


def test_install_downloads_both_assets(home, rust_release):
    result = cc.install()
    directory = home / ".hangar" / "computer-control"
    binary = directory / "hangar-computer-control"
    assert binary.read_bytes() == b"linux-binary"
    assert binary.stat().st_mode & 0o777 == 0o755
    assert (directory / "windows-agent.exe").read_bytes() == b"MZ-agent"
    assert json.loads((directory / "install.json").read_text()) == {"tag": "v2.0.0"}
    assert result["installed_tag"] == "v2.0.0" and result["package_exists"]


def test_package_entry_is_binary_without_args(home, rust_release):
    cc.save(_pedido(home))
    data = json.loads(cc._main_file().read_text())
    data["mcpServers"][cc.NAME]["env"].update(PYTHONPATH="old-pythonpath", VIRTUAL_ENV="")
    cc._main_file().write_text(json.dumps(data))
    cc.install()
    entry = _entrada(home / ".claude-x" / ".claude.json")
    assert entry == _entrada(cc._main_file())
    assert entry["command"] == str(home / ".hangar" / "computer-control" / "hangar-computer-control")
    assert entry["args"] == []
    assert "PYTHONPATH" not in entry["env"] and entry["env"]["VIRTUAL_ENV"] == ""
    assert entry["env"]["LLM_MODEL"] == "m1"


def test_mode_detects_installed_binary_as_package(home):
    command = home / ".hangar" / "computer-control" / "hangar-computer-control"
    cc._main_file().write_text(json.dumps({"mcpServers": {cc.NAME: {"command": str(command), "args": []}}}))
    assert cc.state()["mode"] == "package"


def test_mode_detects_target_release_as_local(home):
    project = home / "another-checkout"
    (project / "target" / "release").mkdir(parents=True)
    binary = project / "target" / "release" / "hangar-computer-control"
    binary.write_bytes(b"local-binary")
    (project / "custom-agent.json").write_text("{}")
    cc._main_file().write_text(json.dumps({"mcpServers": {cc.NAME: {"command": str(binary), "args": []}}}))
    result = cc.state()
    assert result["mode"] == "local" and result["project_dir"] == str(project)
    assert result["agent_configs"] == [str(project / "custom-agent.json")]


def test_local_mode_requires_built_binary(home):
    binary = home / "Projetos" / cc.NAME / "target" / "release" / cc.NAME
    binary.unlink()
    with pytest.raises(cc.ComputerControlError) as error:
        cc.save(_pedido(home, mode="local"))
    project = home / "Projetos" / cc.NAME
    assert error.value.code == "erro_computer_control_dir"
    assert error.value.msg == f"{project} não tem target/release/hangar-computer-control (rode cargo build --release)"
    assert _entrada(cc._main_file()) is None


def test_local_mode_uses_only_binary_and_installed_windows_agent(home):
    project = home / "Projetos" / cc.NAME
    (project / "servidor_mcp.py").unlink()
    (project / ".venv" / "bin" / "python").unlink()
    cc.save(_pedido(home, mode="local"))
    entry = _entrada(cc._main_file())
    assert entry["command"] == str(project / "target" / "release" / "hangar-computer-control")
    assert entry["args"] == [] and "PYTHONPATH" not in entry["env"]
    result = cc.create_target({"name": "vm-new", "transport": "ssh", "host": "vm-new"})
    target = json.loads((project / "vm-new-agent.json").read_text())
    assert target["agent_path"] == str(home / ".hangar" / "computer-control" / "windows-agent.exe")
    assert result["agent_exe"]["path"] == target["agent_path"]


def test_legacy_uvx_entry_still_package_and_rewritten_on_save(home):
    directory = home / ".hangar" / "computer-control"
    directory.mkdir(parents=True)
    (directory / "windows-agent.exe").write_bytes(b"MZ-agent")
    (directory / "hangar-computer-control").write_bytes(b"binary")
    (directory / "install.json").write_text('{"tag":"v2.0.0"}')
    cc._main_file().write_text(json.dumps({"mcpServers": {cc.NAME: {
        "command": "/bin/uvx", "args": ["--from", "legacy-package", cc.NAME], "env": {"PYTHONPATH": "old"}}}}))
    assert cc.state()["mode"] == "package"
    cc.save(_pedido(home, mode="package"))
    entry = _entrada(cc._main_file())
    assert entry["command"] == str(directory / "hangar-computer-control") and entry["args"] == []
    assert "PYTHONPATH" not in entry["env"]


def test_save_keeps_uvx_entry_when_binary_missing(home):
    entry = {"command": "/custom/uvx", "args": ["--from", "working-package", cc.NAME],
             "env": {"LLM_MODEL": "old"}}
    cc._main_file().write_text(json.dumps({"mcpServers": {cc.NAME: entry}}))
    cc.save(_pedido(home, mode="package"))
    saved = _entrada(cc._main_file())
    assert saved["command"] == entry["command"] and saved["args"] == entry["args"]
    assert saved["env"]["LLM_MODEL"] == "m1"


def test_no_uvx_requirement(home, monkeypatch, rust_release):
    monkeypatch.setattr(cc.shutil, "which", lambda name: None)
    result = cc.install()
    assert result["package_exists"] and not result["legacy_package"]


def test_install_migrates_existing_linux_targets_only_for_binary(home, rust_release):
    targets = home / ".hangar" / "computer-control" / "targets"
    targets.mkdir(parents=True)
    legacy = {"transport": "local", "command": ["python", "/old/linux_agent.py"], "request_timeout": 41}
    (targets / "linux-agent.json").write_text(json.dumps(legacy))
    custom = {"transport": "local", "command": ["/custom/agent", "arg"]}
    (targets / "custom-agent.json").write_text(json.dumps(custom))
    cc.install()
    assert json.loads((targets / "linux-agent.json").read_text()) == {
        "transport": "local", "command": [str(targets.parent / "hangar-computer-control"), "agent"],
        "request_timeout": 41}
    assert json.loads((targets / "custom-agent.json").read_text()) == custom


def test_install_migrates_sh_wrapped_linux_target(home, rust_release):
    targets = home / ".hangar" / "computer-control" / "targets"
    targets.mkdir(parents=True)
    prefix = "HYPRLAND_INSTANCE_SIGNATURE=${HYPRLAND_INSTANCE_SIGNATURE:-$(ls -t /run/user/1000/hypr | head -1)} "
    legacy = {"transport": "local", "command": [
        "/bin/sh", "-c", prefix + 'exec /usr/bin/python3 /old/hangar-computer-control/linux_agent.py "$@"', "sh"]}
    (targets / "linux-agent.json").write_text(json.dumps(legacy))
    cc.install()
    command = json.loads((targets / "linux-agent.json").read_text())["command"]
    binary = targets.parent / "hangar-computer-control"
    assert command == ["/bin/sh", "-c", prefix + f'exec {binary} agent "$@"', "sh"]


def test_install_exposes_invalid_target_without_overwriting_it(home, rust_release):
    targets = home / ".hangar" / "computer-control" / "targets"
    targets.mkdir(parents=True)
    invalid = targets / "broken-agent.json"
    invalid.write_text("not-json")
    result = cc.install()
    assert "broken-agent.json" in result["migration_skipped"]
    assert invalid.read_text() == "not-json"


def test_binary_install_download_failure_keeps_previous_install(home, monkeypatch):
    directory = home / ".hangar" / "computer-control"
    directory.mkdir(parents=True)
    (directory / "hangar-computer-control").write_bytes(b"old-linux")
    (directory / "windows-agent.exe").write_bytes(b"old-windows")
    (directory / "install.json").write_text('{"tag":"v1.0.0"}')

    def get(url, timeout):
        if "api.github.com" in url:
            return b'{"tag_name":"v2.0.0","assets":[{"name":"hangar-computer-control-linux-x86_64"}]}'
        if url.endswith("windows-agent.exe"):
            raise cc.ComputerControlError(502, "erro_computer_control_release", "download recusado")
        return b"new-linux"

    monkeypatch.setattr(cc, "_get", get)
    with pytest.raises(cc.ComputerControlError, match="download recusado"):
        cc.install()
    assert (directory / "hangar-computer-control").read_bytes() == b"old-linux"
    assert (directory / "windows-agent.exe").read_bytes() == b"old-windows"
    assert json.loads((directory / "install.json").read_text()) == {"tag": "v1.0.0"}


def test_package_entry_on_windows_uses_windows_binary(home, monkeypatch, rust_release):
    monkeypatch.setattr(cc, "os", SimpleNamespace(**{**vars(cc.os), "name": "nt"}))
    cc.install()
    cc.save(_pedido(home, mode="package"))
    entry = _entrada(cc._main_file())
    assert entry["command"] == str(home / ".hangar" / "computer-control" / "windows-agent.exe")
    assert entry["args"] == [] and cc.state()["mode"] == "package"


@pytest.mark.parametrize("platform,stale_binary", [("posix", False), ("posix", True), ("nt", False)])
def test_legacy_install_updates_uvx_tag_even_with_old_binary(home, monkeypatch, platform, stale_binary):
    monkeypatch.setattr(cc, "os", SimpleNamespace(**{**vars(cc.os), "name": platform}))
    directory = home / ".hangar" / "computer-control"
    directory.mkdir(parents=True)
    if stale_binary:
        (directory / "hangar-computer-control").write_bytes(b"old-rust")
    targets = directory / "targets"
    targets.mkdir()
    target = targets / "existing-agent.json"
    target.write_text(json.dumps({"transport": "ssh", "host": "vm-old", "agent_path": str(directory / "windows-agent.exe")}))
    entry = {"command": "/bin/uvx", "args": ["--from", f"git+https://github.com/{cc.REPO}@v1.0.0", cc.NAME],
             "env": {"HCC_AGENT_CONFIG": str(target)}}
    cc._main_file().write_text(json.dumps({"mcpServers": {cc.NAME: entry}}))
    monkeypatch.setattr(cc.shutil, "which", lambda name: "/bin/uvx" if name == "uvx" else None)
    monkeypatch.setattr(cc, "_get", lambda url, timeout: (
        b'{"tag_name":"v2.0.0","assets":[{"name":"windows-agent.exe"}]}' if "api.github.com" in url else b"MZ-legacy"))
    result = cc.install()
    saved = _entrada(cc._main_file())
    assert saved["command"] == "/bin/uvx"
    assert saved["args"] == ["--from", f"git+https://github.com/{cc.REPO}@v2.0.0", cc.NAME]
    assert result["legacy_package"] and result["installed_tag"] == "v2.0.0"
    cc.save(_pedido(home, mode="package"))
    assert _entrada(cc._main_file())["command"] == "/bin/uvx"


def test_install_preserves_custom_linux_script_argument(home, rust_release):
    targets = home / ".hangar" / "computer-control" / "targets"
    targets.mkdir(parents=True)
    custom = {"transport": "local", "command": ["/custom/agent", "linux_agent.py"]}
    (targets / "custom-agent.json").write_text(json.dumps(custom))
    cc.install()
    assert json.loads((targets / "custom-agent.json").read_text()) == custom


def test_local_save_resolves_relative_project_directory(home, monkeypatch):
    monkeypatch.chdir(home)
    cc.save(_pedido(home, mode="local", project_dir=f"Projetos/{cc.NAME}"))
    entry = _entrada(cc._main_file())
    assert entry["command"] == str(home / "Projetos" / cc.NAME / "target" / "release" / cc.NAME)
    assert entry["env"]["HCC_AGENTS_DIR"] == str(home / "Projetos" / cc.NAME)


@pytest.mark.parametrize("binary_release", [False, True])
def test_install_flow_with_real_http_and_config_files(home, monkeypatch, binary_release):
    assets = {"windows-agent.exe": b"MZ-agent"}
    if binary_release:
        assets["hangar-computer-control-linux-x86_64"] = b"linux-binary"
    payloads = {f"/repos/{cc.REPO}/releases/latest": json.dumps({
        "tag_name": "v3.0.0", "assets": [{"name": name} for name in assets]}).encode()}
    payloads.update({f"/{cc.REPO}/releases/download/v3.0.0/{name}": content for name, content in assets.items()})

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = payloads.get(self.path)
            self.send_response(200 if body is not None else 404)
            self.end_headers()
            self.wfile.write(body or b"missing asset")

        def log_message(self, format, *args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        get = cc._get
        base = f"http://127.0.0.1:{server.server_port}"
        monkeypatch.setattr(cc, "_get", lambda url, timeout: get(base + urlsplit(url).path, timeout))
        monkeypatch.setattr(cc.shutil, "which", lambda name: "/bin/uvx" if name == "uvx" else None)
        try:
            result = cc.install()
            directory = home / ".hangar" / "computer-control"
            cc.create_target({"name": "vm-http", "transport": "ssh", "host": "vm-http"})
            target = directory / "targets" / "vm-http-agent.json"
            cc.save(_pedido(home, mode="package", agent_config=str(target)))
            assert json.loads(target.read_text())["agent_path"] == str(directory / "windows-agent.exe")
            entry = _entrada(cc._main_file())
            assert entry == _entrada(home / ".claude-x" / ".claude.json")
            assert entry["command"] == (str(directory / "hangar-computer-control") if binary_release else "/bin/uvx")
            assert entry["args"] == ([] if binary_release else [
                "--from", f"git+https://github.com/{cc.REPO}@v3.0.0", cc.NAME])
            assert json.loads((directory / "install.json").read_text()) == {
                "tag": "v3.0.0", **({} if binary_release else {"uvx": "/bin/uvx"})}
            assert "PYTHONPATH" not in entry["env"] and result["legacy_package"] is not binary_release
        finally:
            server.shutdown()
            worker.join(timeout=5)


def test_linux_local_target_runs_the_binary_as_agent(home, monkeypatch):
    projeto = home / "Projetos" / cc.NAME
    monkeypatch.setattr(cc.os, "name", "posix")
    binary = projeto / "target" / "release" / cc.NAME
    cc.create_target({"project_dir": str(projeto), "name": "este", "transport": "local"})
    cfg = json.loads((projeto / "este-agent.json").read_text())
    assert cfg["command"] == [str(binary), "agent"]
    assert next(t for t in cc.state()["targets"] if t["name"] == "este")["os"] == "linux"
