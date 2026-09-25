import json
import os
from pathlib import Path

import pytest

from app import config_sync
from tests.config_sync_machines import make_machine, prefs_path, use_machine


async def _no_after(ctx, items):
    return None


def _no_runner(args):
    raise AssertionError(f"nenhum CLI deveria rodar aqui: {args}")


def _send(monkeypatch, origin, target, items):
    """Exporta na origem, passa pelo tar como no fio, e volta a ser o destino."""
    use_machine(monkeypatch, origin)
    bundle = config_sync.unpack(config_sync.pack(config_sync.export_bundle(origin, items)))
    use_machine(monkeypatch, target)
    return bundle


async def _apply(bundle, items, roots):
    return await config_sync.apply_bundle(bundle, items, roots, runner=_no_runner, after=_no_after)


def _settings(roots):
    return json.loads((Path(roots.claude) / "settings.json").read_text())


@pytest.fixture
def pair(tmp_path):
    return make_machine(tmp_path, "ana"), make_machine(tmp_path, "bia", full=False)


async def test_new_machine_gets_skills_hooks_and_referenced_files(pair, monkeypatch):
    ana, bia = pair
    items = ["claude_instructions", "claude_skills", "claude_hooks"]
    report = await _apply(_send(monkeypatch, ana, bia, items), items, bia)
    home = Path(bia.home)
    assert (home / ".claude/CLAUDE.md").read_text() == "# regras\n"
    assert not (home / ".claude/CLAUDE.local.md").exists()
    skill = home / ".claude/skills/minha"
    assert not skill.is_symlink() and (skill / "SKILL.md").read_text() == "skill em repo\n"
    assert os.access(skill / "run.sh", os.X_OK)
    assert not (home / ".claude/skills/orquestrar").exists()
    assert not (home / ".claude/hooks/guard_tmux.py").exists()
    assert (home / ".claude/hooks/lembrete.py").read_text() == f"print('{home}/notas')\n"
    assert os.access(home / ".orca/agent-hooks/claude-hook.sh", os.X_OK)
    assert (home / ".orca/agent-hooks/lib.sh").read_text() == "# lib\n"
    assert report["items"]["claude_skills"]["status"] == "applied"
    assert report["backup"] == ""


async def test_shell_variables_survive_roundtrip(pair, monkeypatch):
    ana, bia = pair
    await _apply(_send(monkeypatch, ana, bia, ["claude_skills"]), ["claude_skills"], bia)
    assert (Path(bia.claude) / "skills/minha/run.sh").read_text() == \
        '#!/bin/sh\necho "${HOME}" {HOME}\n'


async def test_second_apply_is_same_and_heavy_dirs_survive(pair, monkeypatch):
    ana, bia = pair
    skill = Path(bia.claude) / "skills" / "minha"
    (skill / ".venv").mkdir(parents=True)
    (skill / ".venv" / "keep").write_text("venv de lá")
    (skill / "velho.md").write_text("só no destino")
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    first = await _apply(bundle, ["claude_skills"], bia)
    assert (skill / ".venv" / "keep").read_text() == "venv de lá"
    assert not (skill / "velho.md").exists()
    assert (Path(first["backup"]) / "files/.claude/skills/minha/velho.md").read_text() == \
        "só no destino"
    second = await _apply(bundle, ["claude_skills"], bia)
    assert second["items"]["claude_skills"]["status"] == "same"


async def test_apply_replaces_link_with_folder_and_warns(pair, monkeypatch):
    ana, bia = pair
    repo = Path(bia.home) / "clone" / "minha"
    repo.mkdir(parents=True)
    (repo / "SKILL.md").write_text("versão antiga\n")
    (Path(bia.claude) / "skills").mkdir()
    (Path(bia.claude) / "skills" / "minha").symlink_to(repo)
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_skills"]), ["claude_skills"], bia)
    dest = Path(bia.claude) / "skills" / "minha"
    assert not dest.is_symlink() and (dest / "SKILL.md").read_text() == "skill em repo\n"
    assert (repo / "SKILL.md").read_text() == "versão antiga\n"
    codes = [w["code"] for w in report["items"]["claude_skills"]["warnings"]]
    assert "config_sync_link_replaced" in codes


async def test_same_content_behind_link_is_left_alone(pair, monkeypatch):
    ana, bia = pair
    repo = Path(bia.home) / "clone" / "minha"
    repo.mkdir(parents=True)
    (repo / "SKILL.md").write_text("skill em repo\n")
    (repo / "run.sh").write_text('#!/bin/sh\necho "${HOME}" {HOME}\n')
    (repo / "run.sh").chmod(0o755)
    (Path(bia.claude) / "skills").mkdir()
    (Path(bia.claude) / "skills" / "minha").symlink_to(repo)
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_skills"]), ["claude_skills"], bia)
    assert (Path(bia.claude) / "skills" / "minha").is_symlink()
    assert report["items"]["claude_skills"]["status"] == "same"


