import json

from app import loop as loop_mod
from app.loop import ACTIVE, LoopLink, new_loop


def _patch_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(loop_mod.settings, "projects_dir", tmp_path / "projects")


def test_new_loop_shape():
    d = new_loop("passar testes", "pytest -x", 10, True)
    assert d["status"] == "running"
    assert d["iter"] == 0
    assert d["history"] == []
    assert d["goal"] == "passar testes"
    assert d["check_cmd"] == "pytest -x"
    assert d["ended_ts"] is None and d["ended_reason"] is None
    assert d["goal_entry_id"] is None and d["goal_delivered_ts"] is None
    assert isinstance(d["started_ts"], float)


def test_link_roundtrip(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    link = LoopLink("minha-sessao")
    assert link.get() is None
    link.set(new_loop("g", None, 5, False))
    got = link.get()
    assert got["goal"] == "g" and got["check_cmd"] is None
    link.update(status="done", ended_reason="check passou")
    assert link.get()["status"] == "done"
    link.clear()
    assert link.get() is None


def test_link_corrupt_file_is_none(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    link = LoopLink("s")
    link.path.parent.mkdir(parents=True, exist_ok=True)
    link.path.write_text("{broken", encoding="utf-8")
    assert link.get() is None


def test_active_set():
    assert "running" in ACTIVE and "done_claimed" in ACTIVE and "done" not in ACTIVE
