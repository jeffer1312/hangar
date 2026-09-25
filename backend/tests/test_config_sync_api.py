import io
import json
import tarfile

import pytest
from fastapi.testclient import TestClient

from app import config_sync, config_sync_api
from app.api import app
from app.config import settings
from app.config_sync_paths import Roots
from tests.config_sync_machines import make_machine, use_machine

# Convenção da casa (ver test_peers_api.py): cada arquivo declara o próprio token.
TOKEN = "t-config-sync"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
GZIP = {**AUTH, "Content-Type": "application/gzip"}


@pytest.fixture
def cli(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    return TestClient(app)


def _be(monkeypatch, roots):
    use_machine(monkeypatch, roots)
    monkeypatch.setattr(Roots, "this_machine", classmethod(lambda cls: roots))


@pytest.fixture
def ana(tmp_path, monkeypatch):
    roots = make_machine(tmp_path, "ana")
    _be(monkeypatch, roots)
    return roots


def test_routes_require_token(cli, ana):
    assert cli.get("/api/config-sync/manifest").status_code == 401
    assert cli.get("/api/config-sync/bundle?items=claude_env").status_code == 401
    assert cli.post("/api/config-sync/apply?items=claude_env", content=b"x").status_code == 401


def test_manifest_has_hashes_and_no_secret(cli, ana):
    r = cli.get("/api/config-sync/manifest", headers=AUTH)
    assert r.status_code == 200
    body = r.json()
    assert body["version"] == 1 and "JIRA_TOKEN" in body["items"]["claude_env"]["hashes"]
    assert "segredo-jira" not in r.text


def test_bundle_rejects_unknown_item(cli, ana):
    r = cli.get("/api/config-sync/bundle?items=claude_env,nada", headers=AUTH)
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "config_sync_unknown_item"


def test_bundle_too_big_is_413(cli, ana, monkeypatch):
    monkeypatch.setattr(config_sync, "MAX_BUNDLE", 1)
    r = cli.get("/api/config-sync/bundle?items=claude_instructions", headers=AUTH)
    assert r.status_code == 413
    assert r.json()["detail"]["code"] == "config_sync_bundle_too_big"


def test_bundle_then_apply_on_other_machine(cli, ana, tmp_path, monkeypatch):
    raw = cli.get("/api/config-sync/bundle?items=claude_env", headers=AUTH)
    assert raw.status_code == 200
    assert raw.headers["content-type"] == "application/gzip"
    _be(monkeypatch, make_machine(tmp_path, "bia", full=False))

    async def no_after(ctx, items):
        return None

    monkeypatch.setattr(config_sync, "_after_apply", no_after)
    r = cli.post("/api/config-sync/apply?items=claude_env", headers=GZIP, content=raw.content)
    assert r.status_code == 200
    assert r.json()["items"]["claude_env"]["changed"] == ["JIRA_TOKEN"]


def test_apply_rejects_other_version(cli, ana):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        data = json.dumps({"version": 2, "items": {}}).encode()
        info = tarfile.TarInfo("manifest.json")
        info.size = len(data)
        tar.addfile(info, io.BytesIO(data))
    r = cli.post("/api/config-sync/apply?items=claude_env", headers=GZIP, content=buf.getvalue())
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "config_sync_version"


def test_apply_while_busy_is_refused(cli, ana, monkeypatch):
    class Busy:
        def locked(self):
            return True

    monkeypatch.setattr(config_sync_api, "_APPLYING", Busy())
    r = cli.post("/api/config-sync/apply?items=claude_env", headers=GZIP, content=b"x")
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "config_sync_busy"
