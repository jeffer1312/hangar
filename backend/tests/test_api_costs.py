import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings
from app.costs import PERIODOS
from app.models import CostReport


@pytest.fixture
def client():
    """Padrão do projeto (backend/tests/test_api.py:12): token de verdade em settings + Bearer
    real. NÃO tentar monkeypatch de `require_auth` — Depends() captura a função no momento em
    que a rota é decorada (import de api.py), então reatribuir o atributo do módulo depois não
    troca o dependente já resolvido, e a rota responde 401."""
    settings.auth_token = "secret"
    return TestClient(app)


@pytest.fixture
def h():
    return {"Authorization": "Bearer secret"}


def test_sem_period_o_comportamento_e_o_de_hoje(client, h, monkeypatch):
    # PWA instalado roda o bundle VELHO contra o backend novo por dias. Ausência de period
    # tem que significar "tudo", nunca um default novo que muda a tela de quem não pediu nada.
    visto = {}

    def falso(period="all"):
        visto["p"] = period
        return CostReport()

    monkeypatch.setattr("app.api.costs_report", falso)
    r = client.get("/api/costs", headers=h)
    assert r.status_code == 200
    assert visto["p"] == "all"


def test_period_invalido_cai_em_all_sem_erro(client, h, monkeypatch):
    # 422 aqui derrubaria o custo daquela máquina inteira da soma da malha.
    visto = {}

    def falso(period="all"):
        visto["p"] = period
        return CostReport()

    monkeypatch.setattr("app.api.costs_report", falso)
    r = client.get("/api/costs?period=banana", headers=h)
    assert r.status_code == 200
    assert visto["p"] == "all"


def test_resposta_ecoa_o_filtro_aplicado(client, h, monkeypatch):
    monkeypatch.setattr("app.api.costs_report",
                        lambda period="all": CostReport(applied={"period": period}))
    r = client.get("/api/costs?period=7d", headers=h)
    assert r.json()["applied"]["period"] == "7d"


@pytest.mark.parametrize("period", list(PERIODOS))
def test_todo_periodo_de_costs_py_e_aceito_pela_rota(client, h, monkeypatch, period):
    # Amarra a lista da rota à fonte única (costs.PERIODOS): período novo lá sem ajuste aqui
    # tem que quebrar este teste, não cair calado em "all".
    monkeypatch.setattr("app.api.costs_report",
                        lambda period="all": CostReport(applied={"period": period}))
    r = client.get(f"/api/costs?period={period}", headers=h)
    assert r.json()["applied"]["period"] == period
