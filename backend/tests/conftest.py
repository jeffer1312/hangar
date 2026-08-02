import pytest


@pytest.fixture(autouse=True)
def _reset_auth_backoff():
    # O backoff de token errado (app.auth) e um dict de MODULO: sem este reset, os varios testes que
    # mandam token errado de proposito somariam entre si e o proximo caso levaria 429 em vez do 401
    # que ele testa.
    from app import auth
    auth.reset_backoff()
    yield
    auth.reset_backoff()


@pytest.fixture(autouse=True)
def _reset_pi_inbox_ws_warn():
    # Aviso-uma-vez-ate-mudar da recusa de conexao WS (app.api._ws_origem_avisada/_ws_token_avisado)
    # e dict de MODULO: sem reset, o teste que prova "recusa loga" falharia se um teste anterior na
    # mesma sessao ja tivesse "gasto" o aviso daquele host (mesmo padrao do _reset_auth_backoff acima).
    from app import api
    api._ws_origem_avisada.clear()
    api._ws_token_avisado.clear()
    yield
    api._ws_origem_avisada.clear()
    api._ws_token_avisado.clear()


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
