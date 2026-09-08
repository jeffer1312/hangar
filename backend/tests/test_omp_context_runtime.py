"""Política exercitada pelo CLI e pelo prompt real, sem chamada a modelo."""
import json
import os
from pathlib import Path
import subprocess

import pytest

from app.omp_context import configure_claude_context, DISABLED_CONTEXT_IDS, RULE_TEMPLATE
from tests.omp_runtime import run_omp_driver

pytestmark = pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="OMP real exige sandbox")
DRIVER = Path(__file__).resolve().parents[2] / "scripts/tests/omp-context-driver.ts"


def parsed_config(path):
    code = "process.stdout.write(JSON.stringify(Bun.YAML.parse(await Bun.file(process.argv[1]).text())))"
    result = subprocess.run(["bun", "-e", code, str(path)], capture_output=True, text=True, check=True, timeout=10)
    return json.loads(result.stdout)


@pytest.mark.parametrize("project_has_claude,rule_case", [
    (True, "normal"), (False, "normal"), (True, "disabled"), (True, "agents"),
    (True, "id-blocked"), (True, "ttsr-blocked"), (True, "provider-blocked"),
    (True, "nested"), (True, "mdc"),
])
def test_omp_carrega_claude_e_nao_agents_antes_de_ferramentas(tmp_path, monkeypatch, project_has_claude, rule_case):
    home = tmp_path / "home"
    agent = home / ".omp/agent"
    claude = home / ".claude"
    project = home / "project"
    for directory in (agent, claude, project):
        directory.mkdir(parents=True, exist_ok=True)
    private_bin = home / "bin"
    private_bin.mkdir()
    (private_bin / "omp").symlink_to(os.environ["OMP_TEST_BIN"])
    for key in ("OMP_PROFILE", "PI_PROFILE", "PI_CONFIG_DIR"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(claude))
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(agent))
    monkeypatch.setenv("PATH", str(private_bin) + os.pathsep + os.environ["PATH"])
    config = agent / "config.yml"
    initial = {"setupVersion": 2, "theme": {"dark": "titanium"}, "display": {"showTokenUsage": False},
               "disabledExtensions": ["extension-module:personal"]}
    config.write_text(json.dumps(initial), encoding="utf-8")
    (claude / "CLAUDE.md").write_text("GLOBAL_CLAUDE_SENTINEL\n", encoding="utf-8")
    (agent / "AGENTS.md").write_text("FORBIDDEN_AGENTS_SENTINEL\n", encoding="utf-8")
    (project / "AGENTS.md").write_text("FORBIDDEN_AGENTS_SENTINEL\n", encoding="utf-8")
    if project_has_claude:
        (project / "CLAUDE.md").write_text("PROJECT_CLAUDE_SENTINEL\n", encoding="utf-8")
    if rule_case in {"disabled", "agents", "nested", "mdc"}:
        target = agent / "rules" / RULE_TEMPLATE.name
        if rule_case == "nested":
            target = agent / "rules/nested/policy.md"
        elif rule_case == "mdc":
            target = agent / "rules/existente.mdc"
        target.parent.mkdir(parents=True, exist_ok=True)
        text = RULE_TEMPLATE.read_text()
        if rule_case == "disabled":
            text = text.replace("alwaysApply: true", "alwaysApply: true\nenabled: false")
        elif rule_case == "agents":
            text = text.replace("alwaysApply: true", "alwaysApply: true\nagents: [reviewer]")
        target.write_text(text, encoding="utf-8")
    elif rule_case == "id-blocked":
        initial["disabledExtensions"].append("rule:" + RULE_TEMPLATE.stem)
    elif rule_case == "ttsr-blocked":
        initial["ttsr"] = {"disabledRules": [RULE_TEMPLATE.stem]}
    elif rule_case == "provider-blocked":
        initial["disabledProviders"] = ["native"]
    config.write_text(json.dumps(initial), encoding="utf-8")
    before_conflict = config.read_bytes()
    result = configure_claude_context(home=home, claude_dir=claude, enabled=True)
    if rule_case in {"disabled", "agents", "id-blocked", "ttsr-blocked", "provider-blocked"}:
        assert result["errors"]
        assert config.read_bytes() == before_conflict
        assert not (agent / "APPEND_SYSTEM.md").exists()
        return
    assert result["errors"] == [], result
    configured = parsed_config(config)
    assert {k: v for k, v in configured.items() if k != "disabledExtensions"} == {k: v for k, v in initial.items() if k != "disabledExtensions"}
    assert set(configured["disabledExtensions"]) == set(DISABLED_CONTEXT_IDS) | {"extension-module:personal"}
    before = config.read_bytes(), config.stat().st_mtime_ns
    assert configure_claude_context(home=home, claude_dir=claude, enabled=True)["errors"] == []
    assert (config.read_bytes(), config.stat().st_mtime_ns) == before
    run_omp_driver(DRIVER, home, {"EXPECT_PROJECT": "1" if project_has_claude else "0"}, cwd=project, load_rules=True)
    assert (project / "AGENTS.md").read_text() == "FORBIDDEN_AGENTS_SENTINEL\n"
