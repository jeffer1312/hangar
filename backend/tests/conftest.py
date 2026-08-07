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
def _reset_agentpane_cache():
    # _cache do agentpane e dict de MODULO com TTL de 60s: sem reset, a resolucao de uma sessao de
    # teste vazaria pro teste seguinte que reusa o mesmo nome (padrao do _reset_list_snapshot).
    from app import agentpane
    agentpane.invalidate()
    yield
    agentpane.invalidate()


@pytest.fixture(autouse=True)
def _reset_sem_agente_avisadas():
    # _SEM_AGENTE_AVISADAS (Task 5.5) e set de CLASSE (SessionRegistry) com dedup de log por nome
    # que nunca expira: sem reset, um teste que aciona o aviso pra um nome (ex: SESS reusado entre
    # arquivos) envenena o proximo teste que espera o log de novo -- mesmo padrao do
    # _reset_agentpane_cache acima.
    from app.registry import SessionRegistry
    SessionRegistry._SEM_AGENTE_AVISADAS.clear()
    yield
    SessionRegistry._SEM_AGENTE_AVISADAS.clear()


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


# Recortes REAIS do material_colors.scss, medidos em 05/08/2026 trocando o papel de parede: um azul
# e um vermelho. Nao invente valores — a graca e provar que neutro frio vira neutro quente pelo
# mesmo caminho de codigo.
PALETA_AZUL = """$darkmode: True;
$transparent: False;
$primary_paletteKeyColor: #5A77AB;
$background: #111318;
$surface: #111318;
$surfaceContainerLow: #191C20;
$surfaceContainer: #1D2024;
$surfaceContainerHigh: #282A2F;
$onSurface: #E2E2E9;
$onSurfaceVariant: #C4C6D0;
$outline: #8E9099;
$outlineVariant: #44474E;
$primary: #AAC7FF;
$onPrimary: #0A305F;
"""

PALETA_VERMELHA = (PALETA_AZUL
                   .replace("#111318", "#1C110D").replace("#191C20", "#251915")
                   .replace("#1D2024", "#291D18").replace("#282A2F", "#342722")
                   .replace("#E2E2E9", "#F5DED6").replace("#C4C6D0", "#DFC0B5")
                   .replace("#8E9099", "#A78B81").replace("#44474E", "#58423A"))


@pytest.fixture
def paleta_azul():
    return PALETA_AZUL


@pytest.fixture
def paleta_vermelha():
    return PALETA_VERMELHA
