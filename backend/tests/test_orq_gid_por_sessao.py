"""Grupo sem sidecar de pareamento: o gid sai da tabela do regras-<gid>.md."""
import os

import pytest

from app import orq_papeis as op, pair


@pytest.fixture(autouse=True)
def _pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def _regras(gid: str, linhas: str) -> None:
    op.regras_path(gid).write_text(
        "# Regras\n\n## Quem é quem\n\n| papel | sessão | provider | conta | modelo | esforço |\n"
        "|---|---|---|---|---|---|\n" + linhas, encoding="utf-8")


def test_acha_por_nome_exato_e_por_glob():
    _regras("aa11", "| árbitro | pm1-arb | claude | padrao | opus | high |\n"
                    "| executor | pm1-t* | claude | 200-01 | opus | medium |\n")
    assert op.gid_por_sessao("pm1-arb") == "aa11"
    assert op.gid_por_sessao("pm1-t9") == "aa11"
    assert op.gid_por_sessao("pm2-arb") is None


def test_dois_contratos_casando_vence_o_mais_recente():
    _regras("velho", "| executor | x* | claude | padrao | opus | high |\n")
    _regras("novo", "| executor | x* | claude | padrao | opus | high |\n")
    p = op.regras_path("velho")
    os.utime(p, (p.stat().st_atime, p.stat().st_mtime - 100))
    assert op.gid_por_sessao("x1") == "novo"


def test_arquivo_sem_tabela_nao_casa():
    op.regras_path("prosa").write_text("| Papel | Sessão | Conta |\n|---|---|---|\n| árbitro | s1 | c |\n",
                                       encoding="utf-8")
    assert op.gid_por_sessao("s1") is None
