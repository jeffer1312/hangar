"""diag: o diário de uso que a pessoa baixa e manda pra quem mantém o app.

Tudo contra um CLAUDE_CONFIG_DIR falso — nenhum caso aqui pode escrever no ~/.claude de verdade.
"""
import json
from datetime import date, datetime, timedelta, timezone

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


def test_cabecalho_traz_o_commit_e_o_sistema_do_servidor():
    # O que o repositório NÃO conta: de qual commit veio a máquina que reportou. Sem isso a primeira
    # pergunta da análise ("isso já foi corrigido?") não tem resposta.
    diag.anotar_da_tela([{"evento": "x"}])
    cab = json.loads(diag.ler_tudo().splitlines()[0])
    assert cab["evento"] == "diag.formato"
    assert "backend" in cab and "so_servidor" in cab
    assert cab["dias_guardados"] == diag.DIAS_GUARDADOS
    assert "chave de API" in cab["nao_contem"]


def test_registrar_carimba_o_id_do_pedido_em_curso():
    # É o que liga a linha da tela ("POST /select devolveu 409") à do servidor ("o cursor não
    # convergiu"). Sem o id, amarrar as duas depende de comparar horário — que empata com duas
    # telas abertas.
    token = diag.req_atual.set("abc123-7")
    try:
        diag.registrar("opcao.nao_convergiu", "erro", sessao="pmw")
    finally:
        diag.req_atual.reset(token)
    l = _linhas()[0]
    assert l["req"] == "abc123-7" and l["origem"] == "servidor"


def test_sem_pedido_em_curso_nao_inventa_id():
    diag.registrar("qualquer")
    assert "req" not in _linhas()[0]


def test_ultimas_sem_arquivo_nenhum():
    assert diag.ultimas() == []


def test_resumo_sem_pasta_nenhuma():
    assert diag.resumo() == {"dias": 0, "bytes": 0, "arquivos": [],
                             "dias_guardados": diag.DIAS_GUARDADOS}
    # O download nunca vem vazio de verdade: a primeira linha é o cabeçalho com o commit do backend
    # e o sistema, que é o que o repositório sozinho não conta.
    linhas = diag.ler_tudo().splitlines()
    assert len(linhas) == 1
    assert json.loads(linhas[0])["evento"] == "diag.formato"


def test_horario_da_tela_vence_o_do_envio():
    # A tela agrupa o lote por até 4s antes de mandar. Sem isto, dois eventos separados por
    # segundos ficavam com o MESMO horário — o da chegada — e a ordem sumia do arquivo.
    do_evento = (datetime.now().astimezone() - timedelta(seconds=3)).isoformat(timespec="milliseconds")
    diag.anotar_da_tela([{"evento": "sse.abrir", "ts": do_evento}])
    assert datetime.fromisoformat(_linhas()[0]["ts"]) == datetime.fromisoformat(do_evento)


def test_horario_em_utc_vira_o_fuso_do_servidor():
    # O front manda `toISOString()`, que é UTC com "Z"; o arquivo é todo no fuso local.
    agora = datetime.now().astimezone().replace(microsecond=0)
    diag.anotar_da_tela([{"evento": "sse.abrir",
                          "ts": agora.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")}])
    gravado = _linhas()[0]["ts"]
    assert datetime.fromisoformat(gravado) == agora
    assert gravado == agora.isoformat(timespec="milliseconds")   # já no fuso de cá, não em Z


@pytest.mark.parametrize("ruim", ["ontem as 3", "2026-13-45T99:99", "1787749451"])
def test_horario_impossivel_cai_no_do_envio(ruim):
    diag.anotar_da_tela([{"evento": "sse.abrir", "ts": ruim}])
    # Nem perde a linha nem fica sem horário: sobra o do envio, que é o comportamento de antes.
    assert datetime.fromisoformat(_linhas()[0]["ts"]).date() == date.today()


def test_relogio_do_aparelho_em_outro_dia_nao_datila_linha_fora_do_arquivo():
    # O arquivo é UM POR DIA. Um aparelho com a data errada gravaria, dentro do arquivo de hoje,
    # linha datada de outro dia — aí o horário do envio é o menos errado dos dois.
    outro = (datetime.now().astimezone() - timedelta(days=2)).isoformat(timespec="milliseconds")
    diag.anotar_da_tela([{"evento": "sse.abrir", "ts": outro}])
    assert datetime.fromisoformat(_linhas()[0]["ts"]).date() == date.today()


def test_evento_do_proprio_backend_continua_com_o_horario_da_chegada():
    diag.registrar("tmux.mudo", nivel="erro")
    assert datetime.fromisoformat(_linhas()[0]["ts"]).date() == date.today()
