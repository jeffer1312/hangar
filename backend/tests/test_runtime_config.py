"""Camada de config editável em runtime: override em JSON por cima do env.

O que esta suíte trava: segredo nunca volta inteiro, campo desconhecido não vira setting, e
gravar é atômico (arquivo pela metade viraria "sem config nenhuma", calado).
"""
import json

import pytest

from app import runtime_config as rc


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


def test_sem_arquivo_vale_o_env(monkeypatch):
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    assert rc.get("upload_retention_days") == 30
    assert rc.estado()["upload_retention_days"]["origem"] == "env"


def test_override_vence_o_env(monkeypatch):
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    rc.aplicar({"upload_retention_days": 7})
    assert rc.get("upload_retention_days") == 7
    assert rc.estado()["upload_retention_days"]["origem"] == "app"


def test_campo_desconhecido_e_ignorado():
    rc.aplicar({"auth_token": "roubado", "port": 1})
    salvo = json.loads((rc._caminho()).read_text())
    assert salvo == {}          # o cliente não inventa setting


def test_segredo_volta_mascarado_nunca_inteiro():
    rc.aplicar({"groq_api_key": "gsk_abcdefghijklmnop"})
    est = rc.estado()["groq_api_key"]
    assert est["definido"] is True
    assert est["valor"] == "gsk_••••••••mnop"
    assert "abcdefghij" not in est["valor"]
    # O valor real continua acessível pro backend usar.
    assert rc.get("groq_api_key") == "gsk_abcdefghijklmnop"


def test_devolver_a_mascara_exata_nao_apaga_a_chave():
    """Reenviar EXATAMENTE o que o GET devolveu preserva o segredo.

    A versão anterior deste teste usava "••••••••" — uma forma que `mascarar()` nunca produz pra
    chave real (>8 chars vira mista: gsk_••••••••1234). O teste passava e o caminho de verdade
    estava quebrado: encostar no campo sobrescrevia a chave pelo texto mascarado. Aqui o valor
    reenviado sai de `estado()`, que é a fonte que o cliente enxerga.
    """
    real = "gsk_abcdefghijklmnop"
    rc.aplicar({"groq_api_key": real})
    mascara = rc.estado()["groq_api_key"]["valor"]
    assert mascara != real and "•" in mascara
    rc.aplicar({"groq_api_key": mascara})          # o que o front reenviaria
    assert rc.get("groq_api_key") == real
    rc.aplicar({"groq_api_key": "  " + mascara + " "})   # com espaço do teclado
    assert rc.get("groq_api_key") == real


def test_chave_nova_de_verdade_substitui():
    rc.aplicar({"groq_api_key": "gsk_primeira_chave_aqui"})
    rc.aplicar({"groq_api_key": "gsk_segunda_chave_nova"})
    assert rc.get("groq_api_key") == "gsk_segunda_chave_nova"


def test_dois_patch_concorrentes_nao_perdem_mudanca():
    """Sem lock, os dois liam o mesmo estado e o último a gravar apagava o outro."""
    import threading

    rc.aplicar({"editor": "code"})
    erros = []

    def grava(campo, valor):
        try:
            rc.aplicar({campo: valor})
        except Exception as e:  # pragma: no cover
            erros.append(e)

    ts = [threading.Thread(target=grava, args=("upload_retention_days", 11)),
          threading.Thread(target=grava, args=("stall_seconds", 222))]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert not erros
    assert rc.get("upload_retention_days") == 11
    assert rc.get("stall_seconds") == 222      # as DUAS sobrevivem


def test_tipos_invalidos_sao_recusados():
    with pytest.raises(ValueError):
        rc.aplicar({"upload_retention_days": "trinta"})
    with pytest.raises(ValueError):
        rc.aplicar({"upload_retention_days": -1})
    with pytest.raises(ValueError):
        rc.aplicar({"automations": "sim"})


def test_arquivo_corrompido_nao_derruba(monkeypatch):
    rc._caminho().write_text("{ isso não é json")
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    assert rc.get("upload_retention_days") == 30      # cai pro env, sem exceção


def test_nao_deixa_lixo_tmp_ao_gravar():
    rc.aplicar({"editor": "vim"})
    tmps = list(rc._caminho().parent.glob("*.tmp"))
    assert tmps == []


def test_editor_nao_aceita_caminho():
    """O editor vira argv[0] de subprocess. Nome nu (code, nvim) sim; caminho solto, não."""
    rc.aplicar({"editor": "nvim"})
    assert rc.get("editor") == "nvim"
    for ruim in ["/tmp/evil.sh", "../../bin/sh", "-flag", "sub/dir/bin"]:
        with pytest.raises(ValueError):
            rc.aplicar({"editor": ruim})
    assert rc.get("editor") == "nvim"       # nenhuma tentativa ruim passou


def test_llm_base_url_recusa_esquema_nao_http():
    """Mesmo argumento do editor: antes só o dono da máquina escolhia o endpoint (env), agora o
    celular escreve — só vazio (volta ao padrão) ou http(s):// de verdade."""
    rc.aplicar({"llm_base_url": "https://x/v1"})
    with pytest.raises(ValueError):
        rc.aplicar({"llm_base_url": "ftp://x"})
    assert rc.get("llm_base_url") == "https://x/v1"     # a tentativa ruim não passou


def test_llm_base_url_aceita_http_e_vazio():
    rc.aplicar({"llm_base_url": "https://x/v1"})
    assert rc.get("llm_base_url") == "https://x/v1"
    rc.aplicar({"llm_base_url": ""})
    assert rc.get("llm_base_url") == ""
