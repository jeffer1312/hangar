"""Cobertura do sidecar de fila durável (pqueue): append/load, clear, e rename (move preservando
entradas). Isola o queue dir apontando settings.projects_dir pra um tmp."""
import pytest

from app import pqueue
from app.pqueue import PromptQueue


@pytest.fixture(autouse=True)
def _tmp_queue_dir(tmp_path, monkeypatch):
    # _queue_dir() = settings.projects_dir.parent / ".claude-pocket-queue" -> redireciona pro tmp.
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_append_and_load_roundtrip():
    q = PromptQueue("s")
    q.append("um")
    q.append("dois")
    assert [e["text"] for e in PromptQueue("s").load()] == ["um", "dois"]


def test_clear_removes_sidecar():
    q = PromptQueue("s")
    q.append("x")
    q.clear()
    assert PromptQueue("s").load() == []


def test_rename_moves_entries_and_drops_old():
    PromptQueue("old").append("msg um")
    PromptQueue("old").append("msg dois")
    PromptQueue("old").rename("new")
    assert PromptQueue("old").load() == []  # nome velho ficou vazio
    assert [e["text"] for e in PromptQueue("new").load()] == ["msg um", "msg dois"]


def test_rename_without_queue_is_noop():
    # Sessao sem fila: rename nao deve criar nada nem estourar.
    PromptQueue("sem-fila").rename("destino")
    assert PromptQueue("destino").load() == []
