import io
import json
import tarfile
from pathlib import Path

import pytest

from app import config_sync
from app.config_sync_paths import mark
from tests.config_sync_machines import make_machine, use_machine

H, C, G = mark("HOME"), mark("CLAUDE"), mark("HANGAR")


@pytest.fixture
def ana(tmp_path, monkeypatch):
    roots = make_machine(tmp_path, "ana")
    use_machine(monkeypatch, roots)
    return roots


def test_skills_resolve_links_skip_heavy_dirs_and_hangar_skills(ana):
    b = config_sync.export_bundle(ana, ["claude_skills"])
    entries = b.items["claude_skills"]["entries"]
    assert list(entries) == ["skills/minha"]
    assert entries["skills/minha"]["files"] == ["SKILL.md", "run.sh"]
    assert not any(".venv" in name for name in b.files)
    assert b.files["files/claude_skills/skills/minha/run.sh"].mode & 0o111
    assert [w["code"] for w in b.warnings["claude_skills"]] == ["config_sync_heavy_dir_skipped"]


def test_instructions_skip_local_md(ana):
    b = config_sync.export_bundle(ana, ["claude_instructions"])
    assert sorted(b.items["claude_instructions"]["entries"]) == ["CLAUDE.md", "rules/a.md"]


def test_what_must_stay_never_leaves_and_chosen_secrets_go(ana):
    raw = config_sync.pack(config_sync.export_bundle(ana, list(config_sync.ITEMS)))
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
        blob = b"".join(tar.extractfile(m).read() for m in tar.getmembers())
    for forbidden in (b"nao-pode-sair", b"ana@x", b"u-ana", "só desta máquina".encode(),
                      b"127.0.0.1:8765", b"trust_level"):
        assert forbidden not in blob, forbidden
    for chosen in (b"segredo-jira", b"Bearer seg", b"sk-kimi", b"el-key"):
        assert chosen in blob, chosen


def test_hooks_drop_hangar_entries_and_carry_referenced_files(ana):
    data = config_sync.export_bundle(ana, ["claude_hooks"]).items["claude_hooks"]
    commands = [h["command"] for g in data["hooks"]["PreToolUse"] for h in g["hooks"]]
    assert commands == [f"python3 {C}/hooks/lembrete.py",
                        f"/bin/sh '{H}/.orca/agent-hooks/claude-hook.sh'"]
    assert sorted(data["entries"]) == ["hooks/lembrete.py"]
    assert set(data["refs"]) == {f"{C}/hooks/lembrete.py", f"{H}/.orca/agent-hooks/claude-hook.sh",
                                 f"{H}/.orca/agent-hooks/lib.sh", f"{G}/scripts/statusline.js"}
    assert data["refs"][f"{G}/scripts/statusline.js"]["member"] == ""
    assert data["statusLine"]["command"] == \
        f"'{H}/.local/share/fnm/v24/bin/node' '{G}/scripts/statusline.js'"


def test_text_files_get_markers_and_shell_variables_stay(ana):
    b = config_sync.export_bundle(ana, ["claude_hooks", "claude_skills"])
    assert b.files["files/claude_hooks/hooks/lembrete.py"].data == f"print('{H}/notas')\n".encode()
    assert b.items["claude_hooks"]["entries"]["hooks/lembrete.py"]["text"] == [""]
    assert b.files["files/claude_skills/skills/minha/run.sh"].data == \
        b'#!/bin/sh\necho "${HOME}" {HOME}\n'


def test_manifest_hashes_match_between_machines_with_same_config(tmp_path, monkeypatch):
    ana, bia = make_machine(tmp_path, "ana"), make_machine(tmp_path, "bia")
    use_machine(monkeypatch, ana)
    a = config_sync.manifest(ana)
    use_machine(monkeypatch, bia)
    b = config_sync.manifest(bia)
    for item in ("claude_instructions", "claude_skills", "claude_hooks", "claude_mcp", "claude_env"):
        assert a["items"][item]["hashes"] == b["items"][item]["hashes"], item
    assert "segredo-jira" not in json.dumps(a)


def test_manifest_sees_changes_in_referenced_files(tmp_path, monkeypatch):
    ana, bia = make_machine(tmp_path, "ana"), make_machine(tmp_path, "bia")
    (Path(bia.home) / ".orca/agent-hooks/claude-hook.sh").write_text("#!/bin/sh\necho outro\n")
    use_machine(monkeypatch, ana)
    a = config_sync.manifest(ana)["items"]["claude_hooks"]["hashes"]
    use_machine(monkeypatch, bia)
    b = config_sync.manifest(bia)["items"]["claude_hooks"]["hashes"]
    key = f"ref:{H}/.orca/agent-hooks/claude-hook.sh"
    assert key in a and a[key] != b[key]
    assert f"ref:{G}/scripts/statusline.js" not in a


