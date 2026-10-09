"""Tela do hangar-computer-control: grava em todos os .claude.json, desligar tira de todos e religar
volta com a configuração de antes."""
import json
from types import SimpleNamespace

import pytest

from app import computer_control as cc


@pytest.fixture
def home(tmp_path, monkeypatch):
    monkeypatch.setattr(cc.Path, "home", lambda: tmp_path)
    conta = tmp_path / ".claude-x"
    conta.mkdir()
    (conta / ".claude.json").write_text(json.dumps({"oauthAccount": {"a": 1}}), encoding="utf-8")
    monkeypatch.setattr(cc, "list_config_dirs", lambda: [SimpleNamespace(path=str(conta))])
    projeto = tmp_path / "Projetos" / cc.NAME
    (projeto / ".venv" / "bin").mkdir(parents=True)
    (projeto / ".venv" / "bin" / "python").write_text("")
    (projeto / "servidor_mcp.py").write_text("")
    (projeto / "alvo-agent.json").write_text("{}")
    (tmp_path / ".claude.json").write_text("{}", encoding="utf-8")
    return tmp_path


def _pedido(home, **extra):
    projeto = home / "Projetos" / cc.NAME
    return {"enabled": True, "project_dir": str(projeto), "agent_config": str(projeto / "alvo-agent.json"),
            "llm_url": "http://x/v1/chat/completions", "llm_model": "m1", "llm_effort": "high",
            "llm_key": "chave-do-llm-123", "jev_key": "chave-do-jev-456", **extra}


def _entrada(p):
    return (json.loads(p.read_text()).get("mcpServers") or {}).get(cc.NAME)


def test_liga_desliga_e_religa_sem_perder_a_configuracao(home):
    principal, conta = home / ".claude.json", home / ".claude-x" / ".claude.json"
    cc.save(_pedido(home))
    assert _entrada(principal) == _entrada(conta)
    assert _entrada(principal)["env"]["LLM_MODEL"] == "m1"
    assert json.loads(conta.read_text())["oauthAccount"] == {"a": 1}   # resto do arquivo intocado

    estado = cc.save({"enabled": False})
    assert _entrada(principal) is None and _entrada(conta) is None
    assert estado["enabled"] is False and estado["llm_model"] == "m1"   # a tela não perde os campos

    cc.save(_pedido(home, llm_key=None, jev_key=None))   # chave vazia = mantém a guardada
    env = _entrada(conta)["env"]
    assert env["LLM_PROXY_KEY"] == "chave-do-llm-123" and env["TYPESAFE_API_KEY"] == "chave-do-jev-456"


def test_estado_nunca_devolve_a_chave_inteira(home):
    cc.save(_pedido(home))
    texto = json.dumps(cc.state())
    assert "chave-do-llm-123" not in texto and "chave-do-jev-456" not in texto


def test_cria_alvo_ssh_e_recusa_repetido_e_local_fora_do_windows(home, monkeypatch):
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
    with pytest.raises(cc.ComputerControlError) as e:
        cc.create_target({"project_dir": projeto, "name": "eu", "transport": "local"})
    assert e.value.code == "erro_computer_control_local_only_windows"


def test_instalar_versao_publicada_leva_alvos_e_troca_pra_uvx(home, monkeypatch):
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
        rc.aplicar({"jev_windows_api_key": "windows-secret-1234"})
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


