"""Busca, Perguntar e Arquivo são só do Rust: no modo `python` respondem indisponível com código."""
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import runtime_coordinator
from app.api import app
from app.config import settings

TOKEN = "t-busca-sem-rust"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
ROUTES = [("GET", "/api/search?q=x"), ("GET", "/api/search/context?project=p&session_id=s&event_id=e"),
          ("POST", "/api/ask-history"), ("GET", "/api/archive"), ("GET", "/api/archive/recent"),
          ("GET", "/api/archive-por-cwd?cwd=/tmp"), ("GET", "/api/archive/-home-u"),
          ("GET", "/api/archive/-home-u/0cc60e6c-eafc-4915-9936-f0c7b81191d9/history"),
          ("GET", "/api/archive/-home-u/0cc60e6c-eafc-4915-9936-f0c7b81191d9/transcript-image/u/0")]


@pytest.fixture
def python_mode(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))


@pytest.mark.parametrize("method,url", ROUTES)
def test_python_mode_answers_unavailable_with_code(python_mode, method, url):
    response = TestClient(app).request(method, url, headers=AUTH, json={"question": "x"} if method == "POST" else None)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_busca_precisa_rust")


def test_unauthenticated_is_401(python_mode):
    assert TestClient(app).get("/api/search?q=x").status_code == 401


def test_resume_is_not_taken(python_mode):
    response = TestClient(app).post("/api/archive/-home-u/0cc60e6c-eafc-4915-9936-f0c7b81191d9/resume", headers=AUTH, json={})
    assert response.json()["detail"]["code"] != "erro_busca_precisa_rust"
