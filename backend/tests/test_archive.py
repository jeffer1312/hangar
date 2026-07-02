"""Arquivo de conversas mortas: listagem (preview/cwd/live) e validacao anti-traversal do path."""
import json
import os

import pytest

from app import archive


SID = "11111111-1111-1111-1111-111111111111"


@pytest.fixture(autouse=True)
def _tmp_projects(tmp_path, monkeypatch):
    monkeypatch.setattr(archive.settings, "projects_dir", tmp_path)
    return tmp_path


def _write_transcript(projdir, sid=SID, text="oi arquivo", cwd="/home/u/proj"):
    projdir.mkdir(parents=True, exist_ok=True)
    j = projdir / f"{sid}.jsonl"
    j.write_text(json.dumps({
        "type": "user", "uuid": "u1", "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z",
        "message": {"role": "user", "content": text},
    }) + "\n", encoding="utf-8")
    return j


def test_list_archive_preview_cwd_live(tmp_path):
    j = _write_transcript(tmp_path / "-home-u-proj")
    entries = archive.list_archive(set())
    assert [(e.project, e.session_id, e.cwd, e.preview, e.live) for e in entries] == [
        ("-home-u-proj", SID, "/home/u/proj", "oi arquivo", False),
    ]
    # live: o realpath do jsonl em uso marca a entrada
    entries = archive.list_archive({os.path.realpath(str(j))})
    assert entries[0].live is True


def test_archive_jsonl_valid_path(tmp_path):
    j = _write_transcript(tmp_path / "-home-u-proj")
    assert archive.archive_jsonl("-home-u-proj", SID) == j


def test_archive_jsonl_rejects_traversal_and_missing(tmp_path):
    _write_transcript(tmp_path / "-home-u-proj")
    with pytest.raises(ValueError):
        archive.archive_jsonl("../fora", SID)          # projeto fora do alfabeto
    with pytest.raises(ValueError):
        archive.archive_jsonl("-home-u-proj", "../../x")  # sid nao-uuid
    with pytest.raises(FileNotFoundError):
        archive.archive_jsonl("-home-u-proj", "22222222-2222-2222-2222-222222222222")
