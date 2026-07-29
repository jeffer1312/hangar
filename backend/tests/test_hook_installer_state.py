import json
from pathlib import Path
from app import hook_installer as hi

EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"]
SCRIPT = "/x/state_hook.py"
OLD = f"python3 {SCRIPT}"


def _install(p: Path):
    for ev in EVENTS:
        hi._ensure_event_hook(p, ev, OLD)


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


NEW = f'"/venv/bin/python3" "{SCRIPT}"'


def _ours(p: Path, ev: str) -> list[str]:
    data = json.loads(p.read_text())
    return [h["command"] for b in data["hooks"][ev] for h in b["hooks"]
            if hi._refers_to(h["command"], SCRIPT)]


def test_old_format_replaced_in_every_event(tmp_path):
    # O bug: mudar o formato do command fazia o installer nao reconhecer o proprio hook
    # e acrescentar um segundo — em CADA um dos 6 eventos, a cada subida do backend.
    p = tmp_path / "settings.json"
    _install(p)  # formato antigo
    for ev in EVENTS:
        assert hi._ensure_event_hook(p, ev, NEW) is True
        assert _ours(p, ev) == [NEW]


def test_duplicated_versions_collapse_to_one(tmp_path):
    # Estado real da maquina do usuario: as duas versoes no mesmo evento.
    p = tmp_path / "settings.json"
    p.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "python3 /outro/tts-hook.py"}]},
        {"hooks": [{"type": "command", "command": OLD}]},
        {"hooks": [{"type": "command", "command": NEW}]},
    ]}}))
    assert hi._ensure_event_hook(p, "Stop", NEW) is True
    cmds = [h["command"] for b in json.loads(p.read_text())["hooks"]["Stop"] for h in b["hooks"]]
    assert cmds == ["python3 /outro/tts-hook.py", NEW]  # terceiro intacto, um nosso so
    assert hi._ensure_event_hook(p, "Stop", NEW) is False  # 2a rodada nao mexe


def test_other_checkout_same_filename_is_left_alone(tmp_path):
    p = tmp_path / "settings.json"
    alheio = "python3 /outro/checkout/backend/hooks/state_hook.py"
    p.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": alheio}]}]}}))
    assert hi._ensure_event_hook(p, "Stop", NEW) is True
    cmds = [h["command"] for b in json.loads(p.read_text())["hooks"]["Stop"] for h in b["hooks"]]
    assert cmds == [alheio, NEW]
