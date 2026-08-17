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

from app import alcance, config
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


# ── Pareamento (Task 6, Lote B) ────────────────────────────────────────────────
# A rota devolve endereço + credencial para o candidato pedido; candidato desconhecido
# é recusado; sem credencial configurada, erro nomeado. O QR é SVG, desenhado no
# BACKEND (decisão de plano: o front só tem qr-scanner, que lê e não gera).


@pytest.fixture(autouse=True)
def _sem_rede(monkeypatch):
    """NENHUM teste de pareamento toca rede nem processo real (régua do grupo).
    `levantar_estados` chama `_bater` para cada endereço e `_nome_tailscale` para o
    candidato Tailscale — as duas são funções privadas de I/O, trocadas aqui.
    Instrumentado (rodada 1): os 4 testes disparavam 9 urlopen e 3 subprocessos
    `tailscale status`, incluindo o IP da LAN e o host Tailscale reais."""
    monkeypatch.setattr(alcance, "_bater", lambda url: 5.0)
    monkeypatch.setattr(alcance, "_nome_tailscale", lambda: "hangar.tail0000.ts.net")


def test_pareamento_sem_credencial_e_401(cli):
    # Régua da casa: rota nova traz o caso de 401 sem credencial (mesmo padrão do
    # test_sem_credencial_e_401 da listagem).
    assert cli.get("/api/alcance/pareamento?endereco=rede_local").status_code == 401


def test_pareamento_devolve_url_com_token_e_qr_svg(cli, monkeypatch):
    monkeypatch.setattr(alcance, "_detectar_lan", lambda: "192.168.0.42")
    # pairing_url (config.py:215) chama detect_lan_ip DIRETO do config — o mesmo
    # patch no alcance._detectar_lan não alcança; o patch abaixo cobre os dois.
    monkeypatch.setattr(config, "detect_lan_ip", lambda: "192.168.0.42")
    monkeypatch.setattr(settings, "lan_bind_ip", "127.0.0.1")
    monkeypatch.setattr(settings, "public_url", "")
    monkeypatch.setattr(settings, "front_port", 5173)
    monkeypatch.setattr(settings, "auth_token", "9f4c2ae1b73d08e5")
    r = cli.get(
        "/api/alcance/pareamento?endereco=rede_local",
        headers={"Authorization": "Bearer 9f4c2ae1b73d08e5"},
    )
    assert r.status_code == 200
    dados = r.json()
    # Endereço do candidato pedido + credencial, no formato que o validarPareamento
    # (lib/auth.ts:368-405) exige: exatamente um token=.
    assert dados["url"] == "http://192.168.0.42:5173/?token=9f4c2ae1b73d08e5"
    assert "<svg" in dados["qr_svg"]


def test_pareamento_tailscale_usa_o_endereco_do_candidato(cli, monkeypatch):
    """TRAVA do conserto do endereço escolhido (rodada 1, bloqueador 6): com
    public_url vazio, pedir ?endereco=tailscale devolve o URL do candidato
    Tailscale — e NÃO o pairing_url genérico (que cairia no IP da LAN).
    Reintroduzir `url = pairing_url(settings)` tem de deixar este teste VERMELHO."""
    monkeypatch.setattr(alcance, "_detectar_lan", lambda: "192.168.0.42")
    monkeypatch.setattr(config, "detect_lan_ip", lambda: "192.168.0.42")
    monkeypatch.setattr(settings, "lan_bind_ip", "127.0.0.1")
    monkeypatch.setattr(settings, "public_url", "")
    monkeypatch.setattr(settings, "front_port", 5173)
    monkeypatch.setattr(settings, "auth_token", "9f4c2ae1b73d08e5")
    r = cli.get(
        "/api/alcance/pareamento?endereco=tailscale",
        headers={"Authorization": "Bearer 9f4c2ae1b73d08e5"},
    )
    assert r.status_code == 200
    dados = r.json()
    assert dados["url"] == "https://hangar.tail0000.ts.net/?token=9f4c2ae1b73d08e5"


def test_pareamento_endereco_desconhecido_recusado(cli):
    r = cli.get("/api/alcance/pareamento?endereco=nao_existe", headers=AUTH)
    assert r.status_code == 404
    assert r.json()["detail"]["code"] == "alcance_endereco_desconhecido"


def test_pareamento_sem_credencial_configurada_erro_nomeado(cli, monkeypatch):
    # Credencial de fábrica: o require_auth compara o Bearer com o settings ATUAL —
    # ao trocar o token, o header AUTH (constante) deixa de valer; usa-se o token
    # novo no header, e a rota ainda responde 400 (e não 401) porque o auth passa.
    monkeypatch.setattr(settings, "auth_token", "change-me")
    monkeypatch.setattr(settings, "lan_bind_ip", "127.0.0.1")
    r = cli.get("/api/alcance/pareamento?endereco=rede_local", headers={"Authorization": "Bearer change-me"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "alcance_sem_credencial"
