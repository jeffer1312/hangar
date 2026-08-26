"""orq_papeis: tabela de papéis do regras-<gid>.md, casamento com sessão viva, e a faxina do
regras- no pair (leave/merge)."""
import pytest

from app import orq_papeis as op, pair
from app.models import SessionInfo


@pytest.fixture(autouse=True)
def _pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


REGRAS = """# Regras — PM-1

## Quem é quem

| papel | sessão | provider | conta | modelo | esforço |
|---|---|---|---|---|---|
| árbitro | pm1-arbitro | claude | claude-200-3 | opus[1m] | high |
| executor | pm1-t* | Claude | 200-01 | opus[1m] | medium |
| revisão final | pm1-final | claude | claude-200-3 | opus[1m] | - |

## Gates
"""


def test_ler_e_escrever_papel():
    ps = op.ler(REGRAS)
    assert [p.papel for p in ps] == ["árbitro", "executor", "revisão final"]
    assert ps[0].e_arbitro() and not ps[1].e_arbitro()
    assert ps[1].provider == "claude" and ps[2].esforco == ""
    novo = op.escrever_papel(REGRAS, op.Papel("executor", "pm1-t*", "kimi", "apikey", "apikey/k3", "high"))
    assert op.ler(novo)[1] == op.Papel("executor", "pm1-t*", "kimi", "apikey", "apikey/k3", "high")
    assert "## Gates" in novo
    with pytest.raises(ValueError):
        op.escrever_papel(REGRAS, op.Papel("x|y", "s", "claude", "c", "m", "e"))


def test_regras_path():
    assert op.regras_path("ab12").name == "regras-ab12.md"
    assert op.regras_path("ab12").parent == pair._pair_dir()


def _s(name, ts):
    return SessionInfo(name=name, last_activity=ts)


def test_casar_viva_exato_e_glob():
    sess = [_s("pm1-arbitro", 1), _s("pm1-t3", 10), _s("pm1-t9", 50), _s("pm1-t5", 30), _s("outra", 99)]
    assert op.casar_viva(op.Papel("árbitro", "pm1-arbitro", "", "", "", ""), sess) == "pm1-arbitro"
    assert op.casar_viva(op.Papel("executor", "pm1-t*", "", "", "", ""), sess) == "pm1-t9"
    assert op.casar_viva(op.Papel("x", "pm1-final", "", "", "", ""), sess) is None
    assert op.casar_viva(op.Papel("x", "", "", "", "", ""), sess) is None


def test_leave_apaga_regras_junto_com_grupo():
    pair.join("a", "b", "t")
    gid = pair.PairLink("a").get()["gid"]
    (pair._pair_dir() / f"grupo-{gid}.md").write_text("g")
    op.regras_path(gid).write_text("r")
    pair.leave("a")
    assert not (pair._pair_dir() / f"grupo-{gid}.md").exists()
    assert not op.regras_path(gid).exists()


def test_merge_funde_regras_e_apaga_o_orfao():
    pair.join("a", "b", "t")
    pair.join("c", "d", "t")
    ga, gc = pair.PairLink("a").get()["gid"], pair.PairLink("c").get()["gid"]
    op.regras_path(ga).write_text("regras A")
    op.regras_path(gc).write_text("regras C")
    pair.join("a", "c", "t")
    vivo = pair.PairLink("a").get()["gid"]
    perdedor = gc if vivo == ga else ga
    assert not op.regras_path(perdedor).exists()
    texto = op.regras_path(vivo).read_text()
    assert "regras A" in texto and "regras C" in texto
