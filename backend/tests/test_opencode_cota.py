"""Cota do OpenCode Go, raspada do painel (app/opencode_cota.py).

O download é trocado; o que se testa é o parse do HTML e a decisão de estado. O formato do trecho
abaixo é o do SSR do SolidJS que o painel emite (`chave:$R[n]={...usagePercent:..resetInSec:..}`),
o mesmo que o pacote pi-quotas casa.
"""
import os
import time

import pytest

from app import contas, opencode_cota


def _html(*, rolling=(12.5, 3600), weekly=(48.0, 172800), monthly=None, invertido=False) -> str:
    partes = ["<html><script>window._$HY=1;"]
    for chave, val in (("rollingUsage", rolling), ("weeklyUsage", weekly), ("monthlyUsage", monthly)):
        if val is None:
            continue
        pct, reset = val
        corpo = (f"resetInSec:{reset},usagePercent:{pct}" if invertido
                 else f"usagePercent:{pct},resetInSec:{reset}")
        partes.append(f"{chave}:$R[3]={{{corpo}}};")
    partes.append("</script></html>")
    return "".join(partes)


@pytest.fixture
def sem_rede(monkeypatch):
    def _usar(status, html):
        monkeypatch.setattr(opencode_cota, "_baixar", lambda w, c: (status, html))
    return _usar


def test_le_as_tres_janelas(sem_rede):
    sem_rede(200, _html(monthly=(9.0, 864000)))
    estado, janelas, motivo = opencode_cota.ler("ws-1", "cookie")
    assert (estado, motivo) == ("lida", None)
    assert [(j["rotulo"], j["pct"]) for j in janelas] == [("5h", 12.5), ("7d", 48.0), ("30d", 9.0)]
    # reset_ts é absoluto (epoch), não "segundos que faltam": é o que a tela sabe desenhar.
    assert janelas[0]["reset_ts"] > time.time() + 3500


def test_ordem_invertida_dos_campos_tambem_e_lida(sem_rede):
    """O SSR emite `resetInSec` antes de `usagePercent` em parte das renderizações. Com um regex
    só, metade das leituras voltaria vazia sem erro nenhum."""
    sem_rede(200, _html(invertido=True))
    estado, janelas, _ = opencode_cota.ler("ws-1", "cookie")
    assert estado == "lida"
    assert [(j["rotulo"], j["pct"]) for j in janelas] == [("5h", 12.5), ("7d", 48.0)]


def test_janela_ausente_nao_inventa_zero(sem_rede):
    sem_rede(200, _html(monthly=None))
    _, janelas, _ = opencode_cota.ler("ws-1", "cookie")
    assert [j["rotulo"] for j in janelas] == ["5h", "7d"]


def test_pagina_de_login_com_200_nao_vira_zero(sem_rede):
    """Cookie vencido: o site responde 200 com a página de login, sem número nenhum. Isso é
    "não informa cota", nunca 0%."""
    sem_rede(200, "<html><body>Sign in to continue</body></html>")
    estado, janelas, motivo = opencode_cota.ler("ws-1", "cookie-velho")
    assert (estado, janelas, motivo) == ("indisponivel", [], "painel-sem-numeros")


def test_sem_resposta_reporta_o_codigo(sem_rede):
    sem_rede(403, "")
    assert opencode_cota.ler("w", "c")[2] == "http-403"
    sem_rede(0, "")
    assert opencode_cota.ler("w", "c")[2] == "sem-resposta"


def test_pct_fora_da_faixa_e_aparado(sem_rede):
    sem_rede(200, _html(rolling=(-3.0, 60), weekly=(140.0, 60)))
    _, janelas, _ = opencode_cota.ler("w", "c")
    assert [j["pct"] for j in janelas] == [0.0, 100.0]


def test_config_grava_le_e_apaga(tmp_path, monkeypatch):
    monkeypatch.setattr(contas, "compartilhado", lambda: tmp_path)
    monkeypatch.delenv("OPENCODE_GO_WORKSPACE_ID", raising=False)
    monkeypatch.delenv("OPENCODE_GO_AUTH_COOKIE", raising=False)
    assert opencode_cota.config_de("chave:oc") is None
    opencode_cota.definir_config("chave:oc", "ws-9", "abc123")
    assert opencode_cota.config_de("chave:oc") == {"workspace_id": "ws-9", "auth_cookie": "abc123"}
    # O arquivo guarda cookie de sessão: 0600, nunca 0644. No Windows nao ha bit de modo (o
    # st_mode volta 0o666 e quem decide e a ACL), e a protecao equivalente ainda NAO existe — a
    # lacuna fica escrita aqui em vez de virar um assert que nao roda. Mesmo tratamento do
    # peers.json e do config dos agentes.
    if os.name == "posix":
        assert oct((tmp_path / ".hangar-opencode.json").stat().st_mode)[-3:] == "600"
    opencode_cota.definir_config("chave:oc", "", "")
    assert opencode_cota.config_de("chave:oc") is None


def test_config_salva_manda_mais_que_o_ambiente(tmp_path, monkeypatch):
    """Variável esquecida num .bashrc não pode ganhar de quem colou na tela."""
    monkeypatch.setattr(contas, "compartilhado", lambda: tmp_path)
    monkeypatch.setenv("OPENCODE_GO_WORKSPACE_ID", "do-ambiente")
    monkeypatch.setenv("OPENCODE_GO_AUTH_COOKIE", "cookie-ambiente")
    assert opencode_cota.config_de("chave:oc")["workspace_id"] == "do-ambiente"
    opencode_cota.definir_config("chave:oc", "da-tela", "cookie-tela")
    assert opencode_cota.config_de("chave:oc")["workspace_id"] == "da-tela"
