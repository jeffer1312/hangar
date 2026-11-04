"""Os fatos e as escritas ORQ dos grupos pertencem ao Rust, sem callbacks Python."""
import pytest
from fastapi.testclient import TestClient

from app import api, internal_api

SECRET = "cd" * 32


@pytest.fixture(autouse=True)
def _secret():
    internal_api.set_secret(SECRET)
    yield
    internal_api.set_secret(None)


@pytest.mark.parametrize("path", ["orq/group-phase", "orq/promote", "orq/is-orchestrator"])
@pytest.mark.parametrize("headers", [{}, {"X-Hangar-Internal": SECRET}])
def test_orq_group_callbacks_are_gone_even_with_the_secret(path, headers):
    assert not any(getattr(route, "path", "") == f"/internal/{path}" for route in api.app.routes)
    response = TestClient(api.app, client=("127.0.0.1", 50000)).post(
        f"/internal/{path}", json={}, headers=headers)
    assert response.status_code in (404, 405)


def test_protocol_text_route_is_gone():
    assert not any(getattr(route, "path", "") == "/internal/pair/text" for route in api.app.routes)
