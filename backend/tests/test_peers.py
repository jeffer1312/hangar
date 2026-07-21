"""Resolução de peers (peers.py): parse do peers.json, endereço qualificado e erro claro de
transporte. Não sobe um segundo backend — testa só as peças puras que o pareamento cross-server usa
(o caminho HTTP 2-nós fica pra verificação manual entre máquinas)."""
import json

import pytest

from app import peers


def test_is_remote_and_split():
    assert peers.is_remote("srv::sess")
    assert not peers.is_remote("sess")
    assert peers.split_addr("srv::sess") == ("srv", "sess")
    # sessão não tem '::'; split no PRIMEIRO separador
    assert peers.split_addr("srv::a::b") == ("srv", "a::b")


@pytest.fixture
def _tmp_peers(tmp_path, monkeypatch):
    f = tmp_path / "peers.json"
    monkeypatch.setattr(peers, "_PEERS_FILE", f)
    return f


def test_peer_cfg_reads_base_and_token(_tmp_peers):
    _tmp_peers.write_text(json.dumps({
        "box1": {"base_url": "http://10.0.0.2:8766/", "token": "tok123"},
    }), encoding="utf-8")
    assert peers.peer_cfg("box1") == ("http://10.0.0.2:8766", "tok123")  # barra final removida


def test_peer_cfg_missing_or_invalid(_tmp_peers):
    _tmp_peers.write_text(json.dumps({"box1": {"base_url": "http://x"}}), encoding="utf-8")  # sem token
    assert peers.peer_cfg("box1") is None
    assert peers.peer_cfg("desconhecido") is None


def test_peer_cfg_no_file(tmp_path, monkeypatch):
    monkeypatch.setattr(peers, "_PEERS_FILE", tmp_path / "nao-existe.json")
    assert peers.peer_cfg("qualquer") is None


def test_call_unknown_server_raises_peererror(_tmp_peers):
    _tmp_peers.write_text("{}", encoding="utf-8")
    with pytest.raises(peers.PeerError, match="não está em peers.json"):
        peers.call("fantasma", "POST", "/api/x", {})
