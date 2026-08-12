"""Parse do `pi --list-models`.

Por que parse de tabela e não JSON: o `pi --list-models` não tem modo JSON (medido — `--json` não é
flag dele; `--mode json` é do agente). E por que ele e não o sidecar da extensão: medido em
10/08/2026, as duas fontes trazem os MESMOS 384 modelos, mas só esta traz contexto e imagem. O
sidecar continua sendo a fonte de `levels` por modelo, que esta não tem.
"""
import subprocess

import pytest

from app import pi_catalog

SAIDA = """provider          model                                    context  max-out  thinking  images
cline             deepseek/deepseek-v4-flash               1.0M     131.1K   yes       no
kimi-coding       k3                                       1.0M     131.1K   yes       yes
clinepass         cline-pass/glm-5.2                       200K     131.1K   yes       no
"""


def test_cabecalho_e_descartado():
    assert all(m["id"] != "model" for m in pi_catalog.parse(SAIDA))


def test_le_provedor_e_id():
    ms = pi_catalog.parse(SAIDA)
    assert {"provider": "kimi-coding", "id": "k3"}.items() <= ms[1].items()


def test_id_com_barra_dentro_sobrevive():
    """`clinepass` + `cline-pass/glm-5.2`: o Pi corta na PRIMEIRA barra (medido), então o id tem que
    manter a barra interna."""
    ms = pi_catalog.parse(SAIDA)
    assert ms[2]["id"] == "cline-pass/glm-5.2"
    assert ms[2]["provider"] == "clinepass"


def test_etiquetas_de_contexto_e_imagem():
    ms = pi_catalog.parse(SAIDA)
    assert ms[1]["context"] == "1.0M"
    assert ms[1]["images"] is True
    assert ms[0]["images"] is False


def test_linha_malformada_e_pulada_sem_derrubar_a_lista():
    assert len(pi_catalog.parse(SAIDA + "lixo\n")) == 3


def test_tabela_com_coluna_a_mais_nao_vira_lista_vazia(monkeypatch):
    """Mudança de formato do pi tem que virar erro visível, não seletor vazio dito completo."""
    nova = ("provider model context max-out thinking images cost\n"
            "cline deepseek/v4 1.0M 131.1K yes no 0.10\n")
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, nova, ""))
    pi_catalog._cache = None
    with pytest.raises(RuntimeError):
        pi_catalog.listar()


def test_falha_nao_fica_no_cache(monkeypatch):
    """O vazio não pode sobreviver ao conserto do pi."""
    pi_catalog._cache = None
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, "", ""))
    with pytest.raises(RuntimeError):
        pi_catalog.listar()
    assert pi_catalog._cache is None
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, SAIDA, ""))
    assert len(pi_catalog.listar()) == 3
