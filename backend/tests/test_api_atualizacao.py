"""Endpoints do botão Atualizar."""
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings


@pytest.fixture(autouse=True)
def _sem_rede_do_binario(monkeypatch):
    """A tela lê o manifesto do Rust pela rede; aqui o binário publicado é o do topo e está instalado."""
    from app import api
    api._PUBLICADO.clear()
    monkeypatch.setattr(api.atualizar, "pinned_target", lambda destino: (f"origin/{destino}", None))
    yield
    api._PUBLICADO.clear()


def test_codigo_em_dia_com_binario_publicado_diferente_oferece_atualizar(monkeypatch):
    """O binário do Rust sai depois do código: sem commit novo, a troca dele ainda é atualização."""
    from app import api
    monkeypatch.setattr(api.atualizar.rust_release, "stale", lambda release: True)
    c = _client()
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=[]):
        d = c.get("/api/atualizacao", headers=_AUTH).json()
    assert d["atras"] == 0
    assert d["servidor_desatualizado"] is True and d["atualizacao_disponivel"] is True


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


def test_versao_legivel_e_quantos_commits_atras():
    """A versão que a pessoa lê é data+hash, nas três pontas; `atras` conta o que falta puxar."""
    c = _client()
    def _legivel(ref="HEAD"):
        return "2026.09.14-bbbbbbb" if ref == "origin/main" else "2026.09.12-aaaaaaa"
    with patch("app.api.diag.versao_legivel", _legivel), \
         patch("app.api.diag.VERSAO_LEGIVEL_EM_EXECUCAO", "2026.09.10-ccccccc"), \
         patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=[{"sha": "b", "titulo": "x"}] * 3):
        d = c.get("/api/atualizacao", headers=_AUTH).json()
    assert d["versao_legivel"] == {"repo": "2026.09.12-aaaaaaa", "backend": "2026.09.10-ccccccc",
                                   "remoto": "2026.09.14-bbbbbbb"}
    assert d["atras"] == 3


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
    with patch("app.api.atualizar._git", return_value=subprocess.CompletedProcess([], 0, "", "")) as g, \
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
        stderr = ""

    with patch("app.api.atualizar._git", return_value=P()), patch("app.api.diag.registrar"):
        assert api._mudancas_pendentes() is None


# ─── Auto-update: os gates do laço ───────────────────────────────────────────────────────────────────────

import subprocess
from datetime import datetime, timedelta


def _auto_gate(checar=None, estado=None, sha_dist="abc123", sha_alvo="abc123", ate="origin/main"):
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
         patch("app.api.atualizar.pinned_target", return_value=(ate, None)), \
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


def test_auto_update_espera_o_binario_do_topo():
    """Parar antes do topo é só no botão: o automático espera o topo inteiro publicado."""
    assert _auto_gate(ate="abc999") == "binario do Rust do topo ainda nao publicado para este sistema"


def test_auto_update_sem_conferir_o_binario_espera():
    assert _auto_gate(ate=None) == "nao deu pra conferir o binario do Rust publicado"


def test_auto_update_rev_parse_falhou():
    assert _auto_gate(sha_alvo="") == "rev-parse origin/main falhou"


def test_changelog_compara_com_a_branch_configurada(monkeypatch):
    from app import api
    monkeypatch.setattr(settings, "update_branch", "teste")
    vistos = []

    class P:
        returncode = 0
        stdout = ""

    with patch("app.api.atualizar._git", side_effect=lambda *a, **k: (vistos.append(a), P())[1]):
        api._mudancas_pendentes()
    assert vistos[0][-1] == "HEAD..origin/teste"


def test_auto_update_fica_parado_com_branch_de_teste(monkeypatch):
    """O CI só publica o dist da main; a branch de teste atualiza pelo botão."""
    monkeypatch.setattr(settings, "update_branch", "teste")
    assert _auto_gate() == "branch de teste configurada (CP_UPDATE_BRANCH)"


def test_auto_update_nao_roda_sem_saber_dos_commits_locais():
    assert _auto_gate(checar={"ahead_incerto": True}) == "nao deu pra contar os commits locais"


def test_auto_update_nao_tira_o_checkout_da_branch_de_teste():
    """Campo esvaziado na branch de teste: a volta pra main é pelo botão, nunca pelo laço."""
    assert _auto_gate(checar={"branch": "teste", "alvo": "main"}) == \
        "checkout na branch teste, fora do alvo main"
    assert _auto_gate(checar={"branch": "main", "alvo": "main"}) is None


def _disponivel(pre, mudancas):
    c = _client()
    with patch("app.api.atualizar.checar", return_value=pre), \
         patch("app.api._mudancas_pendentes", return_value=mudancas):
        return c.get("/api/atualizacao", headers=_AUTH).json()


def test_checkout_fora_do_alvo_oferece_atualizar_sem_commit_novo():
    """Campo esvaziado na branch de teste: nenhum commit a puxar, mas a troca pra main é o update."""
    d = _disponivel({"pode": True, "branch": "teste", "alvo": "main", "branch_de_trabalho": False}, [])
    assert d["atualizacao_disponivel"] is True and d["mudancas"] == []
    assert _disponivel({"pode": True, "branch": "master", "alvo": "main"}, [])["atualizacao_disponivel"] is False
    assert _disponivel({"pode": True, "branch": "outra", "alvo": "main", "branch_de_trabalho": True},
                       [])["atualizacao_disponivel"] is False


