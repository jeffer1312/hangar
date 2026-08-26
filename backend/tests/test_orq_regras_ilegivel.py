"""Um regras-*.md ilegível não pode derrubar a busca de grupo das outras sessões."""
import pytest

from app import orq_papeis as op, pair


@pytest.fixture(autouse=True)
def _pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")


def test_regras_com_bytes_invalidos_e_pulado():
    op.regras_path("ruim").write_bytes(b"\xff\xfe| papel | sess\xe3o |\n")
    op.regras_path("bom").write_text(
        "## Quem é quem\n\n| papel | sessão | provider | conta | modelo | esforço |\n"
        "|---|---|---|---|---|---|\n| árbitro | s1 | claude | padrao | opus | high |\n", encoding="utf-8")
    assert op.gid_por_sessao("s1") == "bom"
