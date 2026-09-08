"""Configuração de contexto sem sobrescrever escolhas pessoais."""
import json
from pathlib import Path
import subprocess

import pytest

from app.omp_context import configure_claude_context, RULE_TEMPLATE, DISABLED_CONTEXT_IDS
from tests.test_omp_plugin_sync import write_json, read_json, tree_snapshot


class ConfigCLI:
    def __init__(self, agent):
        self.path = agent / "config.yml"
        self.calls = []

    def __call__(self, args, **kwargs):
        self.calls.append(args)
        data = read_json(self.path) if self.path.exists() else {}
        assert args[1:3] in (["config", "get"], ["config", "set"])
        if args[2] == "set":
            data[args[3]] = json.loads(args[4])
            write_json(self.path, data)
        value = data
        for key in args[3].split("."):
            value = value.get(key, []) if isinstance(value, dict) else []
        return subprocess.CompletedProcess(args, 0, json.dumps({"key": args[3], "value": value}), "")


@pytest.fixture
def context_home(tmp_path, monkeypatch):
    home = tmp_path / "home"
    agent = home / ".omp/agent"
    agent.mkdir(parents=True)
    claude = home / ".claude"
    claude.mkdir()
    (claude / "CLAUDE.md").write_text("Contexto global pessoal.\n", encoding="utf-8")
    for key in ("OMP_PROFILE", "PI_PROFILE", "PI_CONFIG_DIR"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(agent))
    write_json(agent / "config.yml", {"theme": "titanium", "custom": {"keep": [1, False]},
                                      "disabledExtensions": ["extension-module:personal"]})
    return home, agent, claude, ConfigCLI(agent)


def configure(context_home, **kwargs):
    home, _, claude, cli = context_home
    return configure_claude_context(home=home, claude_dir=claude, runner=cli, **kwargs)


def test_desligado_nao_le_cli_nem_escreve(context_home):
    home, _, _, cli = context_home
    before = tree_snapshot(home)
    assert configure(context_home, enabled=False)["enabled"] is False
    assert cli.calls == []
    assert tree_snapshot(home) == before


def test_configura_apenas_ids_necessarios_e_repeticao_e_idempotente(context_home):
    home, agent, claude, cli = context_home
    before = read_json(cli.path)
    result = configure(context_home, enabled=True)
    assert result["errors"] == []
    configured = read_json(cli.path)
    assert set(configured["disabledExtensions"]) == set(DISABLED_CONTEXT_IDS) | {"extension-module:personal"}
    assert {k: v for k, v in configured.items() if k != "disabledExtensions"} == {k: v for k, v in before.items() if k != "disabledExtensions"}
    assert (agent / "APPEND_SYSTEM.md").samefile(claude / "CLAUDE.md")
    assert (agent / "rules" / RULE_TEMPLATE.name).samefile(RULE_TEMPLATE)
    # Três leituras antes do set e uma releitura depois: cada `config get` é um processo na subida do backend.
    assert sum(1 for args in cli.calls if args[2] == "get") == 4
    snapshot = tree_snapshot(home)
    cli.calls.clear()
    assert configure(context_home, enabled=True)["errors"] == []
    assert not any(args[2] == "set" for args in cli.calls)
    assert tree_snapshot(home) == snapshot


@pytest.mark.parametrize("kind", ["file", "external-link"])
def test_append_personalizado_e_preservado_sem_mudanca_parcial(context_home, kind):
    home, agent, _, cli = context_home
    target = agent / "APPEND_SYSTEM.md"
    if kind == "file":
        target.write_text("Escolha pessoal", encoding="utf-8")
    else:
        other = home / "other.md"
        other.write_text("Outro contexto", encoding="utf-8")
        target.symlink_to(other)
    before = tree_snapshot(home)
    result = configure(context_home, enabled=True)
    assert result["errors"]
    assert tree_snapshot(home) == before
    assert cli.calls == []


def test_reutiliza_regra_equivalente_sem_criar_duplicata(context_home):
    _, agent, _, _ = context_home
    rules = agent / "rules"
    rules.mkdir()
    existing = rules / "regra-pessoal.md"
    existing.write_text(RULE_TEMPLATE.read_text(), encoding="utf-8")
    assert configure(context_home, enabled=True)["errors"] == []
    assert list(rules.iterdir()) == [existing]
    assert not existing.is_symlink()


