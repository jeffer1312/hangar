import json
from pathlib import Path
from app import hook_installer as hi

EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"]


def _install(p: Path):
    for ev in EVENTS:
        hi._ensure_event_hook(p, ev, "python3 /x/state_hook.py")


def test_installs_all_five_events(tmp_path):
    p = tmp_path / "settings.json"
    _install(p)
    data = json.loads(p.read_text())
    for ev in EVENTS:
        cmds = [h["command"] for b in data["hooks"][ev] for h in b["hooks"]]
        assert "python3 /x/state_hook.py" in cmds


def test_idempotent(tmp_path):
    p = tmp_path / "settings.json"
    _install(p)
    first = p.read_text()
    _install(p)  # second run: no change
    assert p.read_text() == first


def test_preserves_existing_pretooluse(tmp_path):
    p = tmp_path / "settings.json"
    p.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": "python3 /x/askq_capture.py"}]}
    ]}}))
    _install(p)
    cmds = [h["command"] for b in json.loads(p.read_text())["hooks"]["PreToolUse"] for h in b["hooks"]]
    assert "python3 /x/askq_capture.py" in cmds  # askq kept
    assert "python3 /x/state_hook.py" in cmds    # state added alongside


def test_skips_malformed_settings(tmp_path):
    p = tmp_path / "settings.json"
    p.write_text("{ not json")
    _install(p)
    assert p.read_text() == "{ not json"  # never clobbered
