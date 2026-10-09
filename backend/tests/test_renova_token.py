"""Pasta confiada da conta, onde a janela escondida da renovação (operada pelo Rust) abre o `claude`.

Abrir numa pasta não confiada trava na pergunta de confiança; por isso só vale caminho aceito
(`hasTrustDialogAccepted` literalmente `true`) e que ainda existe.
"""
import json
from pathlib import Path

from app import renova_token


def _conta(tmp_path: Path, nome: str) -> Path:
    d = tmp_path / f".claude-{nome}"
    d.mkdir(parents=True)
    return d


def test_pasta_confiada_devolve_a_primeira_existente(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "confia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(tmp_path / "sumiu"): {"hasTrustDialogAccepted": True},
        str(repo): {"hasTrustDialogAccepted": True},
    }}), encoding="utf-8")
    assert renova_token.pasta_confiada(d) == repo


def test_pasta_nao_confiada_nao_conta(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "naoconfia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(repo): {"hasTrustDialogAccepted": False},
    }}), encoding="utf-8")
    assert renova_token.pasta_confiada(d) is None


def test_claude_json_estragado_nao_levanta(tmp_path):
    d = _conta(tmp_path, "json-ruim")
    (d / ".claude.json").write_text("{ nao é json", encoding="utf-8")
    assert renova_token.pasta_confiada(d) is None