def test_regra_canonica_personalizada_nao_e_sobrescrita(context_home):
    home, agent, _, cli = context_home
    rule = agent / "rules" / RULE_TEMPLATE.name
    rule.parent.mkdir()
    rule.write_text("Instruções pessoais diferentes", encoding="utf-8")
    before = tree_snapshot(home)
    assert configure(context_home, enabled=True)["errors"]
    assert tree_snapshot(home) == before
    assert cli.calls == []


def test_global_ausente_nao_cria_link_quebrado(context_home):
    _, agent, claude, _ = context_home
    (claude / "CLAUDE.md").unlink()
    result = configure(context_home, enabled=True)
    assert result["errors"] == []
    assert not (agent / "APPEND_SYSTEM.md").is_symlink()
    assert result["global_context"] == "missing"


def test_link_trocado_durante_configuracao_e_preservado_e_reportado(context_home):
    home, agent, claude, cli = context_home
    other = home / "external.md"
    other.write_text("Contexto externo", encoding="utf-8")
    original = cli.__call__
    def changed_link(args, **kwargs):
        output = original(args, **kwargs)
        if args[2] == "get" and not (agent / "APPEND_SYSTEM.md").is_symlink():
            (agent / "APPEND_SYSTEM.md").symlink_to(other)
        return output
    result = configure_claude_context(home=home, claude_dir=claude, enabled=True, runner=changed_link)
    assert result["errors"]
    assert (agent / "APPEND_SYSTEM.md").samefile(other)


def test_diretorio_trocado_por_symlink_nao_recebe_regra(context_home):
    home, agent, claude, cli = context_home
    external = home / "external-rules"
    external.mkdir()
    original = cli.__call__
    def change_directory(args, **kwargs):
        output = original(args, **kwargs)
        if args[2] == "get" and not (agent / "rules").is_symlink():
            (agent / "rules").symlink_to(external, target_is_directory=True)
        return output
    result = configure_claude_context(home=home, claude_dir=claude, enabled=True, runner=change_directory)
    assert result["errors"]
    assert list(external.iterdir()) == []
    assert (agent / "rules").samefile(external)


@pytest.mark.parametrize("metadata", ["enabled: false", "agents: [reviewer]", "condition: ['x']", "alwaysApply: false"])
def test_regra_restrita_nao_e_adotada(context_home, metadata):
    _, agent, _, cli = context_home
    rule = agent / "rules" / RULE_TEMPLATE.name
    rule.parent.mkdir()
    content = RULE_TEMPLATE.read_text().replace("alwaysApply: true", "alwaysApply: true\n" + metadata)
    rule.write_text(content, encoding="utf-8")
    before = cli.path.read_bytes()
    result = configure(context_home, enabled=True)
    assert result["errors"]
    assert cli.path.read_bytes() == before
    assert not (agent / "APPEND_SYSTEM.md").exists()
    assert rule.read_text() == content


@pytest.mark.parametrize("control", ["disabledExtensions", "ttsr.disabledRules", "disabledProviders"])
def test_bloqueio_pessoal_da_politica_nao_e_removido(context_home, control):
    _, agent, _, cli = context_home
    config = read_json(cli.path)
    name = RULE_TEMPLATE.stem
    if control == "disabledExtensions":
        config[control].append("rule:" + name)
    elif control == "disabledProviders":
        config[control] = ["native"]
    else:
        config["ttsr"] = {"disabledRules": [name]}
    write_json(cli.path, config)
    before = cli.path.read_bytes()
    result = configure(context_home, enabled=True)
    assert result["errors"]
    assert cli.path.read_bytes() == before
    assert not (agent / "APPEND_SYSTEM.md").exists()
    assert not (agent / "rules" / RULE_TEMPLATE.name).exists()


@pytest.mark.parametrize("metadata", ["enabled: !!bool [true]", "? !!str [extra]\n: value"])
def test_tag_yaml_nao_substitui_tipo_estrutural(context_home, metadata):
    home, agent, _, cli = context_home
    rule = agent / "rules" / RULE_TEMPLATE.name
    rule.parent.mkdir()
    text = RULE_TEMPLATE.read_text().replace("alwaysApply: true", "alwaysApply: true\n" + metadata)
    rule.write_text(text, encoding="utf-8")
    before = tree_snapshot(home)
    result = configure(context_home, enabled=True)
    assert result["errors"]
    assert cli.calls == []
    assert tree_snapshot(home) == before
