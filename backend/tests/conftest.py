import pytest


@pytest.fixture(autouse=True)
def _reset_list_snapshot():
    # Endpoints quentes (history/workflows) resolvem a sessao via snapshot com TTL de
    # registry.list() (api._list_snap). Os testes patcham app.api.registry.list POR teste (context
    # manager) -> sem este reset, o snapshot preenchido num teste vazaria pro seguinte dentro do
    # TTL de 1s (fakes de um teste respondendo no outro).
    from app import api
    api._list_snap["infos"] = None
    api._list_snap["t"] = 0.0
    yield
    api._list_snap["infos"] = None
    api._list_snap["t"] = 0.0
