"""orq_md: tabela de cabeçalho fixo dentro de prosa alheia — ler, trocar linha no lugar, trocar
seção, e a guarda de célula suja."""
import pytest

from app import orq_md

CAB = ("papel", "sessão", "provider", "conta", "modelo", "esforço")

CONTRATO = """> Sessões deste grupo: leiam a página do seu papel.

# Regras do grupo — PM-1

Plano: `docs/x.md`.

## Quem é quem

| Papel | Sessão | Provider | Conta | Modelo | Esforço |
|---|---|---|---|---|---|
| árbitro | `pm1-arbitro` | claude | **claude-200-3** | `opus[1m]` | high |
| executor | `pm1-t*` | claude | 200-01 | opus[1m] | medium |

Aviso de grupo contradizendo esta tabela: vale a tabela.

```
| papel | sessão | provider | conta | modelo | esforço |
|---|---|---|---|---|---|
| exemplo | x | claude | y | z | low |
```

## Gates

- nada aqui muda.
"""


def test_le_tabela_normalizando_cabecalho_e_enfeites():
    linhas = orq_md.ler_tabela(CONTRATO, CAB)
    assert [l["papel"] for l in linhas] == ["árbitro", "executor"]
    assert linhas[0]["conta"] == "claude-200-3"      # negrito removido
    assert linhas[0]["sessão"] == "pm1-arbitro"      # backtick removido
    assert linhas[1]["sessão"] == "pm1-t*"


def test_tabela_dentro_de_fence_e_ignorada():
    so_fence = "```\n| papel | sessão | provider | conta | modelo | esforço |\n|---|\n| a | b | c | d | e | f |\n```\n"
    assert orq_md.ler_tabela(so_fence, CAB) == []


def test_troca_linha_no_lugar_e_preserva_o_resto():
    novo = orq_md.trocar_linha(CONTRATO, CAB, "executor", {
        "papel": "executor", "sessão": "pm1-t*", "provider": "claude",
        "conta": "200-01", "modelo": "opus[1m]", "esforço": "high"}, "Quem é quem")
    assert "| executor | pm1-t* | claude | 200-01 | opus[1m] | high |" in novo
    assert "| executor | `pm1-t*` | claude | 200-01 | opus[1m] | medium |" not in novo
    assert "## Gates" in novo and "vale a tabela" in novo and "| exemplo | x |" in novo
    assert orq_md.ler_tabela(novo, CAB)[1]["esforço"] == "high"


def test_papel_novo_entra_no_fim_da_tabela():
    novo = orq_md.trocar_linha(CONTRATO, CAB, "revisor", {
        "papel": "revisor", "sessão": "pm1-rev", "provider": "kimi",
        "conta": "apikey", "modelo": "apikey/k3", "esforço": "high"}, "Quem é quem")
    papeis = [l["papel"] for l in orq_md.ler_tabela(novo, CAB)]
    assert papeis == ["árbitro", "executor", "revisor"]
    # entrou antes da prosa que segue a tabela
    assert novo.index("| revisor |") < novo.index("Aviso de grupo")


def test_tabela_ausente_nasce_sob_o_titulo():
    novo = orq_md.trocar_linha("# só prosa\n", CAB, "árbitro", {
        "papel": "árbitro", "sessão": "a", "provider": "claude", "conta": "padrao",
        "modelo": "opus", "esforço": "high"}, "Quem é quem")
    assert novo.startswith("# só prosa\n\n## Quem é quem\n\n| papel |")
    assert orq_md.ler_tabela(novo, CAB)[0]["conta"] == "padrao"


def test_celula_vazia_vira_traco_e_volta_vazia():
    novo = orq_md.trocar_linha("", CAB, "x", {"papel": "x"}, "T")
    assert "| x | - | - | - | - | - |" in novo
    assert orq_md.ler_tabela(novo, CAB)[0]["modelo"] == ""


@pytest.mark.parametrize("sujo", ["a|b", "a\nb", "a\rb"])
def test_celula_suja_e_recusada(sujo):
    with pytest.raises(ValueError):
        orq_md.trocar_linha("", CAB, "x", {"papel": sujo}, "T")


def test_remover_linha():
    novo = orq_md.remover_linha(CONTRATO, CAB, "executor")
    assert [l["papel"] for l in orq_md.ler_tabela(novo, CAB)] == ["árbitro"]
    assert orq_md.remover_linha("nada", CAB, "x") == "nada"


def test_trocar_secao_substitui_ate_o_proximo_titulo():
    novo = orq_md.trocar_secao(CONTRATO, "Gates", "- só isto")
    assert "- nada aqui muda." not in novo
    assert novo.rstrip().endswith("## Gates\n\n- só isto")
    assert "## Quem é quem" in novo
    # ausente → anexa
    assert orq_md.trocar_secao("x\n", "Nova", "corpo").endswith("## Nova\n\ncorpo\n")


def test_gravar_detecta_conflito_por_mtime(tmp_path):
    p = tmp_path / "r.md"
    mt = orq_md.gravar(p, "a\n")
    assert p.read_text() == "a\n"
    orq_md.gravar(p, "b\n", mt)
    import os
    os.utime(p, (1, 1))
    with pytest.raises(orq_md.Conflito):
        orq_md.gravar(p, "c\n", mt)
    assert p.read_text() == "b\n"
    assert not list(tmp_path.glob(".r.md.*.tmp"))
