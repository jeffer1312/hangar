"""Herança de contas Codex sem copiar identidade nem estado de execução."""

from __future__ import annotations

import copy
import asyncio
import json
import re
import shutil
import os
import subprocess
import tomllib
from pathlib import Path

import pytest

from app import codex_contas as accounts
from app import codex_contas_sync as sync
from app.codex_arquivos import ler
from app.codex_importador import CodexNativo


def _toml_value(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        raise ValueError("None não pode ser escrito no TOML de teste")
    return json.dumps(value, ensure_ascii=False)


def _dump_toml(data: dict) -> bytes:
    lines = []

    def section(values: dict, prefix: tuple[str, ...] = ()):
        scalars = [(key, value) for key, value in values.items() if not isinstance(value, dict)]
        nested = [(key, value) for key, value in values.items() if isinstance(value, dict)]
        for key, value in scalars:
            lines.append(f"{json.dumps(key)} = {_toml_value(value)}")
        for key, value in nested:
            if lines and lines[-1] != "":
                lines.append("")
            name = ".".join(json.dumps(part) for part in (*prefix, key))
            lines.append(f"[{name}]")
            section(value, (*prefix, key))

    section(data)
    return ("\n".join(lines) + ("\n" if lines else "")).encode()


def _key_parts(key_path: str) -> list[str]:
    return [json.loads(part) for part in re.findall(r'"(?:[^"\\]|\\.)*"', key_path)]


def _apply_edit(config: dict, edit: dict) -> None:
    parts = _key_parts(edit["keyPath"])
    atual = config
    for part in parts[:-1]:
        atual = atual.setdefault(part, {})
    if edit["value"] is None:
        atual.pop(parts[-1], None)
    else:
        atual[parts[-1]] = copy.deepcopy(edit["value"])


@pytest.fixture
def isolated(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    source = accounts.default_home()
    destination = tmp_path / ".codex-work"
    source.mkdir()
    destination.mkdir()
    return tmp_path, source, accounts.Account("work", destination, False)


@pytest.fixture
def fake_writer(monkeypatch):
    calls = []

    async def editar_config(path, backups, work_dir, nativo, preparar, **kwargs):
        calls.append(path)
        raw = ler(path)
        atual = tomllib.loads(raw.decode()) if raw else {}
        edits, confirmar = preparar(atual)
        if edits:
            novo = copy.deepcopy(atual)
            for edit in edits:
                _apply_edit(novo, edit)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(_dump_toml(novo))
        confirmar()

    monkeypatch.setattr(sync, "editar_config", editar_config)
    return calls


def _config(path: Path, **values):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(_dump_toml(values))


@pytest.mark.skipif(os.name == "nt", reason="permissão de execução POSIX")
async def test_resource_execution_survives_copy_and_mode_only_changes(isolated, fake_writer):
    _, source, account = isolated
    hook = source / "hooks/probe.sh"
    hook.parent.mkdir()
    hook.write_text("#!/bin/sh\nprintf 'ok'\n")
    hook.chmod(0o755)
    target = account.home / "hooks/probe.sh"

    assert (await sync.prepare_account(account))["status"] == "ready"
    assert subprocess.check_output([str(target)], text=True) == "ok"
    target.chmod(0o600)
    assert (await sync.prepare_account(account))["status"] == "ready"
    assert subprocess.check_output([str(target)], text=True) == "ok"
    hook.chmod(0o644)
    assert (await sync.prepare_account(account))["status"] == "ready"
    assert not target.stat().st_mode & 0o100


@pytest.mark.skipif(os.name == "nt", reason="sonda shell POSIX")
async def test_external_hook_keeps_helper_and_repairs_legacy_copy(isolated, fake_writer):
    root, source, account = isolated
    script = '#!/bin/sh\nbash "$(dirname "$(readlink -f "$0")")/helper.sh"\n'
    originals = []
    for name in ("first", "second"):
        folder = root / name
        folder.mkdir()
        original = folder / "probe.sh"
        original.write_text(script)
        original.chmod(0o755)
        (folder / "helper.sh").write_text(f"printf '{name}'\n")
        originals.append(original)
    hook = source / "hooks/probe.sh"
    hook.parent.mkdir()
    hook.symlink_to(originals[0])
    target = account.home / "hooks/probe.sh"
    target.parent.mkdir()
    target.write_text(script)
    target.chmod(0o600)
    sync._write_state(account, {"public": {"status": "ready"}, "resources": {
        "hooks/probe.sh": {"hash": sync.hash_bytes(script.encode())},
    }})

    assert (await sync.prepare_account(account))["status"] == "ready"
    assert target.is_symlink()
    assert subprocess.check_output([str(target)], text=True) == "first"
    hook.unlink()
    hook.symlink_to(originals[1])
    assert (await sync.prepare_account(account))["status"] == "ready"
    assert subprocess.check_output([str(target)], text=True) == "second"
    hook.unlink()
    hook.write_text(script)
    hook.chmod(0o755)
    assert (await sync.prepare_account(account))["status"] == "ready"
    assert not target.is_symlink()
    assert target.read_text() == script
    assert originals[1].stat().st_mode & 0o777 == 0o755
    hook.unlink()
    hook.symlink_to(originals[1])
    assert (await sync.prepare_account(account))["status"] == "ready"
    hook.unlink()
    assert (await sync.prepare_account(account))["status"] == "ready"
    assert not target.is_symlink()
    assert all(path.read_text() == script for path in originals)


async def test_external_hook_link_failure_is_not_ready(isolated, fake_writer, monkeypatch):
    root, source, account = isolated
    original = root / "probe.sh"
    original.write_text("exit 0\n")
    hook = source / "hooks/probe.sh"
    hook.parent.mkdir()
    hook.symlink_to(original)

    def denied(*args, **kwargs):
        raise PermissionError("symlinks indisponíveis")

    monkeypatch.setattr(Path, "symlink_to", denied)
    result = await sync.prepare_account(account)
    assert result["status"] == "partial"
    assert not (account.home / "hooks/probe.sh").exists()
    assert result["issues"]


async def test_hook_repair_preserves_personal_destination_link(isolated, fake_writer):
    root, source, account = isolated
    original = root / "probe.py"
    original.write_text("print('source')\n")
    personal = root / "personal.py"
    personal.write_text("print('personal')\n")
    hook = source / "hooks/probe.py"
    hook.parent.mkdir()
    hook.symlink_to(original)
    target = account.home / "hooks/probe.py"
    target.parent.mkdir()
    target.symlink_to(personal)

    result = await sync.prepare_account(account)
    assert result["status"] == "partial"
    assert target.resolve() == personal
    assert personal.read_text() == "print('personal')\n"


def test_project_preferences_excludes_identity_and_runtime_state():
    source = {
        "model": "fable",
        "model_reasoning_effort": "high",
        "cli_auth_credentials_store": "keyring",
        "hooks": {"state": {"private-id": {"approved": True}}, "SessionStart": []},
        "projects": {"/private": {"trust_level": "trusted"}},
        "notice": {"migration": True},
        "history": ["private"],
        "sqlite_home": "/private/sqlite",
        "shell_environment_policy": {"set": {
            "OPENAI_API_KEY": "secret",
            "OPENAI_IDENTITY_TOKEN_FILE": "/private/token",
            "TOOL_ENDPOINT": "keep",
        }},
    }

    result = sync.project_preferences(source)

    assert result["model_reasoning_effort"] == "high"
    assert result["cli_auth_credentials_store"] == "file"
    assert result["hooks"] == {"SessionStart": []}
    assert "state" not in result["hooks"]
    assert "projects" not in result and "notice" not in result
    assert "history" not in result and "sqlite_home" not in result
    assert result["shell_environment_policy"]["set"] == {"TOOL_ENDPOINT": "keep"}
    assert source["hooks"]["state"]


def test_project_preferences_reports_provider_choice_without_inheriting_it():
    source = {
        "model": "gpt-5.6-sol",
        "model_provider": "externo",
        "model_providers": {"externo": {"base_url": "https://provider.invalid"}},
        "shell_environment_policy": {"set": {"OPENAI_BASE_URL": "https://other.invalid"}},
    }

    projected, issues = sync._project_preferences(source)

    assert projected["model"] == "gpt-5.6-sol"
    assert projected["model_providers"] == source["model_providers"]
    assert "model_provider" not in projected
    assert "OPENAI_BASE_URL" not in projected["shell_environment_policy"]["set"]
    assert {issue["code"] for issue in issues} == {"codex_account_provider_divergence"}


def test_command_mapping_preserves_windows_quoting_when_path_is_external(tmp_path):
    command = r"python 'C:\Users\jeff\codex\hooks\probe.py'"

    assert sync._map_command(command, tmp_path, tmp_path / "dest", set(), []) == command


async def test_prepare_without_default_config_toml_is_ready(isolated, fake_writer):
    # Maquina que nunca rodou o Codex: `~/.codex` sem config.toml. A primeira conta criada pelo
    # app caia em "origem invalida" por um FileNotFoundError na leitura da conta padrao.
    _, source, account = isolated
    assert not (source / "config.toml").exists()

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result


async def test_prepare_inherits_preferences_and_keeps_account_state(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high", cli_auth_credentials_store="file")
    (source / "agents").mkdir()
    (source / "agents/probe.md").write_text("fonte", encoding="utf-8")
    _config(account.home / "config.toml", model="low", cli_auth_credentials_store="keyring")
    auth = account.home / "auth.json"
    sqlite = account.home / "state.sqlite"
    sessions = account.home / "sessions/keep.jsonl"
    auth.write_bytes(b"auth-secundaria")
    sqlite.write_bytes(b"sqlite-secundario")
    sessions.parent.mkdir()
    sessions.write_bytes(b"sessao")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    config = tomllib.loads((account.home / "config.toml").read_text())
    assert config["model"] == "high"
    assert config["cli_auth_credentials_store"] == "file"
    assert (account.home / "agents/probe.md").read_text() == "fonte"
    assert auth.read_bytes() == b"auth-secundaria"
    assert sqlite.read_bytes() == b"sqlite-secundario"
    assert sessions.read_bytes() == b"sessao"
    assert len(fake_writer) == 1


async def test_destination_change_is_repaired_even_when_source_is_unchanged(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    (source / "skills/probe").mkdir(parents=True)
    (source / "skills/probe/SKILL.md").write_text("fonte", encoding="utf-8")

    await sync.prepare_account(account)
    _config(account.home / "config.toml", model="local")
    (account.home / "skills/probe/SKILL.md").unlink()

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert tomllib.loads((account.home / "config.toml").read_text())["model"] == "high"
    assert (account.home / "skills/probe/SKILL.md").read_text() == "fonte"
    assert len(fake_writer) == 2


async def test_invalid_source_keeps_managed_destination_untouched(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    await sync.prepare_account(account)
    _config(account.home / "config.toml", model="local")
    (source / "config.toml").write_bytes(b"model = [")

    result = await sync.prepare_account(account)

    assert result["status"] == "error"
    assert any(issue["code"] == "codex_account_source_invalid" for issue in result["issues"])
    assert tomllib.loads((account.home / "config.toml").read_text())["model"] == "local"
    assert len(fake_writer) == 1


async def test_restriction_conflict_is_blocking_and_source_removal_keeps_local(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", forced_login_method="chatgpt", forced_chatgpt_workspace_id="ws-1")
    _config(account.home / "config.toml")

    first = await sync.prepare_account(account)
    assert first["status"] == "ready"
    assert tomllib.loads((account.home / "config.toml").read_text())["forced_login_method"] == "chatgpt"

    _config(source / "config.toml")
    second = await sync.prepare_account(account)
    assert second["status"] == "ready"
    assert "forced_login_method" in tomllib.loads((account.home / "config.toml").read_text())

    _config(source / "config.toml", forced_login_method="chatgpt")
    _config(account.home / "config.toml", forced_login_method="api")
    third = await sync.prepare_account(account)
    assert third["status"] == "partial", third
    assert any(issue["code"] == "codex_account_restriction_conflict" for issue in third["issues"])
    assert tomllib.loads((account.home / "config.toml").read_text())["forced_login_method"] == "api"


async def test_removed_hook_definition_preserves_destination_state(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", hooks={"SessionStart": {"enabled": True}})
    _config(account.home / "config.toml", hooks={
        "SessionStart": {"enabled": True},
        "state": {"private": {"approved": True}},
    })

    await sync.prepare_account(account)
    _config(source / "config.toml")
    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    hooks = tomllib.loads((account.home / "config.toml").read_text())["hooks"]
    assert hooks == {"state": {"private": {"approved": True}}}


async def test_removed_preference_preserves_local_change(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high", personality="source")
    _config(account.home / "config.toml", model="low")
    await sync.prepare_account(account)

    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="high", personality="local")
    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert "personality" in tomllib.loads((account.home / "config.toml").read_text())
    assert tomllib.loads((account.home / "config.toml").read_text())["personality"] == "local"


async def test_internal_reference_outside_allowed_resources_is_pending(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model_instructions_file=str(source / "private.md"))
    _config(account.home / "config.toml")

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_unmapped_reference" for issue in result["issues"])
    assert "model_instructions_file" not in tomllib.loads((account.home / "config.toml").read_text())


async def test_concurrent_preparation_reuses_completed_state(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="low")

    results = await asyncio.gather(sync.prepare_account(account), sync.prepare_account(account))

    assert [result["status"] for result in results] == ["ready", "ready"]
    assert len(fake_writer) == 1


async def test_status_is_read_only_and_private_state_is_restricted(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    assert sync.preparation_status(account)["status"] == "idle"
    assert not (account.home.parent / ".hangar").exists()
    await sync.prepare_account(account)
    before = len(fake_writer)

    status = sync.preparation_status(account)

    # Conjunto EXATO, e continua exato: a trava existe pra barrar estado interno vazando no status
    # público. `etapa` e `herdado` entram porque são contrato com a tela — ela mostra em que passo a
    # herança está e quanto a conta recebeu de cada tipo. Campo novo aqui exige decidir se é público.
    assert set(status) == {"status", "trust_pending", "issues", "etapa", "herdado"}
    assert status["status"] == "ready"
    assert len(fake_writer) == before
    state = sync._state_path(account)
    assert state.stat().st_mode & 0o777 == 0o600
    assert state.parent.stat().st_mode & 0o777 == 0o700


async def test_profiles_use_the_same_native_writer(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(source / "fast.config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    _config(account.home / "fast.config.toml", model="low")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert tomllib.loads((account.home / "fast.config.toml").read_text())["model"] == "high"
    assert account.home / "fast.config.toml" in fake_writer


async def test_removed_profile_keeps_local_content_and_removes_managed_file(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(source / "fast.config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    _config(account.home / "fast.config.toml", model="low")
    await sync.prepare_account(account)

    (source / "fast.config.toml").unlink()
    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert not (account.home / "fast.config.toml").exists()


async def test_destination_symlink_ancestor_is_reported_without_writing_outside(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    (source / "agents").mkdir()
    (source / "agents/probe.md").write_text("não sair", encoding="utf-8")
    outside = account.home.parent / "outside"
    outside.mkdir()
    (account.home / "agents").symlink_to(outside, target_is_directory=True)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial"
    assert any(issue["code"] == "codex_account_path_conflict" for issue in result["issues"])
    assert not (outside / "probe.md").exists()


async def test_config_symlink_is_not_replaced(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    outside = account.home.parent / "outside-config.toml"
    outside.write_bytes(b'model = "outside"\n')
    (account.home / "config.toml").symlink_to(outside)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert outside.read_bytes() == b'model = "outside"\n'
    assert (account.home / "config.toml").is_symlink()


async def test_broad_source_symlink_is_rejected_before_enumeration(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    (source / "agents").symlink_to(source, target_is_directory=True)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_source_root_link" for issue in result["issues"])
    assert not (account.home / "agents/config.toml").exists()
    assert not (account.home / "agents/auth.json").exists()


async def test_source_file_symlink_to_auth_is_rejected(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    secret = source / "auth.json"
    secret.write_text("secret", encoding="utf-8")
    (source / "AGENTS.md").symlink_to(secret)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_source_forbidden" for issue in result["issues"])
    assert not (account.home / "AGENTS.md").exists()


async def test_pasta_antiga_de_upload_em_skill_e_ignorada_sem_bloquear(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    skill = source / "skills/orquestrar"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("---\nname: orquestrar\n---\n", encoding="utf-8")
    uploads = skill / ".hangar-uploads"
    uploads.mkdir()
    (uploads / "anexo.txt").write_text("não copiar", encoding="utf-8")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert (account.home / "skills/orquestrar/SKILL.md").is_file()
    assert not (account.home / "skills/orquestrar/.hangar-uploads").exists()
    assert not any(issue["code"] == "codex_account_source_forbidden" for issue in result["issues"])


async def test_source_file_symlink_to_default_config_is_rejected(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    (source / "AGENTS.md").symlink_to(source / "config.toml")

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_source_forbidden" for issue in result["issues"])
    assert not (account.home / "AGENTS.md").exists()


async def test_agent_link_to_broad_directory_is_rejected(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    broad = source.parent / "tmp-broad"
    broad.mkdir()
    (broad / "random.txt").write_text("não copiar", encoding="utf-8")
    (source / "agents").mkdir()
    (source / "agents/broad").symlink_to(broad, target_is_directory=True)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_source_broad_link" for issue in result["issues"])
    assert not (account.home / "agents/broad/random.txt").exists()


async def test_agent_link_with_readme_and_random_files_is_rejected(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    broad = source.parent / "agents-broad"
    broad.mkdir()
    (broad / "README.md").write_text("readme", encoding="utf-8")
    (broad / "random.txt").write_text("não copiar", encoding="utf-8")
    (source / "agents").mkdir()
    (source / "agents/broad").symlink_to(broad, target_is_directory=True)

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_source_broad_link" for issue in result["issues"])
    assert not (account.home / "agents/broad/random.txt").exists()


async def test_external_skill_symlink_is_allowed_when_it_points_to_one_resource(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    external = source.parent / "personal-skill"
    external.mkdir()
    (external / "SKILL.md").write_text("pessoal", encoding="utf-8")
    (source / "skills").mkdir()
    (source / "skills/pessoal").symlink_to(external, target_is_directory=True)

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert (account.home / "skills/pessoal/SKILL.md").read_text() == "pessoal"


async def test_mcp_environment_does_not_inherit_identity_or_runtime(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", mcp_servers={"probe": {"command": "probe", "env": {
        "CODEX_HOME": str(source),
        "CODEX_SQLITE_HOME": str(source / "state.sqlite"),
        "OPENAI_API_KEY": "secret",
        "OPENAI_IDENTITY_TOKEN_FILE": str(source / "identity-token"),
        "TOOL_ENDPOINT": "keep",
    }}})
    _config(account.home / "config.toml")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    config = tomllib.loads((account.home / "config.toml").read_text())
    assert config["mcp_servers"]["probe"]["env"] == {"TOOL_ENDPOINT": "keep"}
    assert {issue["code"] for issue in result["issues"]} == {
        "codex_account_mcp_runtime_excluded",
        "codex_account_mcp_auth_excluded",
    }
    assert not any("secret" in str(issue) for issue in result["issues"])


async def test_hooks_internal_command_is_remapped_to_destination(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    hook = source / ".hangar-hooks/probe.py"
    hook.parent.mkdir()
    hook.write_text("print('ok')", encoding="utf-8")
    (source / "hooks.json").write_text(json.dumps({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": f"python {hook}"}]}],
    }}), encoding="utf-8")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    data = json.loads((account.home / "hooks.json").read_text())
    command = data["hooks"]["SessionStart"][0]["hooks"][0]["command"]
    assert str(account.home / ".hangar-hooks/probe.py") in command
    assert command != f"python {hook}"


async def test_agent_config_file_reference_is_remapped(isolated, fake_writer):
    _, source, account = isolated
    agent = source / "agents/probe.toml"
    agent.parent.mkdir()
    agent.write_text('name = "probe"\n', encoding="utf-8")
    _config(source / "config.toml", agents={"probe": {"config_file": str(agent)}})

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    config = tomllib.loads((account.home / "config.toml").read_text())
    assert config["agents"]["probe"]["config_file"] == str(account.home / "agents/probe.toml")


async def test_agent_toml_reference_is_transformed_by_native_writer(isolated, fake_writer):
    _, source, account = isolated
    hook = source / "hooks/probe.py"
    hook.parent.mkdir()
    hook.write_text("print('ok')", encoding="utf-8")
    agent = source / "agents/probe.toml"
    agent.parent.mkdir()
    agent.write_text(f'config_file = "{hook}"\n', encoding="utf-8")
    _config(source / "config.toml")

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    data = tomllib.loads((account.home / "agents/probe.toml").read_text())
    assert data["config_file"] == str(account.home / "hooks/probe.py")


async def test_agent_toml_override_does_not_edit_personal_collision(isolated, fake_writer):
    _, source, account = isolated
    hook = source / "hooks/probe.py"
    hook.parent.mkdir()
    hook.write_text("print('ok')", encoding="utf-8")
    agent = source / "agents/probe.toml"
    agent.parent.mkdir()
    agent.write_text(f'config_file = "{hook}"\n', encoding="utf-8")
    _config(source / "config.toml")
    personal = account.home / "agents/probe.toml"
    personal.parent.mkdir()
    personal.write_text('config_file = "local.py"\n', encoding="utf-8")

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_resource_conflict" for issue in result["issues"])
    assert personal.read_text() == 'config_file = "local.py"\n'


async def test_unmapped_hook_reference_preserves_previous_resource(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    hook = source / "hooks.json"
    hook.write_text(json.dumps({"hooks": {"SessionStart": [{"hooks": [{
        "type": "command", "command": f"python {source / 'private.py'}",
    }]}]}}), encoding="utf-8")
    (account.home / "hooks.json").write_text('{"hooks": {"local": []}}', encoding="utf-8")

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    assert any(issue["code"] == "codex_account_unmapped_reference" for issue in result["issues"])
    assert json.loads((account.home / "hooks.json").read_text()) == {"hooks": {"local": []}}


async def test_unmapped_config_reference_preserves_previous_managed_value(isolated, fake_writer):
    _, source, account = isolated
    instruction = source / "AGENTS.md"
    instruction.write_text("global", encoding="utf-8")
    _config(source / "config.toml", model_instructions_file=str(instruction))
    _config(account.home / "config.toml")
    await sync.prepare_account(account)
    _config(source / "config.toml", model_instructions_file=str(source / "private.md"))

    result = await sync.prepare_account(account)

    assert result["status"] == "partial", result
    config = tomllib.loads((account.home / "config.toml").read_text())
    assert config["model_instructions_file"] == str(account.home / "AGENTS.md")


async def test_invalid_manifest_blocks_preparation_and_preserves_destination(isolated, fake_writer):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    await sync.prepare_account(account)
    sync._state_path(account).write_bytes(b"not-json")
    _config(account.home / "config.toml", model="local")

    result = await sync.prepare_account(account)

    assert result["status"] == "error"
    assert any(issue["code"] == "codex_account_state_invalid" for issue in result["issues"])
    assert tomllib.loads((account.home / "config.toml").read_text())["model"] == "local"


@pytest.mark.skipif(not shutil.which("codex"), reason="usa o `codex config` real; o CI nao o instala")
async def test_preparation_uses_native_writer_in_temporary_codex_home(isolated, monkeypatch):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    hook = source / "hooks/probe.py"
    hook.parent.mkdir()
    hook.write_text("print('ok')", encoding="utf-8")
    agent = source / "agents/probe.toml"
    agent.parent.mkdir()
    agent.write_text(f'config_file = "{hook}"\n', encoding="utf-8")
    monkeypatch.setattr(sync, "_NATIVO", CodexNativo)

    result = await sync.prepare_account(account)

    assert result["status"] == "ready", result
    assert tomllib.loads((account.home / "config.toml").read_text())["model"] == "high"
    assert tomllib.loads((account.home / "agents/probe.toml").read_text())["config_file"] == str(
        account.home / "hooks/probe.py")


async def test_source_change_during_write_does_not_advance_signature(isolated, monkeypatch):
    _, source, account = isolated
    _config(source / "config.toml", model="high")
    _config(account.home / "config.toml", model="low")
    changed = []

    async def writer(path, backups, work_dir, nativo, preparar, **kwargs):
        raw = ler(path)
        current = tomllib.loads(raw.decode()) if raw else {}
        edits, confirm = preparar(current)
        updated = copy.deepcopy(current)
        for edit in edits:
            _apply_edit(updated, edit)
        path.write_bytes(_dump_toml(updated))
        if not changed:
            changed.append(True)
            _config(source / "config.toml", model="changed-after-read")
        confirm()

    monkeypatch.setattr(sync, "editar_config", writer)
    result = await sync.prepare_account(account)

    assert result["status"] == "error"
    assert any(issue["code"] == "codex_account_changed_during_prepare" for issue in result["issues"])
    assert sync.preparation_status(account)["status"] == "error"


async def test_trust_pending_anterior_sobrevive_falha_e_atualiza_apos_leitura_valida(
    isolated, fake_writer, monkeypatch,
):
    _, source, account = isolated
    _config(source / "config.toml", model="high", plugins={"sample@market": {"enabled": True}})
    _config(account.home / "config.toml", model="low")
    sync._write_state(account, {"public": {"status": "ready", "trust_pending": True, "issues": []}})
    from app import codex_contas_plugins as plugins

    async def falha(*args, **kwargs):
        return {"manifest": {}, "issues": [{"code": "codex_account_plugin_inventory_failed"}],
                "trust_pending": None}

    async def aprovada(*args, **kwargs):
        return {"manifest": {}, "issues": [], "trust_pending": False}

    monkeypatch.setattr(plugins, "sync_plugins", falha)
    primeira = await sync.prepare_account(account, force=True)
    assert primeira["trust_pending"] is True

    monkeypatch.setattr(plugins, "sync_plugins", aprovada)
    segunda = await sync.prepare_account(account, force=True)
    assert segunda["trust_pending"] is False
