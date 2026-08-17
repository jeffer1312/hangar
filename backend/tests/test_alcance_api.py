"""Borda HTTP do alcance — por onde este servidor responde (aba Acesso).

A Task 1 só abriu o espaço: a rota de listagem existe e devolve lista vazia, em vez
de 404. Esta Task (3) preenche o conteúdo: a rota devolve a lista com estado por
endereço e sinaliza quando o bind é loopback. A Task 6 (Lote B) acrescenta aqui o
caso de pareamento, sem tocar no que já está.

Depois da revisão da Task 1 — o revisor removeu o `require_auth` desta rota e a
suíte ficou verde — o caso de 401 sem credencial é obrigatório (régua do grupo).
"""
import pytest
from fastapi.testclient import TestClient

from app import alcance
from app.api import app
from app.config import settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-alcance"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def test_sem_credencial_e_401(cli):
    # Régua da casa: sem este caso a rota ficava VERDE mesmo com o require_auth
    # removido — a suíte precisa provar que a autenticação protege de verdade.
    assert cli.get("/api/alcance").status_code == 401


def test_lista_estado_por_endereco_e_sinal_de_loopback(cli, monkeypatch):
    # Mesma técnica do test_engine_probe: as funções privadas de I/O são trocadas —
    # nenhum teste toca rede nem processo real.
    monkeypatch.setattr(alcance, "_bater", lambda url: 12.0)
    monkeypatch.setattr(alcance, "_detectar_lan", lambda: "192.168.0.42")
    monkeypatch.setattr(alcance, "_nome_tailscale", lambda: "hangar.tail9c2f.ts.net")
    monkeypatch.setattr(settings, "lan_bind_ip", "127.0.0.1")
    monkeypatch.setattr(settings, "public_url", "")
    r = cli.get("/api/alcance", headers=AUTH)
    assert r.status_code == 200
    dados = r.json()
    assert dados["loopback"] is True
    assert dados["bind"] == "127.0.0.1"
    assert [e["tipo"] for e in dados["enderecos"]] == [
        "nesta_maquina", "rede_local", "tailscale", "publico",
    ]
    assert dados["enderecos"][0]["estado"] == "ok"
    assert dados["enderecos"][0]["tempo_ms"] == 12
    assert dados["enderecos"][3]["estado"] == "nao_configurado"
    assert dados["enderecos"][3]["tempo_ms"] is None
