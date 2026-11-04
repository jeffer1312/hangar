"""Grupo antes do vigia: somente os vínculos reais de uma execução viva."""
import json

import pytest

from pathlib import Path
from app import pair
from orq_contract_client import contexts as orq_context, roles as op, runs


@pytest.fixture(autouse=True)
def _pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setenv("HOME",str(tmp_path))
    root=tmp_path/".hangar/orq"
    root.mkdir(parents=True)
    for name in ("arb","t1","review1","t2","pm1-arb","pm1-t9","pm2-arb","x1","s1"):
        _identity(name,"identity:"+name)
    return tmp_path


def _identity(name,key):
    d=Path.home()/".hangar/claude-headless"
    d.mkdir(parents=True,exist_ok=True)
    (d/f"{name}.json").write_text(json.dumps({"key":key}))


def _regras(gid: str, linhas: str) -> None:
    op.regras_path(gid).write_text(
        "# Regras\n\n## Quem é quem\n\n| papel | sessão | provider | conta | modelo | esforço |\n"
        "|---|---|---|---|---|---|\n" + linhas, encoding="utf-8")


def test_contrato_antigo_por_nome_ou_glob_nao_vincula():
    _regras("aa11", "| árbitro | pm1-arb | claude | padrao | opus | high |\n"
                    "| executor | pm1-t* | claude | 200-01 | opus | medium |\n")
    assert op.gid_por_sessao("pm1-arb") is None
    assert op.gid_por_sessao("pm1-t9") is None
    assert op.gid_por_sessao("pm2-arb") is None


def test_contratos_recentes_nao_definem_time():
    _regras("velho", "| executor | x* | claude | padrao | opus | high |\n")
    _regras("novo", "| executor | x* | claude | padrao | opus | high |\n")
    assert op.gid_por_sessao("x1") is None


def test_arquivo_sem_tabela_nao_casa():
    op.regras_path("prosa").write_text("| Papel | Sessão | Conta |\n|---|---|---|\n| árbitro | s1 | c |\n",
                                       encoding="utf-8")
    assert op.gid_por_sessao("s1") is None


def _run(events):
    d = runs.root() / "current"
    d.mkdir(exist_ok=True)
    (d / "orq.json").write_text(json.dumps({"auto": True, "arbiter": "arb",
                                           "arbiter_identity": "claude:identity:arb"}), encoding="utf-8")
    (d / "eventos.jsonl").write_text("\n".join(json.dumps(e) for e in events), encoding="utf-8")


def test_registro_real_preservado_antes_do_vigia():
    _run([{"tipo": "execucao_inicio", "gid": "real"},
          {"tipo": "task_inicio", "task": 1, "executor": "t1", "par": "review1",
           "session_identities": {"t1": "claude:identity:t1", "review1": "claude:identity:review1"}}])
    assert op.gid_por_sessao("arb") == op.gid_por_sessao("t1") == op.gid_por_sessao("review1") == "real"
    assert op.gid_por_sessao("t2") is None


def test_nome_recriado_na_execucao_viva_nao_herda_time(monkeypatch):
    _run([{"tipo": "execucao_inicio", "gid": "real"},
          {"tipo": "task_inicio", "task": 1, "executor": "t1", "par": "review1",
           "session_identities": {"t1": "claude:identity:t1", "review1": "claude:identity:review1"}}])
    _identity("t1","new-life:t1")
    _identity("arb","new-life:arb")
    assert op.gid_por_sessao("t1") is None
    assert op.gid_por_sessao("arb") is None


def test_registro_legado_sem_identidade_nao_e_prova():
    _run([{"tipo": "execucao_inicio", "gid": "real"},
          {"tipo": "task_inicio", "task": 1, "executor": "t1", "par": "review1"}])
    assert op.gid_por_sessao("t1") is None
    assert op.gid_por_sessao("review1") is None


def test_execucao_encerrada_nao_vincula_nome_recriado():
    _run([{"tipo": "execucao_inicio", "gid": "old"},
          {"tipo": "task_inicio", "task": 1, "executor": "t1", "par": "review1"},
          {"tipo": "execucao_fim", "resultado": "concluido"}])
    assert op.gid_por_sessao("arb") is None
    assert op.gid_por_sessao("t1") is None