async def test_destination_hangar_skills_are_kept(pair, monkeypatch):
    ana, bia = pair
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    bundle.items["claude_skills"]["entries"]["skills/orquestrar"] = {
        "kind": "file", "files": [""], "text": [], "hash": "x"}
    bundle.files["files/claude_skills/skills/orquestrar"] = config_sync.FileBlob(b"falsa", 0o644)
    report = await _apply(bundle, ["claude_skills"], bia)
    assert not (Path(bia.claude) / "skills" / "orquestrar").exists()
    codes = [w["code"] for w in report["items"]["claude_skills"]["warnings"]]
    assert "config_sync_hangar_skill_kept" in codes


async def test_entries_outside_the_config_dir_are_refused(pair, monkeypatch):
    ana, bia = pair
    bundle = _send(monkeypatch, ana, bia, ["claude_instructions"])
    bundle.items["claude_instructions"]["entries"]["../fora.md"] = {
        "kind": "file", "files": [""], "text": [], "hash": "x"}
    bundle.files["files/claude_instructions/../fora.md"] = config_sync.FileBlob(b"x", 0o644)
    report = await _apply(bundle, ["claude_instructions"], bia)
    assert not (Path(bia.home) / "fora.md").exists()
    codes = [w["code"] for w in report["items"]["claude_instructions"]["warnings"]]
    assert "config_sync_invalid_entry" in codes


async def test_backslash_paths_inside_an_entry_are_refused(pair, monkeypatch):
    ana, bia = pair
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    bundle.items["claude_skills"]["entries"]["skills/minha"] = {
        "kind": "dir", "files": ["..\\..\\fora.md"], "text": [], "hash": "x"}
    bundle.files["files/claude_skills/skills/minha/..\\..\\fora.md"] = \
        config_sync.FileBlob(b"x", 0o644)
    report = await _apply(bundle, ["claude_skills"], bia)
    assert not list(Path(bia.home).parent.rglob("*fora.md"))
    codes = [w["code"] for w in report["items"]["claude_skills"]["warnings"]]
    assert "config_sync_invalid_entry" in codes


async def test_drive_letter_paths_are_refused(pair, monkeypatch):
    ana, bia = pair
    items = ["claude_instructions", "claude_skills"]
    bundle = _send(monkeypatch, ana, bia, items)
    bundle.items["claude_skills"]["entries"]["skills/minha"] = {
        "kind": "dir", "files": ["C:/fora.md"], "text": [], "hash": "x"}
    bundle.files["files/claude_skills/skills/minha/C:/fora.md"] = config_sync.FileBlob(b"x", 0o644)
    bundle.items["claude_instructions"]["entries"]["D:fora.md"] = {
        "kind": "file", "files": [""], "text": [], "hash": "x"}
    bundle.files["files/claude_instructions/D:fora.md"] = config_sync.FileBlob(b"x", 0o644)
    report = await _apply(bundle, items, bia)
    assert not list(Path(bia.home).parent.rglob("*fora.md"))
    for item in items:
        codes = [w["code"] for w in report["items"][item]["warnings"]]
        assert "config_sync_invalid_entry" in codes


def _fail_second_write(monkeypatch):
    real, calls = config_sync._write_file, []

    def flaky(path, blob):
        calls.append(path)
        if len(calls) == 2:
            raise OSError(28, "No space left")
        real(path, blob)
    monkeypatch.setattr(config_sync, "_write_file", flaky)


def _old_skill(bia) -> Path:
    skill = Path(bia.claude) / "skills" / "minha"
    (skill / ".venv").mkdir(parents=True)
    (skill / ".venv" / "keep").write_text("venv daqui")
    (skill / ".git").mkdir()
    (skill / ".git" / "HEAD").write_text("ref: main")
    (skill / "SKILL.md").write_text("velho\n")
    return skill


def _assert_untouched_after_failure(report, bia):
    assert report["items"]["claude_skills"]["status"] == "failed"
    assert not list((Path(bia.claude) / "skills").glob(".minha.hangar-novo-*"))


async def test_failed_write_keeps_the_old_folder_and_its_heavy_dirs(pair, monkeypatch):
    ana, bia = pair
    skill = _old_skill(bia)
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    _fail_second_write(monkeypatch)
    report = await _apply(bundle, ["claude_skills"], bia)
    _assert_untouched_after_failure(report, bia)
    assert (skill / "SKILL.md").read_text() == "velho\n"
    assert (skill / ".venv" / "keep").exists()


