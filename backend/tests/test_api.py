import pytest
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings


@pytest.fixture
def client():
    settings.auth_token = "secret"
    app = FastAPI()

    @app.get("/ping", dependencies=[Depends(require_auth)])
    def ping():
        return {"ok": True}

    with TestClient(app) as c:
        yield c


def test_rejects_without_token(client):
    assert client.get("/ping").status_code == 401


def test_accepts_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200


def test_accepts_cookie(client):
    client.cookies.set("cp_token", "secret")
    r = client.get("/ping")
    assert r.status_code == 200


def test_rejects_wrong_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer wrong"})
    assert r.status_code == 401


def test_rejects_wrong_cookie(client):
    client.cookies.set("cp_token", "wrong")
    r = client.get("/ping")
    assert r.status_code == 401
