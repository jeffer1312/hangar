"""Lista única de credenciais (app/credenciais.py) + apelido (app/apelidos.py).

Nada de rede nem de disco real: `list_config_dirs`, `engines.listar`, `_login_de` e
`cotas.listar_cotas` são trocados; o apelido escreve num `tmp_path` pela pasta compartilhada.
"""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from app import apelidos, contas, cotas, credenciais, engines


@pytest.fixture
def casa(tmp_path, monkeypatch):
    """Pasta compartilhada de mentira — é onde o mapa de apelidos é gravado."""
    monkeypatch.setattr(contas, "compartilhado", lambda: tmp_path)
    return tmp_path


# ----------------------------------------------------------------------------------- apelidos


def test_apelido_grava_e_le(casa):
    assert apelidos.ler() == {}
    apelidos.definir("kimi:apikey", "Kimi")
    assert apelidos.ler() == {"kimi:apikey": "Kimi"}
    assert apelidos.de("kimi:apikey", "apikey") == "Kimi"
    assert apelidos.de("outro:x", "natural") == "natural"


def test_apelido_vazio_apaga(casa):
    apelidos.definir("a", "Nome")
    apelidos.definir("a", "   ")
    assert apelidos.ler() == {}


def test_arquivo_ilegivel_nao_derruba(casa):
    (casa / ".claude-pocket-apelidos.json").write_text("{isto nao e json", encoding="utf-8")
    assert apelidos.ler() == {}
    # JSON válido do TIPO errado cai no mesmo lugar (precedente do statusline.read()).
    (casa / ".claude-pocket-apelidos.json").write_text("[1,2]", encoding="utf-8")
    assert apelidos.ler() == {}


def test_apelido_tem_teto_de_tamanho(casa):
    apelidos.definir("a", "x" * 200)
    assert len(apelidos.ler()["a"]) == apelidos._MAX


# -------------------------------------------------------------------------------- lista única


def _monta(monkeypatch, *, dirs=(), motores=None, cotas_lista=()):
    monkeypatch.setattr(credenciais, "list_config_dirs", lambda: list(dirs))
    monkeypatch.setattr(engines, "listar", lambda: dict(motores or {}))
    monkeypatch.setattr(credenciais, "_login_de", lambda c: credenciais.EstadoLogin(estado="ok", loggedIn=True))
    monkeypatch.setattr(contas, "e_conta", lambda p: True)
    monkeypatch.setattr(cotas, "listar_cotas", lambda: list(cotas_lista))


def _dir(path, label, active=False):
    return SimpleNamespace(path=path, label=label, active=active)


def _cota(cid, pct=13.0, estado="lida"):
    return cotas.CotaConta(id=cid, label="x", provedor="claude", estado=estado,
                           janelas=[cotas.JanelaCota(rotulo="5h", pct=pct)] if estado == "lida" else [],
                           ts=1000.0)


def test_conta_do_claude_e_chave_saem_na_MESMA_lista(casa, monkeypatch):
    _monta(monkeypatch,
           dirs=[_dir("/home/u/.claude", "default", True)],
           motores={"kimi": {"label": "Kimi", "base_url": "https://api.kimi.com/coding/v1",
                             "api_key": "chave-de-exemplo-1234"}},
           cotas_lista=[_cota("claude:/home/u/.claude", 13.0), _cota("chave:kimi", 5.0)])
    linhas = credenciais.listar()
    assert [(c.tipo, c.nome) for c in linhas] == [("claude", "default"), ("chave", "Kimi")]
    assert linhas[0].ativa is True and linhas[0].path == "/home/u/.claude"
    assert linhas[1].usos == ["claude_code"]
    # Cota casada por id nos dois tipos — é o mesmo id da faixa do rodapé.
    assert [c.cota.janelas[0].pct for c in linhas] == [13.0, 5.0]


def test_a_chave_NUNCA_volta_inteira(casa, monkeypatch):
    _monta(monkeypatch, motores={"kimi": {"label": "Kimi", "base_url": "https://x/y",
                                          "api_key": "chave-de-exemplo-longa-4f2a"}})
    linha = credenciais.listar()[0]
    assert linha.chave_mascarada == "chave-d••••4f2a"
    assert "valor-longo" not in json.dumps(linha.model_dump())


def test_apelido_troca_o_nome_mas_guarda_o_original(casa, monkeypatch):
    _monta(monkeypatch, dirs=[_dir("/home/u/.claude-200-01", "200-01")])
    apelidos.definir("claude:/home/u/.claude-200-01", "PMédico 01")
    linha = credenciais.listar()[0]
    assert (linha.nome, linha.nome_natural, linha.apelido) == ("PMédico 01", "200-01", "PMédico 01")


def test_credencial_que_so_a_cota_conhece_aparece_na_lista(casa, monkeypatch):
    """O provider do Kimi vem do config.toml dele, não do cadastro do app. Se ele aparece na faixa
    do rodapé e não na tela, a tela mente sobre ser "todas as credenciais desta máquina"."""
    _monta(monkeypatch, cotas_lista=[_cota("kimi:apikey", 5.0)])
    linhas = credenciais.listar()
    assert [(c.id, c.tipo, c.usos) for c in linhas] == [("kimi:apikey", "chave", ["kimi_cli"])]


def test_conta_sem_cota_lida_continua_na_lista(casa, monkeypatch):
    """Sem leitura de limite a linha existe do mesmo jeito — some-la esconderia justo a conta que
    precisa de atenção."""
    _monta(monkeypatch, dirs=[_dir("/home/u/.claude", "default", True)],
           cotas_lista=[_cota("claude:/home/u/.claude", estado="indisponivel")])
    linha = credenciais.listar()[0]
    assert linha.cota.estado == "indisponivel" and linha.cota.janelas == []
