import json

from app import hook_state


def test_demote_service_updates_map_and_file(tmp_path, monkeypatch):
    state_dir = tmp_path / ".hangar-state"
    state_dir.mkdir()
    sidecar = state_dir / "sid1.json"
    sidecar.write_text(json.dumps({"state": "awaiting_input", "ts": 5.0}))
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    monkeypatch.setattr(hook_state, "hook_state", hs)
    assert hs.get_state("sid1")[0] == "awaiting_input"
    for sid in ["sid1", "ausente"]:
        hs.demote_awaiting(sid)
    assert hs.get_state("sid1") == ("idle", 5.0)
    assert json.loads(sidecar.read_text()) == {"state": "idle", "ts": 5.0}
