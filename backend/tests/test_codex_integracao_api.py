"""A consulta da integração é só leitura; reconciliar usa a operação dedicada e autenticada."""
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app import codex_integracao
from app.auth import reset_backoff
from app.config import settings
from app.harness_api import harness_router

ROTA = "/api/harness/codex/integracao"
TOKEN = "token-integracao-teste"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture
def painel(monkeypatch):
    snapshot = {
        "estado": "executando", "etapa": "Importando", "ultima_execucao": None,
        "proxima_atualizacao": None, "plugins": [], "erros": [], "avisos": [],
        "confianca_pendente": False,
    }
    servico = SimpleNamespace(status=Mock(return_value=snapshot), iniciar=AsyncMock(return_value=snapshot),
                              sessao=AsyncMock(return_value=snapshot))
    monkeypatch.setattr(codex_integracao, "SERVICO", servico)
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    reset_backoff()
    app = FastAPI()
    app.include_router(harness_router)
    with TestClient(app) as cliente:
        yield cliente, servico, snapshot
    reset_backoff()


@pytest.mark.parametrize("metodo", ["get", "post"])
def test_exige_autenticacao(painel, metodo):
    cliente, servico, _ = painel
    assert getattr(cliente, metodo)(ROTA).status_code == 401
    servico.status.assert_not_called()
    servico.iniciar.assert_not_called()


def test_consulta_nao_inicia_operacao(painel):
    cliente, servico, snapshot = painel
    resposta = cliente.get(ROTA, headers=AUTH)
    assert resposta.status_code == 200
    dados = resposta.json()
    assert isinstance(dados.pop("automatica"), bool)
    assert dados == snapshot
    servico.status.assert_called_once_with()
    servico.iniciar.assert_not_called()


def test_gatilho_de_sessao_e_operacao_propria(painel):
    cliente, servico, snapshot = painel
    resposta = cliente.post(ROTA + "/sessao", headers=AUTH)
    assert resposta.status_code == 202
    dados = resposta.json()
    assert isinstance(dados.pop("automatica"), bool)
    assert dados == snapshot
    servico.sessao.assert_awaited_once_with()
    servico.iniciar.assert_not_called()


def test_reconciliacao_retorna_202_com_snapshot(painel):
    cliente, servico, snapshot = painel
    resposta = cliente.post(ROTA, headers=AUTH)
    assert resposta.status_code == 202
    dados = resposta.json()
    assert isinstance(dados.pop("automatica"), bool)
    assert dados == snapshot
    servico.iniciar.assert_awaited_once_with(motivo="manual", forcar=True)
    servico.status.assert_not_called()
