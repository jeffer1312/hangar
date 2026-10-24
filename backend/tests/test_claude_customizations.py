import json

import pytest

from app import claude_customizations as customizations
from app.adapters.claude import ClaudeAdapter
from app.adapters.claude_headless import sessions
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter


SETTINGS = {
    "enabledPlugins": {"superpowers@claude-plugins-official": False},
    "skillOverrides": {"falar": "off"},
    "permissions": {"deny": ["Skill(superpowers:brainstorming)"]},
}
SID = "11111111-1111-1111-1111-111111111111"


def test_session_settings_preserve_existing_flags_without_mutating_the_inputs():
    existing = {
        "env": {"CLAUDE_CODE_EXTRA_BODY": '{"service_tier":"priority"}'},
        "enabledPlugins": {"ecc@ecc": True},
        "permissions": {"deny": ["Bash(rm *)"]},
    }
    argv = ["claude", "--resume", SID, "--settings", json.dumps(existing), "--model", "opus"]

    actual = customizations.apply_settings(argv, SETTINGS)
    settings = json.loads(actual[actual.index("--settings") + 1])

    assert actual.count("--settings") == 1
    assert actual[-2:] == ["--model", "opus"]
    assert settings == {
        "env": {"CLAUDE_CODE_EXTRA_BODY": '{"service_tier":"priority"}'},
        "enabledPlugins": {"ecc@ecc": True, "superpowers@claude-plugins-official": False},
        "skillOverrides": {"falar": "off"},
        "permissions": {"deny": ["Bash(rm *)", "Skill(superpowers:brainstorming)"]},
    }
    assert len(argv) == 7
    assert "superpowers@claude-plugins-official" not in existing["enabledPlugins"]
    assert SETTINGS["permissions"]["deny"] == ["Skill(superpowers:brainstorming)"]


def test_equals_settings_flag_is_merged_and_denials_are_not_duplicated():
    argv = ["claude", "--settings=" + json.dumps({
        "permissions": {"deny": ["Skill(superpowers:brainstorming)"]},
    })]

    actual = customizations.apply_settings(argv, SETTINGS)
    value = json.loads(actual[actual.index("--settings") + 1])

    assert actual.count("--settings") == 1
    assert value["permissions"]["deny"] == ["Skill(superpowers:brainstorming)"]


def test_new_session_without_choices_does_not_inherit_settings_from_another_session():
    argv = ["claude", "--session-id", SID]

    assert customizations.apply_settings(argv, None) == argv
    assert customizations.environment(None) == {"HANGAR_CLAUDE_SETTINGS": ""}


def test_invalid_existing_settings_fail_before_launch(tmp_path):
    path = tmp_path / "settings.json"
    path.write_text("not JSON", encoding="utf-8")

    with pytest.raises(ValueError):
        customizations.apply_settings(["claude", "--settings", str(path)], SETTINGS)


def test_headless_metadata_retains_choices_without_copying_them_to_new_sessions(tmp_path, monkeypatch):
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sessions")
    sessions.save("chosen", str(tmp_path), SID, claude_settings=SETTINGS)
    sessions.update("chosen", model="opus")
    sessions.save("fresh", str(tmp_path), "22222222-2222-2222-2222-222222222222")

    chosen = sessions.load("chosen")
    fresh = sessions.load("fresh")
    assert chosen is not None and chosen["claude_settings"] == SETTINGS
    assert fresh is not None and "claude_settings" not in fresh


def test_terminal_resume_applies_the_session_settings(monkeypatch, tmp_path):
    # O lado sem terminal é lançado pelo Rust (claude_launch.rs) e testado lá.
    from app import pensamento, plugin_bridge

    monkeypatch.setattr(plugin_bridge, "raizes_dos_plugins", lambda: [])
    monkeypatch.setattr(pensamento, "ler", lambda: False)
    terminal = ClaudeAdapter().spawn_command(str(tmp_path), SID, claude_settings=SETTINGS)

    assert json.loads(terminal[terminal.index("--settings") + 1]) == SETTINGS


def test_terminal_binding_carries_choices_for_the_active_conversation(tmp_path, monkeypatch):
    from app import pqueue, runtime_terminal

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(runtime_terminal, "_collect", lambda _: {
        "name": "chosen", "namespace": "mux:1", "pane": "=chosen:0.0", "created": 1,
        "pane_birth": 1, "agent_pid": 42, "agent_birth": 1, "session_proof": "proof",
        "session_id": SID, "config_dir": str(tmp_path / ".claude"), "cwd": str(tmp_path),
        "jsonl": str(tmp_path / f"{SID}.jsonl"), "mux_argv": ["tmux"], "windows": False,
        "claude_settings": SETTINGS,
    })

    binding = runtime_terminal.resolve_binding("chosen")

    assert binding is not None and binding.meta["session_id"] == SID
    assert binding.meta["claude_settings"] == SETTINGS


def test_clear_keeps_choices_and_reports_unavailable_persistence_without_blocking_init(tmp_path, monkeypatch):
    import asyncio
    from app.adapters.claude_headless.adapter import _Sessao

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sessions")
    customizations.configure(None, None)
    meta = sessions.save("chosen", str(tmp_path), SID, config_dir=str(tmp_path), claude_settings=SETTINGS)
    adapter = ClaudeHeadlessAdapter()
    session = _Sessao("chosen", meta)
    new_sid = "33333333-3333-3333-3333-333333333333"

    asyncio.run(adapter._on_system(session, {"subtype": "init", "session_id": new_sid}))

    stored = sessions.load("chosen")
    assert stored is not None and stored["session_id"] == new_sid
    assert stored["claude_settings"] == SETTINGS
    assert session.initialized.is_set()
    assert session.problema == "claude_customizations_unavailable"


def test_default_creation_works_without_rust():
    customizations.configure(None, None)

    assert customizations.prepare(SID, "/tmp", "/tmp/.claude", None, resume=False) is None
