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


def test_branch_de_trabalho_da_409_e_nao_lanca_a_atualizacao():
    """A atualização alinha o disco com origin/main; numa branch de trabalho ela arrastaria a
    branch. Recusa ANTES de lançar o motor, nomeando a branch pra tela poder dizer qual é."""
    c = _client()
    with patch("app.api.atualizar.checar",
               return_value={"pode": True, "branch_de_trabalho": True, "branch": "mobile-expo"}), \
         patch("app.api.atualizar.iniciar") as ini:
        r = c.post("/api/atualizacao/iniciar", headers=_AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_atualizacao_branch"
    assert r.json()["detail"]["params"]["branch"] == "mobile-expo"
    assert not ini.called


def test_reiniciar_lanca_e_devolve_na_hora():
    c = _client()
    with patch("app.api.atualizar.reiniciar_agora", return_value={"ok": True, "pid": 7}) as rei:
        r = c.post("/api/atualizacao/reiniciar", headers=_AUTH)
    assert r.status_code == 200 and r.json()["pid"] == 7
    assert rei.called


def test_reiniciar_fora_do_systemd_da_409_dizendo_a_topologia():
    """Windows/instalação na mão: quem sobe e derruba o servidor é o instalador, então recusa —
    e a recusa nomeia a topologia, senão a tela só saberia dizer "não deu"."""
    c = _client()
    with patch("app.api.atualizar.reiniciar_agora",
               return_value={"ok": False, "erro": "topologia", "topologia": "windows"}):
        r = c.post("/api/atualizacao/reiniciar", headers=_AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_reinicio_indisponivel"
    assert r.json()["detail"]["params"]["topologia"] == "windows"


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


# ─── Auto-update: os gates do laço ───────────────────────────────────────────────────────────────────────

import subprocess
from datetime import datetime, timedelta


def _auto_gate(checar=None, estado=None, sha_dist="abc123", sha_alvo="abc123"):
    """Roda `_auto_update_motivo` com git/estado/dist do CI mockados. Devolve o motivo de NÃO atualizar (None = dispara)."""
    from app import api
    pre = {"pode": True, "behind": 2, "ahead": 0, "divergiu": False, "sujo": 0, "branch_de_trabalho": False}
    if checar:
        pre.update(checar)

    class _Sha:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self): return (sha_dist or "").encode()

    with patch("app.api.atualizar.checar", return_value=pre), \
         patch("app.api.atualizar.estado", return_value=estado or {}), \
         patch("app.api.atualizar.estado_para_tela", return_value=estado or {}), \
         patch("app.api.urllib.request.urlopen", return_value=_Sha()), \
         patch("app.api.atualizar._git",
               return_value=subprocess.CompletedProcess([], 0, stdout=sha_alvo, stderr="")):
        return api._auto_update_motivo()


def test_auto_update_dispara_com_tudo_aberto():
    assert _auto_gate() is None


def test_auto_update_sem_dist_destes_commit_nao_builda_local():
    """A regra que impede o build local no automático: sha do dist ≠ HEAD → espera o próximo tick."""
    assert _auto_gate(sha_dist="velho", sha_alvo="novo") == "dist do CI ainda nao e deste commit"


def test_auto_update_nao_toca_arvore_suja():
    assert _auto_gate(checar={"sujo": 3}) == "arvore suja (trabalho nao commitado)"


def test_auto_update_em_dia_nao_dispara():
    assert _auto_gate(checar={"behind": 0}) == "em dia"


def test_auto_update_nao_repete_falha_recente():
    falha = {"ok": False, "fase": "pronto", "ts": datetime.now().astimezone().isoformat()}
    assert _auto_gate(estado=falha) == "ultima atualizacao falhou"
    velha = {"ok": False, "fase": "pronto", "ts": (datetime.now().astimezone() - timedelta(days=2)).isoformat()}
    assert _auto_gate(estado=velha) is None


def test_auto_update_ts_invalido_abre_com_log(caplog):
    """ts corrompido: o gate abre (não trava a máquina pra sempre), mas registra warning em vez de ficar mudo."""
    falha = {"ok": False, "fase": "pronto", "ts": "nao-e-data"}
    with caplog.at_level("WARNING"):
        assert _auto_gate(estado=falha) is None
    assert any("ts invalido" in r.message for r in caplog.records)


def test_auto_update_divergiu_vem_antes_de_ahead():
    """divergiu = ahead>0 AND behind>0: o motivo logado tem que ser o mais preciso."""
    assert _auto_gate(checar={"ahead": 2, "behind": 2, "divergiu": True}) == "checkout divergiu de origin/main"


def test_auto_update_rev_parse_falhou():
    assert _auto_gate(sha_alvo="") == "rev-parse origin/main falhou"