async def test_failed_write_keeps_the_link(pair, monkeypatch):
    ana, bia = pair
    repo = Path(bia.home) / "clone" / "minha"
    repo.mkdir(parents=True)
    (repo / "SKILL.md").write_text("versão antiga\n")
    (Path(bia.claude) / "skills").mkdir()
    link = Path(bia.claude) / "skills" / "minha"
    link.symlink_to(repo)
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    _fail_second_write(monkeypatch)
    report = await _apply(bundle, ["claude_skills"], bia)
    _assert_untouched_after_failure(report, bia)
    assert link.is_symlink() and os.readlink(link) == str(repo)
    codes = [w["code"] for w in report["items"]["claude_skills"]["warnings"]]
    assert "config_sync_link_replaced" not in codes


async def test_failed_swap_puts_the_folder_and_heavy_dirs_back(pair, monkeypatch):
    ana, bia = pair
    skill = _old_skill(bia)
    bundle = _send(monkeypatch, ana, bia, ["claude_skills"])
    real = config_sync._move

    def swap_fails(src, dst):
        if Path(src).name.startswith(".minha.hangar-novo") and Path(dst) == skill:
            raise PermissionError(13, "arquivo aberto")
        real(src, dst)
    monkeypatch.setattr(config_sync, "_move", swap_fails)
    report = await _apply(bundle, ["claude_skills"], bia)
    _assert_untouched_after_failure(report, bia)
    assert (skill / "SKILL.md").read_text() == "velho\n"
    assert (skill / ".venv" / "keep").read_text() == "venv daqui"
    assert (skill / ".git" / "HEAD").read_text() == "ref: main"


async def test_hangar_file_missing_on_destination_is_reported(pair, monkeypatch):
    ana, bia = pair
    (Path(bia.hangar) / "scripts" / "statusline.js").unlink()
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_hooks"]), ["claude_hooks"], bia)
    codes = [w["code"] for w in report["items"]["claude_hooks"]["warnings"]]
    assert "config_sync_hangar_outdated" in codes


async def test_hooks_keep_destination_hangar_hooks_and_replace_user_hooks(pair, monkeypatch):
    ana, bia = pair
    hangar = Path(bia.hangar)
    ours = f'"{hangar}/.venv/bin/python3" "{hangar}/backend/hooks/state_hook.py" || exit 0'
    (Path(bia.claude) / "settings.json").write_text(json.dumps({"hooks": {
        "PreToolUse": [{"hooks": [{"type": "command", "command": ours}]},
                       {"hooks": [{"type": "command", "command": "python3 /velho.py"}]}],
        "Stop": [{"hooks": [{"type": "command", "command": "echo tchau"}]}],
    }}))
    monkeypatch.setattr("app.config_sync_paths.shutil.which", lambda n: f"/usr/bin/{n}")
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_hooks"]), ["claude_hooks"], bia)
    hooks = _settings(bia)["hooks"]
    commands = [h["command"] for g in hooks["PreToolUse"] for h in g["hooks"]]
    assert commands == [ours, f"python3 {bia.claude}/hooks/lembrete.py",
                        f"/bin/sh '{bia.home}/.orca/agent-hooks/claude-hook.sh'"]
    assert [h["command"] for g in hooks["Stop"] for h in g["hooks"]] == ["echo tchau"]
    replaced = sorted(w["params"]["command"] for w in report["items"]["claude_hooks"]["warnings"]
                      if w["code"] == "config_sync_hook_replaced")
    assert replaced == ["python3 /velho.py"]
    again = await _apply(_send(monkeypatch, ana, bia, ["claude_hooks"]), ["claude_hooks"], bia)
    assert again["items"]["claude_hooks"]["status"] == "same"


async def test_status_line_gets_local_node_and_hangar_path(pair, monkeypatch):
    ana, bia = pair
    monkeypatch.setattr("app.config_sync_paths.shutil.which",
                        lambda n: "/usr/bin/node" if n == "node" else None)
    await _apply(_send(monkeypatch, ana, bia, ["claude_hooks"]), ["claude_hooks"], bia)
    assert _settings(bia)["statusLine"]["command"] == \
        f"'/usr/bin/node' '{bia.hangar}/scripts/statusline.js'"


