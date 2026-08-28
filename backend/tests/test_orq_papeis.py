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


def test_rodizio_varias_linhas_no_mesmo_papel():
    """Um papel com `vez` ocupa uma linha por conta, e gravar a segunda NÃO sobrescreve a primeira
    (a chave da linha passa a ser papel+vez). O contrato é promovido pro formato de 7 colunas na
    primeira gravação com vez, e os papéis que já existiam sobrevivem à promoção."""
    t = op.escrever_papel(REGRAS, op.Papel("revisor", "pm1-rev*", "claude", "200-01", "opus", "high", "1"))
    t = op.escrever_papel(t, op.Papel("revisor", "pm1-rev*", "pi", "codex", "luna", "max", "2"))
    t = op.escrever_papel(t, op.Papel("revisor", "pm1-rev*", "kimi", "apikey", "k3", "", "3"))
    revisores = [p for p in op.ler(t) if p.papel == "revisor"]
    assert [p.vez for p in revisores] == ["1", "2", "3"]
    assert [p.conta for p in revisores] == ["200-01", "codex", "apikey"]
    # Promoveu o formato sem perder quem já estava lá, nem o resto do arquivo.
    assert [p.papel for p in op.ler(t)][:3] == ["árbitro", "executor", "revisão final"]
    assert "## Gates" in t and op.tem_coluna_vez(t)
    # A promoção é NO LUGAR: um título só, e a tabela continua ANTES das seções escritas à mão.
    # A primeira versão apagava a tabela e deixava `trocar_linha` recriá-la, o que deixava o título
    # órfão e punha a tabela no fim do arquivo, depois do `## Gates`.
    assert t.count("## Quem é quem") == 1, "a promoção duplicou o título da seção"
    assert t.index("## Quem é quem") < t.index("## Gates"), "a tabela saiu do lugar"
    # Regravar a vez 2 troca no lugar, não duplica.
    t2 = op.escrever_papel(t, op.Papel("revisor", "pm1-rev*", "pi", "codex", "luna", "low", "2"))
    revisores2 = [p for p in op.ler(t2) if p.papel == "revisor"]
    assert len(revisores2) == 3 and revisores2[1].esforco == "low"


def test_contrato_sem_rodizio_nao_ganha_a_coluna():
    """Quem nunca usou rodízio não pode ter o arquivo mexido: são contratos de trabalhos em curso."""
    t = op.escrever_papel(REGRAS, op.Papel("executor", "pm1-t*", "kimi", "apikey", "apikey/k3", "high"))
    assert not op.tem_coluna_vez(t)
    assert "| vez |" not in t


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
