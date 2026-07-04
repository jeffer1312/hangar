"""Cobertura do sidecar de vinculo 'then' (chain): set/get, clear, e rename (move preservando o
vinculo). Isola o chain dir apontando settings.projects_dir pra um tmp -- mesmo padrao de test_pqueue.py."""
import pytest

from app import chain
from app.chain import ThenLink


@pytest.fixture(autouse=True)
def _tmp_chain_dir(tmp_path, monkeypatch):
    # _chain_dir() = settings.projects_dir.parent / ".claude-pocket-chain" -> redireciona pro tmp.
    monkeypatch.setattr(chain.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_set_and_get_roundtrip():
    ThenLink("a").set("b", "roda os testes")
    assert ThenLink("a").get() == {"target": "b", "text": "roda os testes"}


def test_get_without_link_is_none():
    assert ThenLink("sem-vinculo").get() is None


def test_clear_removes_sidecar():
    ThenLink("a").set("b", "oi")
    ThenLink("a").clear()
    assert ThenLink("a").get() is None


def test_clear_without_link_is_noop():
    ThenLink("sem-vinculo").clear()  # nao deve estourar


def test_set_overwrites_existing_link():
    ThenLink("a").set("b", "primeiro")
    ThenLink("a").set("c", "segundo")
    assert ThenLink("a").get() == {"target": "c", "text": "segundo"}


def test_rename_moves_link_and_drops_old():
    ThenLink("old").set("alvo", "texto")
    ThenLink("old").rename("new")
    assert ThenLink("old").get() is None
    assert ThenLink("new").get() == {"target": "alvo", "text": "texto"}


def test_rename_without_link_is_noop():
    ThenLink("sem-vinculo").rename("destino")
    assert ThenLink("destino").get() is None