async def test_missing_program_is_reported(pair, monkeypatch):
    ana, bia = pair
    monkeypatch.setattr("app.config_sync_paths.shutil.which", lambda n: None)
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_hooks"]), ["claude_hooks"], bia)
    missing = {(w["params"]["program"], w["params"]["where"])
               for w in report["items"]["claude_hooks"]["warnings"]
               if w["code"] == "config_sync_missing_program"}
    assert {("node", "statusLine"), ("python3", "PreToolUse")} <= missing


async def test_env_and_settings_origin_wins_and_local_only_keys_stay(pair, monkeypatch):
    ana, bia = pair
    (Path(bia.claude) / "settings.json").write_text(json.dumps(
        {"model": "sonnet", "theme": "light", "env": {"JIRA_TOKEN": "velho", "SO_LA": "1"}}))
    items = ["claude_env", "claude_settings"]
    report = await _apply(_send(monkeypatch, ana, bia, items), items, bia)
    s = _settings(bia)
    assert s["model"] == "opus" and s["theme"] == "light"
    assert s["env"] == {"JIRA_TOKEN": "segredo-jira", "SO_LA": "1"}
    assert report["items"]["claude_env"]["changed"] == ["JIRA_TOKEN"]
    assert report["backup"]   # o settings.json anterior foi guardado


async def test_plugin_keys_merge_per_plugin(pair, monkeypatch):
    ana, bia = pair
    (Path(bia.claude) / "settings.json").write_text(json.dumps(
        {"enabledPlugins": {"so-la@mkt": True, "ponytail@ponytail": False}}))
    monkeypatch.setitem(config_sync._APPLIERS, "claude_plugins", config_sync._apply_plugin_keys)
    await _apply(_send(monkeypatch, ana, bia, ["claude_plugins"]), ["claude_plugins"], bia)
    s = _settings(bia)
    assert s["enabledPlugins"] == {"so-la@mkt": True, "ponytail@ponytail": True}
    assert s["extraKnownMarketplaces"]["ponytail"]["source"]["url"] == "https://x/ponytail.git"


async def test_mcp_keeps_destination_hangar_entry_and_login(pair, monkeypatch):
    ana, bia = pair
    (Path(bia.home) / ".claude.json").write_text(json.dumps({
        "oauthAccount": {"emailAddress": "bia@x"},
        "mcpServers": {"hangar": {"type": "http", "url": "http://127.0.0.1:9999/mcp/"}}}))
    (Path(bia.home) / ".claude.json").chmod(0o644)
    await _apply(_send(monkeypatch, ana, bia, ["claude_mcp"]), ["claude_mcp"], bia)
    data = json.loads((Path(bia.home) / ".claude.json").read_text())
    assert data["oauthAccount"] == {"emailAddress": "bia@x"}
    assert data["mcpServers"]["hangar"]["url"] == "http://127.0.0.1:9999/mcp/"
    assert data["mcpServers"]["grafana"]["headers"] == {"Authorization": "Bearer seg"}
    if os.name != "nt":   # agora leva o Authorization da origem
        assert (Path(bia.home) / ".claude.json").stat().st_mode & 0o777 == 0o600


async def test_engines_file_is_private(pair, monkeypatch):
    ana, bia = pair
    await _apply(_send(monkeypatch, ana, bia, ["engines"]), ["engines"], bia)
    path = Path(bia.claude) / "engines.json"
    assert json.loads(path.read_text())["kimi"]["api_key"] == "sk-kimi"
    if os.name != "nt":
        assert path.stat().st_mode & 0o777 == 0o600


async def test_hangar_prefs_skip_machine_keys(pair, monkeypatch):
    ana, bia = pair
    report = await _apply(_send(monkeypatch, ana, bia, ["hangar_prefs"]), ["hangar_prefs"], bia)
    prefs = json.loads(prefs_path(bia).read_text())
    assert prefs["elevenlabs_api_key"] == "el-key" and prefs["tts_max_chars"] == 900
    assert "sync" not in prefs
    assert sorted(report["items"]["hangar_prefs"]["changed"]) == ["elevenlabs_api_key",
                                                                  "tts_max_chars"]


async def test_install_plugins_adds_missing_marketplace_and_plugin(pair, monkeypatch):
    ana, bia = pair
    calls = []
    monkeypatch.setattr(config_sync.shutil, "which",
                        lambda n: "/usr/bin/claude" if n == "claude" else None)
    report = await config_sync.apply_bundle(
        _send(monkeypatch, ana, bia, ["claude_plugins"]), ["claude_plugins"], bia,
        runner=lambda args: calls.append(args) or (True, ""), after=_no_after)
    assert calls == [
        ["/usr/bin/claude", "plugin", "marketplace", "add", "https://x/ponytail.git"],
        ["/usr/bin/claude", "plugin", "install", "ponytail@ponytail"]]
    assert _settings(bia)["enabledPlugins"] == {"ponytail@ponytail": True}
    assert report["items"]["claude_plugins"]["status"] == "applied"


