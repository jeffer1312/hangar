"""Endpoints do botão Atualizar."""
from unittest.mock import patch

from fastapi.testclient import TestClient

from app.config import settings


def _client():
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app)


_AUTH = {"Authorization": "Bearer secret"}


def test_exige_auth():
    c = _client()
    assert c.get("/api/atualizacao").status_code == 401
    assert c.post("/api/atualizacao/iniciar").status_code == 401


def test_as_duas_versoes_vem_de_fontes_diferentes():
    """A do disco e a do processo vivo divergem durante a atualização — é o ponto de mostrar as duas."""
    c = _client()
    with patch("app.api.diag._git_describe", return_value="v9-disco"), \
         patch("app.api.diag.VERSAO_EM_EXECUCAO", "v8-processo"), \
         patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=[]):
        d = c.get("/api/atualizacao", headers=_AUTH).json()
    assert d["versoes"] == {"repo": "v9-disco", "backend": "v8-processo"}
    assert d["atualizacao_disponivel"] is False


def test_procurar_vai_a_rede_antes_de_comparar():
    """Sem o fetch, "Procurar de novo" respondia com a foto do último fetch automático (30min).

    Ou seja: afirmava ter procurado sem ter ido olhar — pior que não ter o botão.
    """
    c = _client()
    ordem = []
    def _git_espiao(*args, **kw):
        ordem.append(args[0])
        class P:
            returncode = 0
            stdout = ""
        return P()
    with patch("app.api.atualizar._git", _git_espiao), \
         patch("app.api.atualizar.checar", side_effect=lambda: (ordem.append("checar"), {"pode": True})[1]):
        c.get("/api/atualizacao?procurar=1", headers=_AUTH)
    assert ordem[0] == "fetch", f"o fetch tem que vir ANTES da comparação: {ordem}"
    assert "checar" in ordem


def test_polling_normal_nao_vai_a_rede():
    """A tela bate neste endpoint a cada 2s durante a atualização — fetch ali é rede à toa."""
    c = _client()
    with patch("app.api.atualizar._git") as g, \
         patch("app.api.atualizar.checar", return_value={"pode": True}):
        c.get("/api/atualizacao", headers=_AUTH)
    assert not any(ch.args and ch.args[0] == "fetch" for ch in g.call_args_list)


def test_mudanca_pendente_liga_o_aviso():
    c = _client()
    mudancas = [{"sha": "abc1234", "titulo": "fix: alguma coisa"}]
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=mudancas):
        d = c.get("/api/atualizacao", headers=_AUTH).json()
    assert d["atualizacao_disponivel"] is True
    assert d["mudancas"] == mudancas


def test_iniciar_lanca_e_devolve_na_hora():
    c = _client()
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api.atualizar.iniciar", return_value={"ok": True, "pid": 42}) as ini:
        r = c.post("/api/atualizacao/iniciar", headers=_AUTH)
    assert r.status_code == 200 and r.json()["pid"] == 42
    assert ini.called


def test_iniciar_duas_vezes_da_409():
    c = _client()
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api.atualizar.iniciar", return_value={"ok": False, "erro": "ja_rodando"}):
        r = c.post("/api/atualizacao/iniciar", headers=_AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_atualizacao_ja_rodando"


def test_dependencia_faltando_da_409_nomeando_o_que_falta():
    """Recusa ANTES de começar, com o nome — a tela precisa poder dizer o que instalar."""
    c = _client()
    with patch("app.api.atualizar.checar", return_value={"pode": False, "faltando": ["uv"]}), \
         patch("app.api.atualizar.iniciar") as ini:
        r = c.post("/api/atualizacao/iniciar", headers=_AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["params"]["faltando"] == ["uv"]
    assert not ini.called


def test_config_devolve_a_versao_do_processo():
    c = _client()
    with patch("app.api.diag.VERSAO_EM_EXECUCAO", "v8-processo"):
        d = c.get("/api/config", headers=_AUTH).json()
    assert d["somente_leitura"]["versao"] == "v8-processo"


def test_mudancas_parseia_a_saida_do_git():
    from app import api

    class P:
        returncode = 0
        stdout = "abc1234\x00fix: um\ndef5678\x00feat: dois\n"

    with patch("app.api.atualizar._git", return_value=P()):
        assert api._mudancas_pendentes() == [
            {"sha": "abc1234", "titulo": "fix: um"},
            {"sha": "def5678", "titulo": "feat: dois"},
        ]


def test_git_que_falha_nao_inventa_changelog():
    from app import api

    class P:
        returncode = 128
        stdout = ""

    with patch("app.api.atualizar._git", return_value=P()):
        assert api._mudancas_pendentes() == []