def test_windows_key_updates_only_key_in_active_and_parked_entries(home, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    cc.save(_pedido(home))
    cc.save({"enabled": False})
    cc.save(_pedido(home))
    before = {path: json.loads(path.read_text()) for path in [*cc._config_files(), cc._parked_file()]}
    rc.aplicar({"jev_windows_api_key": "new-windows-key-7890", "jev_api_key": "openrouter-secret"})
    for path, original in before.items():
        entry = original if path == cc._parked_file() else original["mcpServers"][cc.NAME]
        entry["env"]["TYPESAFE_API_KEY"] = "new-windows-key-7890"
        assert json.loads(path.read_text()) == original
    assert "jev_windows_api_key" not in json.loads(rc._caminho().read_text())
    state = rc.estado()["jev_windows_api_key"]
    assert state["definido"] and state["valor"] != "new-windows-key-7890"
    for value in ("", "  ", state["valor"]):
        rc.aplicar({"jev_windows_api_key": value})
        assert cc.jev_key() == "new-windows-key-7890"
    cc.save({"enabled": False})
    rc.aplicar({"jev_windows_api_key": "parked-windows-key"})
    assert not cc.state()["enabled"] and cc.jev_key() == "parked-windows-key"
    assert "parked-windows-key" not in json.dumps(rc.estado())


def test_windows_key_carries_the_destination_of_its_provider(home, monkeypatch):
    """Chave do OpenRouter no Windows leva o endereço e o modelo de lá; voltar para a TypeSafe tira os dois.
    Antes, a chave do OpenRouter ia para a TypeSafe e voltava 401."""
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    cc.save(_pedido(home))
    rc.aplicar({"jev_windows_api_key": "sk-or-windows"})
    env = cc._known_entry()["env"]
    assert (env["JEV_ENDPOINT"], env["JEV_MODEL"]) == (rc.JEV_OPENROUTER_URL, rc.JEV_OPENROUTER_MODELO)
    # A mesma chave da tela geral leva o modelo escolhido lá, com o til do apelido.
    rc.aplicar({"jev_api_key": "sk-or-geral", "jev_endpoint": rc.JEV_OPENROUTER_URL, "jev_model": "typesafe/jev-latest"})
    rc.aplicar({"jev_windows_api_key": "sk-or-geral"})
    assert cc._known_entry()["env"]["JEV_MODEL"] == "~typesafe/jev-latest"
    rc.aplicar({"jev_windows_api_key": "apik-typesafe"})
    env = cc._known_entry()["env"]
    assert env["TYPESAFE_API_KEY"] == "apik-typesafe" and "JEV_ENDPOINT" not in env and "JEV_MODEL" not in env


def test_same_key_option_copies_the_general_key_to_windows(home, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    cc.save(_pedido(home))
    cc.save_jev_key("")
    # Sem chave no Windows e sem escolha: nada é copiado sem pedido.
    rc.aplicar({"jev_api_key": "sk-or-geral"})
    assert cc.jev_key() == "" and rc.estado()["jev_windows_mesma_chave"]["valor"] is False
    # Ligar a opção copia a geral, com o destino dela.
    rc.aplicar({"jev_windows_mesma_chave": True})
    assert cc.jev_key() == "sk-or-geral" and rc.estado()["jev_windows_mesma_chave"]["valor"] is True
    assert cc._known_entry()["env"]["JEV_ENDPOINT"] == rc.JEV_OPENROUTER_URL
    # Trocar a chave geral leva a nova junto: era a mesma.
    rc.aplicar({"jev_api_key": "sk-or-nova"})
    assert cc.jev_key() == "sk-or-nova"
    # Desligado, a chave do Windows fica como está.
    rc.aplicar({"jev_windows_mesma_chave": False})
    rc.aplicar({"jev_windows_api_key": "apik-windows"})
    rc.aplicar({"jev_api_key": "sk-or-outra"})
    assert cc.jev_key() == "apik-windows" and rc.estado()["jev_windows_mesma_chave"]["valor"] is False
    # Religar copia a geral de novo.
    rc.aplicar({"jev_windows_mesma_chave": True})
    assert cc.jev_key() == "sk-or-outra"


def test_existing_separate_windows_key_is_kept_without_a_choice(home, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    cc.save(_pedido(home))
    cc.save_jev_key("apik-windows")
    rc.aplicar({"jev_api_key": "sk-or-geral"})
    assert cc.jev_key() == "apik-windows", "quem já tinha outra chave no Windows continua com ela"
    assert rc.estado()["jev_windows_mesma_chave"]["valor"] is False


def test_windows_key_reads_legacy_settings_without_copying_global_key(home, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    rc.aplicar({"jev_api_key": "openrouter-secret"})
    assert not rc.estado()["jev_windows_api_key"]["definido"]
    settings = home / ".claude" / "settings.json"
    settings.parent.mkdir()
    settings.write_text(json.dumps({"env": {"TYPESAFE_API_KEY": "legacy-typesafe-key"}}))
    assert cc.jev_key() == "legacy-typesafe-key"
    rc.aplicar({"jev_windows_api_key": rc.estado()["jev_windows_api_key"]["valor"]})
    assert not cc._parked_file().exists()


@pytest.mark.parametrize("relative,content", [
    (".claude.json", "{invalid-secret"),
    (".claude.json", '{"mcpServers": "invalid-secret"}'),
    (".hangar/computer-control.json", "[\"invalid-secret\"]"),
    (".claude/settings.json", "{invalid-secret"),
])
def test_invalid_windows_config_keeps_other_settings_available(home, monkeypatch, relative, content):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    path = home / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    state = rc.estado()
    assert "upload_retention_days" in state
    assert state["jev_windows_api_key"]["erro"]
    assert "invalid-secret" not in json.dumps(state)
    with pytest.raises(ValueError, match="jev_windows_api_key"):
        rc.aplicar({"jev_windows_api_key": "replacement-secret"})
    assert path.read_text() == content


def test_invalid_windows_key_type_does_not_write_other_changes(home, monkeypatch):
    from app import runtime_config as rc

    monkeypatch.setattr(rc, "_backend_config_base", lambda: home)
    with pytest.raises(ValueError, match="jev_windows_api_key: esperado texto"):
        rc.aplicar({"jev_windows_api_key": True, "upload_retention_days": 7})
    assert not cc._parked_file().exists() and not rc._caminho().exists()
