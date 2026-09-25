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