def test_alvo_que_o_git_nao_compara_oferece_atualizar_e_vai_pro_diario(monkeypatch):
    from app import api
    monkeypatch.setattr(api, "_ALVOS_AUSENTES_AVISADOS", set())
    monkeypatch.setattr(settings, "update_branch", "teste")

    class P:
        returncode = 128
        stdout = "fatal: ambiguous argument 'HEAD..origin/teste': unknown revision"
        stderr = ""

    class Ok:
        returncode = 0
        stdout = ""
        stderr = ""

    with patch("app.api.atualizar._git", return_value=P()), \
         patch("app.api.diag.registrar") as reg, \
         patch("app.api.atualizar.checar", return_value={"pode": True, "branch": "teste", "alvo": "teste"}):
        d = _client().get("/api/atualizacao", headers=_AUTH).json()
        api._mudancas_pendentes()
    assert d["atualizacao_disponivel"] is True and d["mudancas"] == []
    eventos = [c for c in reg.call_args_list if c.args[0] == "atualizacao.alvo_ausente"]
    assert len(eventos) == 1                              # uma vez, mesmo com o polling
    detalhe = eventos[0].kwargs["detalhe"]
    assert "origin/teste" in detalhe and "rc=128" in detalhe and "unknown revision" in detalhe
    # Voltou a comparar: a próxima falha é registrada de novo.
    with patch("app.api.atualizar._git", return_value=Ok()):
        assert api._mudancas_pendentes() == []
    assert "teste" not in api._ALVOS_AUSENTES_AVISADOS


def test_409_de_branch_nomeia_o_alvo():
    c = _client()
    with patch("app.api.atualizar.checar", return_value={
            "pode": True, "branch_de_trabalho": True, "branch": "outra", "alvo": "teste"}), \
         patch("app.api.atualizar.iniciar") as ini:
        r = c.post("/api/atualizacao/iniciar", headers=_AUTH)
    assert r.status_code == 409 and "nao na teste" in r.json()["detail"]["msg"]
    assert r.json()["detail"]["params"]["alvo"] == "teste"
    assert not ini.called


@pytest.mark.parametrize("entrada, esperado", [({"commit": "a" * 40, "api": 3}, 3), ({"commit": "a" * 40}, 1),
                                                ({"commit": "a" * 40, "api": "3"}, 1), (None, 1)])
def test_api_publicado_vem_da_entrada_desta_plataforma(monkeypatch, entrada, esperado):
    """O app nativo compara o contrato do servidor publicado; sem o campo (ou torto) vale 1, nunca erro."""
    from app import api
    manifesto = {"platforms": {"linux-x86_64": entrada} if entrada else {}}
    monkeypatch.setattr(api.atualizar, "pinned_target", lambda destino: (f"origin/{destino}", ("url", None, manifesto)))
    monkeypatch.setattr(api.atualizar.rust_release, "platform_key", lambda: "linux-x86_64")
    monkeypatch.setattr(api.atualizar.rust_release, "stale", lambda release: False)
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=[]):
        d = _client().get("/api/atualizacao", headers=_AUTH).json()
    assert d["api_publicado"] == esperado


def test_api_publicado_sem_release_lida_vale_1():
    with patch("app.api.atualizar.checar", return_value={"pode": True}), \
         patch("app.api._mudancas_pendentes", return_value=[]):
        d = _client().get("/api/atualizacao", headers=_AUTH).json()
    assert d["api_publicado"] == 1


# ─── Modo pacote: o Rust verifica e atualiza; os laços do motor git não rodam ───────────────────

@pytest.mark.parametrize("laco", ["_fetch_loop", "_auto_update_loop"])
def test_lacos_do_motor_git_nao_rodam_no_modo_pacote(monkeypatch, laco):
    """Sem `.git` na pasta da versão, o `git fetch` e o motor `git` rodariam sobre o pacote."""
    import asyncio
    from app import api
    monkeypatch.setattr(api.diag, "install_manifest", lambda: {"version": "0.1.0.1", "commit": "c", "channel": "latest"})
    chamadas = []
    monkeypatch.setattr(api.atualizar, "_git", lambda *a, **k: chamadas.append(a))

    async def dormir(_s):
        raise AssertionError("o laço entrou no ciclo")
    monkeypatch.setattr(api.asyncio, "sleep", dormir)
    asyncio.run(asyncio.wait_for(getattr(api, laco)(), timeout=5))
    assert chamadas == []


def test_fetch_loop_roda_no_checkout(monkeypatch):
    import asyncio
    from app import api
    monkeypatch.setattr(api.diag, "install_manifest", lambda: None)
    chamadas = []
    monkeypatch.setattr(api.atualizar, "_git", lambda *a, **k: chamadas.append(a))

    async def dormir(_s):
        raise asyncio.CancelledError
    monkeypatch.setattr(api.asyncio, "sleep", dormir)
    with pytest.raises(asyncio.CancelledError):
        asyncio.run(api._fetch_loop())
    assert chamadas == [("fetch", "origin")]