async def test_plugin_failure_keeps_detail(pair, monkeypatch):
    ana, bia = pair
    monkeypatch.setattr(config_sync.shutil, "which",
                        lambda n: "/usr/bin/claude" if n == "claude" else None)
    report = await config_sync.apply_bundle(
        _send(monkeypatch, ana, bia, ["claude_plugins"]), ["claude_plugins"], bia,
        runner=lambda args: (False, "fatal: repo not found"), after=_no_after)
    warnings = report["items"]["claude_plugins"]["warnings"]
    assert {w["code"] for w in warnings} >= {"config_sync_marketplace_failed",
                                             "config_sync_plugin_failed"}
    assert all(w["params"].get("detail") == "fatal: repo not found" for w in warnings
               if w["code"].endswith("_failed"))


async def test_install_plugins_without_claude_cli_warns(pair, monkeypatch):
    ana, bia = pair
    monkeypatch.setattr(config_sync.shutil, "which", lambda n: None)
    report = await _apply(_send(monkeypatch, ana, bia, ["claude_plugins"]), ["claude_plugins"], bia)
    assert any(w["code"] == "config_sync_missing_program" and w["params"]["program"] == "claude"
               for w in report["items"]["claude_plugins"]["warnings"])
    assert _settings(bia)["enabledPlugins"] == {"ponytail@ponytail": True}


async def test_failing_item_does_not_stop_the_others(pair, monkeypatch):
    ana, bia = pair

    def boom(ctx):
        raise RuntimeError("quebrou")

    monkeypatch.setitem(config_sync._APPLIERS, "claude_env", boom)
    items = ["claude_env", "claude_settings"]
    report = await _apply(_send(monkeypatch, ana, bia, items), items, bia)
    assert report["items"]["claude_env"]["status"] == "failed"
    assert report["items"]["claude_settings"]["status"] == "applied"


class FakeNative:
    edits: list = []

    def __init__(self, home, codex, binario):
        self.codex = codex

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return False

    async def request(self, method, params):
        FakeNative.edits.append(params["edits"])
        (self.codex / "config.toml").write_text('model = "gpt-6"\n')


async def test_apply_codex_writes_agents_and_config_through_native_writer(pair, monkeypatch):
    ana, bia = pair
    (Path(bia.codex) / "config.toml").write_text(
        '[mcp_servers.hangar]\nurl = "http://127.0.0.1:9999/mcp/"\n')
    FakeNative.edits = []
    monkeypatch.setattr(config_sync, "_NATIVE", FakeNative)
    monkeypatch.setattr(config_sync.shutil, "which",
                        lambda n: "/usr/bin/codex" if n == "codex" else None)
    report = await _apply(_send(monkeypatch, ana, bia, ["codex"]), ["codex"], bia)
    assert (Path(bia.codex) / "AGENTS.md").read_text() == "# regras\n"
    edits = {json.loads(e["keyPath"]): e["value"] for e in FakeNative.edits[0]}
    assert edits["model"] == "gpt-6"
    assert set(edits["mcp_servers"]) == {"hangar", "docs"}
    assert edits["mcp_servers"]["hangar"]["url"] == "http://127.0.0.1:9999/mcp/"
    assert "config:model" in report["items"]["codex"]["changed"]


async def test_apply_codex_without_cli_warns(pair, monkeypatch):
    ana, bia = pair
    monkeypatch.setattr(config_sync.shutil, "which", lambda n: None)
    report = await _apply(_send(monkeypatch, ana, bia, ["codex"]), ["codex"], bia)
    assert (Path(bia.codex) / "AGENTS.md").exists()
    assert any(w["params"].get("program") == "codex"
               for w in report["items"]["codex"]["warnings"])


async def test_after_apply_rebuilds_bridges_and_hangar_hooks(pair, monkeypatch):
    ana, bia = pair
    calls = []
    monkeypatch.setattr(config_sync.skill_bridge, "rebuild",
                        lambda home, log=None: calls.append(("bridge", home)))
    for name in config_sync._HANGAR_HOOK_INSTALLERS:
        monkeypatch.setattr(config_sync.hook_installer, name,
                            lambda name=name: calls.append((name,)))
    bundle = _send(monkeypatch, ana, bia, ["claude_hooks"])
    await config_sync.apply_bundle(bundle, ["claude_hooks"], bia, runner=_no_runner)
    assert ("bridge", Path(bia.home)) in calls
    assert ("ensure_state_hooks_installed",) in calls
