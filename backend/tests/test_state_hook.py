import json, os, subprocess, sys
from pathlib import Path

HOOK = str(Path(__file__).resolve().parent.parent / "hooks" / "state_hook.py")

def _run(payload: dict, config_dir: Path) -> None:
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(config_dir)}
    subprocess.run([sys.executable, HOOK], input=json.dumps(payload).encode(),
                   env=env, check=True, timeout=10)

def _marker(config_dir: Path, sid: str) -> dict:
    return json.loads((config_dir / ".claude-pocket-state" / f"{sid}.json").read_text())

def test_user_prompt_submit_is_working(tmp_path):
    _run({"hook_event_name": "UserPromptSubmit", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "working"

def test_stop_is_idle(tmp_path):
    _run({"hook_event_name": "Stop", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "idle"

def test_notification_is_awaiting(tmp_path):
    _run({"hook_event_name": "Notification", "session_id": "abc"}, tmp_path)
    assert _marker(tmp_path, "abc")["state"] == "awaiting_input"

def test_pre_and_post_tool_use_are_working(tmp_path):
    for ev in ("PreToolUse", "PostToolUse"):
        _run({"hook_event_name": ev, "session_id": "s"}, tmp_path)
        assert _marker(tmp_path, "s")["state"] == "working"

def test_marker_has_float_ts(tmp_path):
    _run({"hook_event_name": "Stop", "session_id": "abc"}, tmp_path)
    assert isinstance(_marker(tmp_path, "abc")["ts"], float)

def test_unknown_event_writes_nothing(tmp_path):
    _run({"hook_event_name": "SomethingElse", "session_id": "abc"}, tmp_path)
    assert not (tmp_path / ".claude-pocket-state" / "abc.json").exists()

def test_missing_session_id_does_not_crash(tmp_path):
    _run({"hook_event_name": "Stop"}, tmp_path)  # exit 0, no marker
