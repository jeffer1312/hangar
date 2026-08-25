"""diag: o diário de uso que a pessoa baixa e manda pra quem mantém o app.

Tudo contra um CLAUDE_CONFIG_DIR falso — nenhum caso aqui pode escrever no ~/.claude de verdade.
"""
import json
from datetime import date, timedelta

import pytest

from app import diag


@pytest.fixture(autouse=True)
def config_falso(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    return tmp_path


def _linhas(quando: date | None = None) -> list[dict]:
    p = diag.caminho_do_dia(quando)
    return [json.loads(x) for x in p.read_text(encoding="utf-8").splitlines() if x.strip()]


def test_grava_o_que_conhece_e_descarta_o_resto():
    n = diag.anotar_da_tela([{
        "evento": "opcao.tocar", "nivel": "erro", "tela": "chat", "sessao": "pmw",
        "codigo": "409", "ms": 120,
        # Campos fora da lista: o que protege o arquivo de virar despejo do cliente.
        "texto_da_mensagem": "conteudo que nao pode entrar",
        "api_key": "sk-segredo",
    }])
    assert n == 1
    linha = _linhas()[0]
    assert linha["evento"] == "opcao.tocar" and linha["nivel"] == "erro"
    assert linha["codigo"] == "409" and linha["ms"] == 120
    assert "texto_da_mensagem" not in linha and "api_key" not in linha
    assert linha["origem"] == "tela"
    assert "ts" in linha


def test_conteudo_de_conversa_nao_entra_por_campo_nenhum():
    # A trava é a lista de campos, não uma varredura de texto. Este caso existe pra que remover a
    # lista (ou trocá-la por "aceita tudo") quebre um teste em vez de vazar calado.
    diag.anotar_da_tela([{"evento": "msg.enviar", "prompt": "segredo", "resposta": "segredo",
                          "conteudo": "segredo", "arquivo": "/home/eu/projeto/x.py"}])
    bruto = diag.caminho_do_dia().read_text(encoding="utf-8")
    assert "segredo" not in bruto and "/home/eu" not in bruto


def test_evento_sem_verbo_nao_vira_linha():
    assert diag.anotar_da_tela([{"tela": "chat"}, {"evento": "   "}, {"evento": 3}, "texto"]) == 0
    assert not diag.caminho_do_dia().exists()


def test_nivel_desconhecido_vira_ok_em_vez_de_perder_o_evento():
    diag.anotar_da_tela([{"evento": "x", "nivel": "catastrofico"}, {"evento": "y"}])
    assert [l["nivel"] for l in _linhas()] == ["ok", "ok"]


def test_booleano_nao_passa_por_numero():
    # Em Python `True` É um int; sem a checagem explícita, um `ms: true` viraria 1 milissegundo.
    diag.anotar_da_tela([{"evento": "x", "ms": True}])
    assert "ms" not in _linhas()[0]


def test_plataforma_entra_no_app_abriu():
    diag.anotar_da_tela([{"evento": "app.abriu", "so": "Windows", "navegador": "Chrome 141",
                          "versao": "1.2.3", "vista": "desktop", "tela_px": "1920x1032",
                          "cli": "a1b2c3"}])
    l = _linhas()[0]
    assert l["so"] == "Windows" and l["navegador"] == "Chrome 141"
    assert l["vista"] == "desktop" and l["tela_px"] == "1920x1032" and l["cli"] == "a1b2c3"


def test_backend_grava_no_mesmo_arquivo_com_origem_propria():
    diag.anotar_da_tela([{"evento": "da.tela"}])
    diag.registrar("lista.mux_indisponivel", "erro", detalhe="timeout")
    origens = [(l["evento"], l["origem"]) for l in _linhas()]
    assert origens == [("da.tela", "tela"), ("lista.mux_indisponivel", "servidor")]


def test_registrar_nunca_levanta(monkeypatch):
    # Um diário que derruba o pedido que ele deveria descrever é pior que diário nenhum.
    monkeypatch.setattr(diag, "_escrever", lambda _l: (_ for _ in ()).throw(OSError("disco cheio")))
    diag.registrar("qualquer")   # não levanta


def test_guarda_uma_semana_e_apaga_o_mais_velho(config_falso):
    base = config_falso / ".hangar-diag"
    base.mkdir(parents=True)
    hoje = date.today()
    velho = hoje - timedelta(days=diag.DIAS_GUARDADOS)      # fora da janela
    novo = hoje - timedelta(days=diag.DIAS_GUARDADOS - 2)   # dentro
    for d in (velho, novo):
        diag.caminho_do_dia(d).write_text('{"evento":"antigo"}\n', encoding="utf-8")

    diag.anotar_da_tela([{"evento": "hoje"}])   # a poda roda no append

    nomes = [p.name for p in diag.arquivos()]
    assert diag.caminho_do_dia(velho).name not in nomes
    assert diag.caminho_do_dia(novo).name in nomes
    assert diag.caminho_do_dia(hoje).name in nomes


def test_dia_por_arquivo_separado():
    ontem = date.today() - timedelta(days=1)
    diag.caminho_do_dia().parent.mkdir(parents=True, exist_ok=True)
    diag.caminho_do_dia(ontem).write_text('{"evento":"de-ontem"}\n', encoding="utf-8")
    diag.anotar_da_tela([{"evento": "de-hoje"}])
    # ler_tudo concatena do mais ANTIGO pro mais novo — é a ordem de quem vai ler o arquivo.
    tudo = diag.ler_tudo()
    assert tudo.index("de-ontem") < tudo.index("de-hoje")


def test_teto_do_dia_para_de_gravar_e_diz_que_parou(monkeypatch):
    monkeypatch.setattr(diag, "_TETO_DIA", 400)
    for i in range(40):
        diag.anotar_da_tela([{"evento": f"e{i}", "detalhe": "x" * 100}])
    bruto = diag.caminho_do_dia().read_text(encoding="utf-8")
    assert "diag.teto" in bruto          # um arquivo que só cessa parece máquina desligada
    assert len(bruto) < 4000             # e parou de verdade, não só avisou


def test_lote_gigante_e_cortado():
    assert diag.anotar_da_tela([{"evento": "x"}] * 500) == diag._TETO_LOTE


def test_resumo_conta_dias_e_bytes():
    diag.anotar_da_tela([{"evento": "x"}])
    r = diag.resumo()
    assert r["dias"] == 1 and r["bytes"] > 0
    assert r["dias_guardados"] == diag.DIAS_GUARDADOS
    assert r["arquivos"] == [diag.caminho_do_dia().name]


def test_ultimas_vem_das_mais_novas_e_atravessa_os_dias():
    ontem = date.today() - timedelta(days=1)
    diag.caminho_do_dia().parent.mkdir(parents=True, exist_ok=True)
    diag.caminho_do_dia(ontem).write_text(
        '{"ts":"x","evento":"velho-1"}\n{"ts":"x","evento":"velho-2"}\n', encoding="utf-8")
    diag.anotar_da_tela([{"evento": "novo-1"}, {"evento": "novo-2"}])
    # Mais novas primeiro; passa pro dia anterior só depois de esgotar o de hoje.
    assert [l["evento"] for l in diag.ultimas(4)] == ["novo-2", "novo-1", "velho-2", "velho-1"]
    assert [l["evento"] for l in diag.ultimas(1)] == ["novo-2"]


def test_ultimas_pula_linha_corrompida_em_vez_de_derrubar():
    # Queda no meio de um append deixa meia linha; a tela não pode ficar sem prévia por causa disso.
    diag.caminho_do_dia().parent.mkdir(parents=True, exist_ok=True)
    diag.caminho_do_dia().write_text(
        '{"ts":"x","evento":"bom"}\n{"ts":"x","evento":"pela-met\n', encoding="utf-8")
    assert [l["evento"] for l in diag.ultimas()] == ["bom"]


def test_ultimas_sem_arquivo_nenhum():
    assert diag.ultimas() == []


def test_resumo_sem_pasta_nenhuma():
    assert diag.resumo() == {"dias": 0, "bytes": 0, "arquivos": [],
                             "dias_guardados": diag.DIAS_GUARDADOS}
    assert diag.ler_tudo() == ""
