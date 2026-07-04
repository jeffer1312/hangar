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


def test_list_folders_aggregates(tmp_path):
    _write_transcript(tmp_path / "-home-u-proj")
    _write_transcript(tmp_path / "-home-u-proj", sid="33333333-3333-3333-3333-333333333333",
                      text="segunda conversa")
    folders = archive.list_folders()
    assert [(f.project, f.cwd, f.count) for f in folders] == [
        ("-home-u-proj", "/home/u/proj", 2),
    ]


def test_list_conversations_preview_cwd_live(tmp_path):
    j = _write_transcript(tmp_path / "-home-u-proj")
    entries = archive.list_conversations("-home-u-proj", set())
    assert [(e.project, e.session_id, e.cwd, e.preview, e.live) for e in entries] == [
        ("-home-u-proj", SID, "/home/u/proj", "oi arquivo", False),
    ]
    # live: o realpath do jsonl em uso marca a entrada
    entries = archive.list_conversations("-home-u-proj", {os.path.realpath(str(j))})
    assert entries[0].live is True
    # validacao: projeto fora do alfabeto / inexistente
    import pytest as _pytest
    with _pytest.raises(ValueError):
        archive.list_conversations("../fora", set())
    with _pytest.raises(FileNotFoundError):
        archive.list_conversations("nao-existe", set())


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


def test_archive_cwd_reads_from_header(tmp_path):
    # usado por "Retomar conversa": precisa do cwd real pra subir a sessao nova no lugar certo.
    _write_transcript(tmp_path / "-home-u-proj", cwd="/home/u/proj")
    assert archive.archive_cwd("-home-u-proj", SID) == "/home/u/proj"
    with pytest.raises(FileNotFoundError):
        archive.archive_cwd("-home-u-proj", "22222222-2222-2222-2222-222222222222")
