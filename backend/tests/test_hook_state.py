import json, time
from pathlib import Path
from app import hook_state


def _write(d: Path, sid: str, state: str):
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))


def test_load_existing_seeds_map(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    _write(sd, "bbb", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    assert hs.get_state("bbb")[0] == "idle"


def test_get_state_none_when_absent(tmp_path):
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("missing") is None


def test_apply_updates_existing(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    _write(sd, "aaa", "idle")            # state flips
    hs._apply(sd / "aaa.json")
    assert hs.get_state("aaa")[0] == "idle"


def test_apply_ignores_bad_json(tmp_path):
    sd = tmp_path / ".claude-pocket-state"; sd.mkdir(parents=True)
    (sd / "x.json").write_text("{ not json")
    hs = hook_state.HookState()
    hs._apply(sd / "x.json")             # no raise
    assert hs.get_state("x") is None
