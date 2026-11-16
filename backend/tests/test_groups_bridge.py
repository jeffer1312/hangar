"""O Rust é o único que grava `.hangar-pair`; o filho recebe a pasta do arquivo dos grupos."""
import pytest

from app import pair
from app.config import settings


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "arquivo")
    return tmp_path


def test_child_gets_group_env(monkeypatch, tmp_path):
    from app import rust_server
    monkeypatch.delenv("HANGAR_SERVER_ID", raising=False)
    monkeypatch.delenv("HANGAR_PEERS_FILE", raising=False)
    monkeypatch.setattr(settings, "server_id", "casa")
    sup = rust_server.Supervisor(tmp_path / "bin", "127.0.0.1", 1, 2, "t", "127.0.0.1", lambda: False)
    try:
        env = sup._env()
    finally:
        from app import internal_api
        internal_api.set_secret(None)
    assert "HANGAR_SERVER_ID" not in env
    assert "HANGAR_PEERS_FILE" not in env
    assert env["HANGAR_PAIR_ARCHIVE"] == str(pair._arquivo_dir())