def test_pack_roundtrip_and_unpack_rejects_bad_members(ana):
    b = config_sync.export_bundle(ana, ["claude_instructions"])
    back = config_sync.unpack(config_sync.pack(b))
    assert back.items == json.loads(json.dumps(b.items))
    assert back.files["files/claude_instructions/CLAUDE.md"].data == b"# regras\n"
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        info = tarfile.TarInfo("../fora.md")
        info.size = 1
        tar.addfile(info, io.BytesIO(b"x"))
    with pytest.raises(config_sync.BundleError) as exc:
        config_sync.unpack(buf.getvalue())
    assert exc.value.code == "config_sync_invalid_bundle"
    with pytest.raises(config_sync.BundleError):
        config_sync.unpack(b"lixo")


def _members(raw: bytes) -> bytes:
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
        return b"".join(tar.extractfile(m).read() for m in tar.getmembers())


def test_cited_credentials_and_merged_configs_never_travel_as_refs(ana):
    home, claude = Path(ana.home), Path(ana.claude)
    settings = json.loads((claude / "settings.json").read_text())
    settings["statusLine"]["command"] = (
        f"cat '{home}/.claude.json' '{claude}/.credentials.json' '{claude}/settings.json' "
        f"'{claude}/engines.json'")
    (claude / "settings.json").write_text(json.dumps(settings))
    b = config_sync.export_bundle(ana, ["claude_hooks"])
    refs = b.items["claude_hooks"]["refs"]
    for name in (".claude.json", ".credentials.json", "settings.json", "engines.json"):
        assert not any(ref.endswith(name) for ref in refs), (name, sorted(refs))
    blob = _members(config_sync.pack(b))
    assert b"nao-pode-sair" not in blob and b"ana@x" not in blob


def test_private_files_keep_a_private_mode(ana):
    repo = Path(ana.home) / "Projetos" / "skills" / "minha"
    (repo / "SKILL.md").chmod(0o600)
    (repo / "run.sh").chmod(0o700)
    files = config_sync.export_bundle(ana, ["claude_skills"]).files
    assert files["files/claude_skills/skills/minha/SKILL.md"].mode == 0o600
    assert files["files/claude_skills/skills/minha/run.sh"].mode == 0o700


def test_broken_item_does_not_break_the_manifest(ana):
    (Path(ana.claude) / "settings.json").write_text(json.dumps({"hooks": ["x"]}))
    items = config_sync.manifest(ana)["items"]
    assert items["claude_hooks"]["ok"] is False
    assert [w["code"] for w in items["claude_hooks"]["warnings"]] == ["config_sync_item_failed"]
    assert items["claude_skills"]["ok"] is True


def test_bundle_too_big_names_largest_items(ana, monkeypatch):
    monkeypatch.setattr(config_sync, "MAX_BUNDLE", 10)
    with pytest.raises(config_sync.BundleTooBig) as exc:
        config_sync.export_bundle(ana, ["claude_skills", "claude_instructions"])
    assert exc.value.largest[0][0] == "claude_skills"


def test_keep_sends_only_the_chosen_hook_entries(ana):
    keep = {"claude_hooks": ["statusLine", f"ref:{H}/.orca/agent-hooks/claude-hook.sh"]}
    data = config_sync.export_bundle(ana, ["claude_hooks"], keep=keep).items["claude_hooks"]
    assert data["hooks"] == {} and data["entries"] == {}
    assert data["statusLine"]["command"].endswith(f"'{G}/scripts/statusline.js'")
    # O arquivo marcado sozinho vai, o vizinho dele (lib.sh) não foi marcado e fica.
    assert set(data["refs"]) == {f"{G}/scripts/statusline.js", f"{H}/.orca/agent-hooks/claude-hook.sh"}


def test_keep_event_carries_the_files_its_commands_use(ana):
    data = config_sync.export_bundle(ana, ["claude_hooks"],
                                     keep={"claude_hooks": ["hooks:PreToolUse"]}).items["claude_hooks"]
    assert list(data["hooks"]) == ["PreToolUse"] and "statusLine" not in data
    assert f"{C}/hooks/lembrete.py" in data["refs"]


def test_keep_filters_every_item_and_plugin_brings_its_marketplace(ana):
    keep = {"claude_env": [], "claude_plugins": ["plugin:ponytail@ponytail"], "codex": ["config:model"],
            "claude_skills": []}
    items = config_sync.export_bundle(ana, list(keep), keep=keep).items
    assert items["claude_env"]["env"] == {} and items["claude_skills"]["entries"] == {}
    assert list(items["claude_plugins"]["extraKnownMarketplaces"]) == ["ponytail"]
    assert "agents_md" not in items["codex"] and items["codex"]["config"] == {"model": "gpt-6"}


def test_manifest_describes_entries_from_their_own_files(ana):
    hooks = Path(ana.claude) / "hooks"
    (hooks / "lembrete.py").write_text('"""Lembra de anotar.\n\nDetalhe."""\nprint(1)\n')
    items = config_sync.manifest(ana)["items"]
    assert items["claude_skills"]["descriptions"]["skills/minha"] == "skill em repo"
    assert items["claude_hooks"]["descriptions"]["hooks/lembrete.py"] == "Lembra de anotar."
    assert items["claude_hooks"]["labels"]["statusLine"] == ["statusline.js"]
