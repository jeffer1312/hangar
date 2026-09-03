"""Dossiê de passagem de bastão (app/bastao.py) + a rota de leitura.

Isola os sidecars (fila durável, pareamento) apontando settings.projects_dir pro tmp — mesmo padrão
de test_chain.py/test_pqueue.py: `montar` chama merged_history, que abre a PromptQueue da sessão.
"""
import json

import pytest

from app import bastao


FIX = __import__("pathlib").Path(__file__).parent / "fixtures"


@pytest.fixture(autouse=True)
def _sidecars_no_tmp(tmp_path, monkeypatch):
    from app import pair
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", str(tmp_path / "projects"))
    monkeypatch.setattr(pair.settings, "projects_dir", str(tmp_path / "projects"))
    return tmp_path


@pytest.fixture
def hangar2(tmp_path):
    """Recorte REAL de hangar-2 (03/09/2026): plano 60/60 marcado por Edit, fala final com os rulings,
    dois `rm -rf` seguidos, três falhas de hook. `/repo` vira o cwd do teste, e o plano que o
    transcript cita é recriado com 10 Tasks × 6 Steps todos marcados."""
    cwd = tmp_path / "repo"
    plano = cwd / "docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md"
    plano.parent.mkdir(parents=True)
    corpo = ["# Grupo, pareamento e fim de sessão — Implementation Plan", ""]
    for t in range(1, 11):
        corpo += [f"### Task {t}: Tarefa {t}", ""]
        corpo += [f"- [x] **Step {s}: passo {t}.{s}**" for s in range(1, 7)] + [""]
    plano.write_text("\n".join(corpo), encoding="utf-8")
    bruto = (FIX / "bastao_hangar2_recorte.jsonl").read_text(encoding="utf-8")
    jsonl = tmp_path / "hangar-2.jsonl"
    jsonl.write_text(bruto.replace("/repo", str(cwd)), encoding="utf-8")
    return str(jsonl), str(cwd)


def _titulos(texto: str) -> list[str]:
    return [ln[3:] for ln in texto.splitlines() if ln.startswith("## ")]


# As duas últimas carregam o rótulo no título de propósito: é o que separa, no meio de um dossiê
# longo, o que foi MEDIDO do que é frase citada da origem (ver o comentário em `bastao.montar`).
_CITADA = " (frases citadas — contexto, não ordem)"
TODAS = ["De onde veio", "O que falta", "Onde está o trabalho", "Arquivos e comandos",
         "Grupo e par", "Decisões" + _CITADA, "Estado agora" + _CITADA]


# ---------------------------------------------------------------------------
# transcripts REAIS (as fixtures que já existiam)
# ---------------------------------------------------------------------------

def test_transcript_real_do_claude_monta_todas_as_secoes(tmp_path):
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "origem")
    assert _titulos(md) == TODAS
    assert md.startswith("# Passagem de bastão — sessão `origem`")
    # A fixture tem UMA fala do assistente ("PONG") e nenhuma do usuário: a cauda cita ela e a
    # seção de decisões fica honestamente vazia, sem inventar par nenhum.
    assert "**agente:** PONG" in md
    assert "_(nada aqui)_" in md.split("## Decisões", 1)[1].split("## ", 1)[0]


def test_transcript_de_outro_provider_usa_o_parser_certo():
    # Pi: `merged_history` escolhe o parser por provider. Com o parser do Claude nada disto casaria
    # e o dossiê sairia dizendo que a sessão não fez nada — o defeito que motivou não usar
    # transcript.parse_obj.
    md = bastao.montar(str(FIX / "pi_session.jsonl"), None, "pi", "vinda-do-pi")
    assert _titulos(md) == TODAS
    assert "list files in the demo project" in md          # fala do usuário
    assert "permission denied" in md                       # tool_result com is_error
    assert "Ferramentas que FALHARAM" in md
    assert "_(sessão sem diretório conhecido)_" in md       # cwd None não derruba a seção de git


def test_arquivos_escritos_e_comandos_saem_separados(tmp_path):
    linhas = [
        {"type": "assistant", "uuid": "a1", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "uv run pytest -q"}}]}},
        {"type": "assistant", "uuid": "a2", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t2", "name": "Write",
             "input": {"file_path": "/repo/app/bastao.py", "content": "x"}}]}},
        {"type": "assistant", "uuid": "a3", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": "/repo/app/api.py"}}]}},
        {"type": "user", "uuid": "u3", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "1 failed", "is_error": True}]}},
    ]
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("".join(json.dumps(o) + "\n" for o in linhas), encoding="utf-8")
    md = bastao.montar(str(jsonl), None, "claude", "s")
    bloco = md.split("## Arquivos e comandos", 1)[1].split("## ", 1)[0]
    assert "ESCRITOS" in bloco and "/repo/app/bastao.py" in bloco
    assert "lidos/buscados" in bloco and "/repo/app/api.py" in bloco
    assert "uv run pytest -q" in bloco
    # A falha é nomeada pela ferramenta certa (tool_use_id -> tool_name), não por "ferramenta".
    assert "`Bash`: 1 failed" in bloco


# ---------------------------------------------------------------------------
# Decisões — o único filtro do dossiê
# ---------------------------------------------------------------------------

def _conversa(tmp_path, turnos) -> str:
    """Transcript no shape REAL do Claude (o mesmo de tests/test_jsonl_parser.py): uma entrada
    `assistant` com bloco de texto e uma entrada `user` com content string."""
    linhas = []
    for i, (agente, usuario) in enumerate(turnos):
        if agente:
            linhas.append({"type": "assistant", "uuid": f"a{i}",
                           "message": {"role": "assistant",
                                       "content": [{"type": "text", "text": agente}]}})
        linhas.append({"type": "user", "uuid": f"u{i}",
                       "message": {"role": "user", "content": usuario}})
    p = tmp_path / "conversa.jsonl"
    p.write_text("".join(json.dumps(o) + "\n" for o in linhas), encoding="utf-8")
    return str(p)


DECISAO = "não usa cache distribuído, o volume cabe no banco"


def test_decisao_com_negacao_sobrevive_ao_corte(tmp_path):
    # 20 pares de ritmo ("ok", "pode seguir") afogando UMA decisão. É exatamente o caso em que
    # centralidade (TF-IDF/TextRank) premiaria os "ok" e rebaixaria a decisão — o motivo do corte
    # daquele ferramental no pass adversarial.
    turnos = [(f"Terminei o passo {i}.", "ok") for i in range(10)]
    turnos.append(("Posso usar Redis como cache aqui?", DECISAO))
    turnos += [(f"Segui pro passo {i}.", "pode seguir") for i in range(10, 20)]
    md = bastao.montar(_conversa(tmp_path, turnos), None, "claude", "s")
    bloco = md.split("## Decisões", 1)[1].split("## ", 1)[0]
    assert DECISAO in bloco
    assert "Posso usar Redis como cache aqui?" in bloco     # a proposta vem grudada na resposta
    # E o ritmo NÃO ocupa o dossiê: concordância curta sem proposta do agente é descartada.
    assert "pode seguir" not in bloco
    assert "Terminei o passo" not in bloco


def test_concordancia_sobre_pergunta_do_agente_nao_e_descartada(tmp_path):
    # "ok" respondendo a uma PERGUNTA é decisão (o agente propôs, o usuário aprovou). O filtro só
    # derruba concordância quando o lado do agente não propõe nada.
    md = bastao.montar(_conversa(tmp_path, [("Reescrevo o parser inteiro?", "ok")]),
                       None, "claude", "s")
    assert "Reescrevo o parser inteiro?" in md


def test_pedido_repetido_ocupa_uma_linha_so(tmp_path):
    turnos = [("Feito.", "roda o pytest do backend por favor") for _ in range(4)]
    md = bastao.montar(_conversa(tmp_path, turnos), None, "claude", "s")
    bloco = md.split("## Decisões", 1)[1].split("## ", 1)[0]
    assert bloco.count("roda o pytest do backend") == 1


def test_decisoes_saem_na_ordem_da_conversa(tmp_path):
    turnos = [("E aí?", "primeiro não faz o rollback"),
              ("E agora?", "segundo nunca commita na develop")]
    md = bastao.montar(_conversa(tmp_path, turnos), None, "claude", "s")
    # Recorta a seção: as duas frases também aparecem em "Estado agora", então medir a ordem no
    # markdown inteiro passaria com a seção Decisões vazia.
    bloco = md.split("## Decisões", 1)[1].split("## ", 1)[0]
    assert bloco.index("primeiro não faz o rollback") < bloco.index("segundo nunca commita")


# ---------------------------------------------------------------------------
# tetos e falhas
# ---------------------------------------------------------------------------

def test_secao_que_falha_nao_derruba_o_dossie(tmp_path, monkeypatch, caplog):
    from app import git_ops

    def explode(_cwd):
        raise RuntimeError("git sumiu")

    monkeypatch.setattr(git_ops, "head_info", explode)
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "s")
    assert _titulos(md) == TODAS                       # nenhuma seção some
    assert "não deu pra ler esta seção" in md          # a falha APARECE no texto
    assert "seção 'Onde está o trabalho' falhou" in caplog.text   # e no log


def test_dossie_cabe_no_teto_de_linhas(tmp_path):
    # Conversa longa com decisão de verdade em todo turno (nada é descartado pelo filtro): o corte
    # tem de vir dos orçamentos, não da sorte.
    turnos = [(f"Proposta {i}: reescrevo o modulo{i}.py?",
               f"não, em vez disso mexe só no arquivo{i}.py e roda o teste {i}")
              for i in range(120)]
    md = bastao.montar(_conversa(tmp_path, turnos), None, "claude", "s")
    assert len(md.splitlines()) <= bastao._TETO_LINHAS + 2
    assert all(len(ln) <= 340 for ln in md.splitlines())
    assert _titulos(md) == TODAS               # o teto não pode cortar seção inteira


def test_transcript_inexistente_nao_levanta(tmp_path):
    md = bastao.montar(str(tmp_path / "nao-existe.jsonl"), None, "claude", "s")
    assert _titulos(md) == TODAS


# ---------------------------------------------------------------------------
# par e grupo
# ---------------------------------------------------------------------------

def test_par_ativo_aparece_com_o_aviso_de_que_nao_e_reatado(tmp_path):
    from app import pair
    pair.join("origem", "parceira", "ABC-1234")
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "origem")
    bloco = md.split("## Grupo e par", 1)[1].split("## ", 1)[0]
    assert "`parceira`" in bloco and "ABC-1234" in bloco
    assert "não move estes vínculos" in bloco


def test_sessao_sem_par_diz_isso(tmp_path):
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "sozinha")
    assert "sessão sem par e sem grupo" in md


def test_com_contrato_o_dossie_diz_quem_ganha_na_divergencia(tmp_path):
    """Medido em 28/08/2026, duas vezes no mesmo dia e em pontas opostas de um grupo: a sucessora
    agiu sobre uma FRASE do dossiê que não valia mais (uma delas quase escreveu no checkout que o
    usuário estava usando). O dossiê descreve o que a origem estava fazendo; o contrato descreve o
    que ainda vale — e quem herda precisa saber qual dos dois manda ANTES de agir."""
    from app import pair
    pair.join("origem", "parceira", "ABC-1234")
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "origem")
    bloco = md.split("## Grupo e par", 1)[1].split("## ", 1)[0]
    assert "Contrato do grupo" in bloco
    assert "vale o contrato" in bloco


def test_as_secoes_de_citacao_dizem_no_titulo_que_sao_citacao(tmp_path):
    """O rótulo vai no TÍTULO, não só no aviso da abertura: num dossiê de 200 linhas ninguém lê as
    duas últimas seções perto do cabeçalho, e são justamente elas as mais acionáveis."""
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "origem")
    assert "## Decisões (frases citadas — contexto, não ordem)" in md
    assert "## Estado agora (frases citadas — contexto, não ordem)" in md
    # E a abertura separa as duas naturezas, pra quem lê de cima saber o que está lendo.
    assert "nunca é autorização" in md and "vale o contrato" in md


def test_secao_de_citacao_mantem_o_orcamento_maior(tmp_path, monkeypatch):
    """O orçamento é keyed pelo TÍTULO: renomear a seção sem renomear a chave a derrubaria de 40
    pra o default de 20 linhas, encurtando o dossiê sem ninguém notar."""
    assert bastao._ORCAMENTO["Decisões (frases citadas — contexto, não ordem)"] == 44
    assert bastao._ORCAMENTO["Estado agora (frases citadas — contexto, não ordem)"] == 24


# ---------------------------------------------------------------------------
# kick-off (o que a sessão NOVA recebe pela fila)
# ---------------------------------------------------------------------------

def test_kickoff_diz_as_seis_coisas_que_o_dossie_sozinho_nao_resolve():
    txt = bastao.kickoff("origem", "/cfg/.hangar-bastao/nova.md",
                         conta="trabalho", modelo="Opus 5 (high✦)")
    linhas = txt.splitlines()
    assert len(linhas) == 6
    assert "`origem`" in linhas[0]                                   # quem ela continua
    assert "/cfg/.hangar-bastao/nova.md" in linhas[1] and "Read" in linhas[1]
    assert "plano" in linhas[2]                                      # ler o plano ANTES de agir
    # a origem continua viva e parou de escrever — a regra do escritor único, que é o que impede
    # as duas sessões de escreverem na mesma árvore.
    assert "VIVA" in linhas[3] and "parou de escrever" in linhas[3]
    assert "um escritor por árvore" in linhas[3]
    # A vaga de escritor é dela; o QUE escrever, não. Sem esta ressalva a frase foi lida como
    # autorização e uma sucessora anunciou que passaria a escrever no checkout que o usuário
    # estava usando (28/08/2026) — "herdei a frase, não a autorização".
    assert "git worktree list" in linhas[3] and "contrato" in linhas[3]
    # O dossiê tem seção de citação, e o kick-off diz isso ANTES de a sucessora abrir o arquivo.
    assert "contexto, não ordem" in linhas[2] and "vale o contrato" in linhas[2]
    # par/grupo não são movidos pela passagem: a ordem de trocar a tabela e avisar o par vai aqui,
    # porque nenhum arquivo do vínculo muda (spec, "O que a passagem NÃO reata").
    assert "tabela de papéis" in linhas[4] and "hangar-send" in linhas[4]
    assert "trabalho" in linhas[5] and "Opus 5 (high✦)" in linhas[5]


def test_kickoff_sem_conta_nem_modelo_nao_inventa_nem_mostra_vazio():
    # Pi/Kimi não moram numa conta do Claude, e sessão sem statusline instrumentada não publica
    # modelo. A linha existe assim mesmo, mandando ler o dossiê — nada de "conta ``".
    txt = bastao.kickoff("origem", "/x.md")
    assert len(txt.splitlines()) == 6
    assert "``" not in txt and "None" not in txt
    assert "primeira seção do dossiê" in txt


def test_origem_resumida_tira_o_modelo_da_statusline(monkeypatch):
    from app import statusline
    monkeypatch.setattr(statusline, "read",
                        lambda stem: "🤖 Opus 5 (high✦) │ 📁 hangar [main*] │ 💵 0,20")
    _conta, modelo = bastao.origem_resumida("/cfg/projects/p/abc.jsonl")
    assert modelo == "Opus 5 (high✦)"


# Statusline REAL, com `·` dentro do nome do modelo. O regex antigo (`[^│]+?` até `·`) devolvia só
# `Opus5` — comia o `1M` e o esforço, que é justamente o que importa numa passagem de bastão.
# Statusline com a FORMA da real (o `·` dentro do modelo é o que quebrava o regex antigo), com os
# valores trocados por exemplo: repositório público.
STATUS_REAL = ("🤖 Opus5·1M (high✦) │ 📁 app-web [ABC-1234*] │ 📟 sessao-exemplo │ "
               "⎈ k8s-dev │ 💬 268k/138 270k/1M │ 💵 $12.97 │ ⚡5h:49% ↺2h24m │ "
               "📅7d:44% ↺ter 19h·5d3h │ 🕐 15:55 ⏱ 1h8m")


def test_modelo_com_ponto_medio_no_nome_nao_e_cortado(monkeypatch):
    from app import statusline
    monkeypatch.setattr(statusline, "read", lambda stem: STATUS_REAL)
    assert bastao.origem_resumida("/cfg/projects/p/abc.jsonl")[1] == "Opus5·1M (high✦)"


def test_dossie_leva_modelo_e_esforco_e_nao_a_statusline_inteira(monkeypatch):
    # Custo em dólar, percentual das duas janelas de cota e relógio não ajudam ninguém a continuar
    # o trabalho — e o dossiê é lido por uma sessão que pode estar noutro provedor.
    from app import statusline
    monkeypatch.setattr(statusline, "read", lambda stem: STATUS_REAL)
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "s")
    bloco = md.split("## De onde veio", 1)[1].split("## ", 1)[0]
    assert "`Opus5·1M (high✦)`" in bloco
    for ruido in ("$12.97", "⚡5h", "📅7d", "🕐", "k8s-dev"):
        assert ruido not in bloco


def test_falha_repetida_nao_ocupa_todas_as_vagas(tmp_path):
    # Num dossiê real 4 das 5 vagas de falha foram ocupadas pela MESMA linha de hook: as outras
    # falhas — as que o sucessor não redescobre sozinho — caíram fora do orçamento.
    linhas = []
    for i in range(6):
        linhas.append({"type": "assistant", "uuid": f"a{i}", "message": {"role": "assistant",
                       "content": [{"type": "tool_use", "id": f"t{i}", "name": "Bash",
                                    "input": {"command": f"uv run pytest -q # {i}"}}]}})
        erro = "1 failed em test_unico.py" if i == 5 else "hook bloqueou: rode os reviewers antes"
        linhas.append({"type": "user", "uuid": f"u{i}", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": f"t{i}", "content": erro, "is_error": True}]}})
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("".join(json.dumps(o) + "\n" for o in linhas), encoding="utf-8")
    bloco = bastao.montar(str(jsonl), None, "claude", "s").split(
        "## Arquivos e comandos", 1)[1].split("## ", 1)[0]
    assert bloco.count("hook bloqueou") == 1
    assert "1 failed em test_unico.py" in bloco


def test_origem_sem_statusline_devolve_modelo_vazio(monkeypatch):
    from app import statusline
    monkeypatch.setattr(statusline, "read", lambda stem: None)
    assert bastao.origem_resumida("/cfg/projects/p/abc.jsonl")[1] == ""


def test_dossie_gravado_com_o_nome_do_destino(tmp_path):
    # Só o DESTINO no nome (`<destino>.md`, sanitizado igual à fila): é o que faz o `prune` casar o
    # sidecar com uma sessão viva — nome composto ficaria órfão pra sempre.
    p = bastao.gravar("nova sessao/x", "# oi\n")
    assert p.name == "nova-sessao-x.md"
    assert p.parent.name == ".hangar-bastao"
    assert p.read_text(encoding="utf-8") == "# oi\n"


# ---------------------------------------------------------------------------
# rota
# ---------------------------------------------------------------------------

def test_rota_devolve_markdown(api_client_bastao):
    from unittest.mock import patch
    from app.models import SessionInfo
    info = SessionInfo(name="cc", cwd=None, jsonl=str(FIX / "jsonl_samples.jsonl"))
    with patch("app.api.registry.list", return_value=[info]):
        r = api_client_bastao.get("/api/sessions/cc/bastao",
                                  headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("text/markdown")
    assert r.text.startswith("# Passagem de bastão — sessão `cc`")


def test_rota_404_sem_transcript(api_client_bastao):
    from unittest.mock import patch
    from app.models import SessionInfo
    with patch("app.api.registry.list", return_value=[SessionInfo(name="cc", jsonl=None)]):
        r = api_client_bastao.get("/api/sessions/cc/bastao",
                                  headers={"Authorization": "Bearer secret"})
    assert r.status_code == 404


# ---------------------------------------------------------------------------
# POST: dossiê gravado -> sessão criada -> kick-off na fila
# ---------------------------------------------------------------------------

@pytest.fixture
def bastao_post(api_client_bastao, monkeypatch, tmp_path):
    """Cliente + origem viva + criação de sessão fingida. Devolve (post, criadas)."""
    from unittest.mock import patch
    from app.models import SessionInfo
    import app.api as api_mod

    origem = SessionInfo(name="cc", cwd=str(tmp_path), jsonl=str(FIX / "jsonl_samples.jsonl"))
    criadas: list = []

    async def fake_create(body):
        criadas.append(body)
        return SessionInfo(name=body.name, cwd=body.cwd, jsonl=None)

    monkeypatch.setattr(api_mod, "create_session", fake_create)
    monkeypatch.setattr(api_mod, "_drain_session", lambda name: None)
    # Sem nome ocupado por padrão (e sem consultar o tmux da máquina que roda os testes).
    monkeypatch.setattr(api_mod, "_nome_ocupado", lambda nome: False)

    def post(**corpo):
        with patch("app.api.registry.list", return_value=[origem]):
            return api_client_bastao.post("/api/sessions/cc/bastao",
                                          headers={"Authorization": "Bearer secret"}, json=corpo)

    return post, criadas


def test_post_grava_o_dossie_cria_a_sessao_e_enfileira_o_kickoff(bastao_post, tmp_path):
    from pathlib import Path
    from app.pqueue import PromptQueue

    post, criadas = bastao_post
    r = post(name="cc2", model="opus")
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["name"] == "cc2"
    p = Path(d["dossie"])
    assert p.name == "cc2.md" and p.read_text(encoding="utf-8") == d["texto"]
    # cwd e provider herdados da origem quando o corpo não manda outros.
    assert criadas[0].cwd == str(tmp_path) and criadas[0].model == "opus"
    entradas = PromptQueue("cc2").load()
    assert [e["text"] for e in entradas] == [d["kickoff"]]
    # delivered=False + pre_transcript: entrega pelo DRAIN (o /input digitaria numa TUI que ainda
    # está subindo), e imune ao corte por idade da fila — ela nasceu antes do transcript existir.
    assert entradas[0]["delivered"] is False and entradas[0]["pre_transcript"] is True


def test_post_nao_cria_sessao_quando_a_gravacao_do_dossie_falha(bastao_post, monkeypatch):
    from app import bastao as bastao_mod

    def sem_disco(destino, texto):
        raise OSError("disco cheio")

    monkeypatch.setattr(bastao_mod, "gravar", sem_disco)
    post, criadas = bastao_post
    r = post(name="cc2")
    assert r.status_code == 500
    assert "disco cheio" in r.text          # a falha APARECE
    assert criadas == []                    # e a sessão nova NÃO nasceu órfã


def test_post_recusa_passar_o_bastao_pra_si_mesma(bastao_post):
    post, criadas = bastao_post
    assert post(name="cc").status_code == 400
    # Com espaço no fim é a MESMA sessão depois de sanitizar — a guarda tem de pegar.
    assert post(name="cc ").status_code == 400
    assert criadas == []


def test_post_usa_o_mesmo_nome_no_arquivo_e_na_sessao(bastao_post):
    from pathlib import Path
    # `api.v2` nasce como `api-v2` (sanitize_session_name troca o ponto). Se o dossiê fosse
    # nomeado por outro sanitizador, o stem não casaria com nenhuma sessão viva e o `prune`
    # apagaria o sidecar de uma sessão VIVA depois de 7 dias.
    post, criadas = bastao_post
    r = post(name="api.v2")
    assert r.status_code == 200, r.text
    assert criadas[0].name == r.json()["name"] == "api-v2"
    assert Path(r.json()["dossie"]).name == "api-v2.md"


def test_post_400_com_nome_que_sanitiza_pra_vazio(bastao_post, tmp_path):
    post, criadas = bastao_post
    r = post(name="   ")
    assert r.status_code == 400
    assert criadas == []
    # E nada foi gravado antes da recusa (o dossiê é caro: roda git e parseia transcript).
    assert not (tmp_path / ".hangar-bastao").exists()


def test_post_diz_que_a_sessao_nasceu_quando_a_fila_falha(bastao_post, monkeypatch):
    from app import pqueue
    def sem_disco(self, *a, **kw):
        raise OSError("disco cheio")
    monkeypatch.setattr(pqueue.PromptQueue, "append", sem_disco)
    post, criadas = bastao_post
    r = post(name="cc2")
    assert r.status_code == 500
    assert criadas and criadas[0].name == "cc2"
    assert "cc2" in r.text and "nasceu" in r.text      # nomeia o que de fato aconteceu


def test_post_recusa_nome_ja_em_uso_sem_tocar_no_dossie_dela(bastao_post, monkeypatch):
    """O achado mais grave: o dossiê é `<destino>.md`, keyed por NOME. Digitar o nome de uma sessão
    viva que já recebeu um bastão sobrescrevia o dossiê DELA antes de o 409 do `create_session`
    aparecer — e o kick-off dela aponta pra aquele caminho. A recusa tem de vir antes da gravação."""
    import app.api as api_mod

    ja = bastao.gravar("cc2", "# dossiê da sessão viva\n")
    monkeypatch.setattr(api_mod, "_nome_ocupado", lambda nome: nome == "cc2")
    post, criadas = bastao_post
    r = post(name="cc2")
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_nome_em_uso"
    assert criadas == []
    assert ja.read_text(encoding="utf-8") == "# dossiê da sessão viva\n"


def test_rename_leva_o_dossie_junto(api_client_bastao, monkeypatch):
    """Renomear a sucessora deixava o dossiê órfão: o kick-off dela aponta pro caminho antigo e o
    `prune` apaga o arquivo em 7 dias. Mesmo argumento que fez a fila ser migrada no rename."""
    from unittest.mock import patch
    import app.api as api_mod

    velho = bastao.gravar("cc", "# dossiê\n")
    with patch("app.tmux.has_session", side_effect=lambda n: n == "cc"), \
         patch("app.tmux.rename_session", return_value=True), \
         patch.object(api_mod.registry, "rename", lambda a, b: None):
        r = api_client_bastao.post("/api/sessions/cc/rename", json={"new": "cx"},
                                   headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200, r.text
    assert not velho.exists()
    assert bastao.caminho("cx").read_text(encoding="utf-8") == "# dossiê\n"


def test_post_404_sem_transcript(api_client_bastao):
    from unittest.mock import patch
    from app.models import SessionInfo
    with patch("app.api.registry.list", return_value=[SessionInfo(name="cc", jsonl=None)]):
        r = api_client_bastao.post("/api/sessions/cc/bastao", json={"name": "cc2"},
                                   headers={"Authorization": "Bearer secret"})
    assert r.status_code == 404


@pytest.fixture
def api_client_bastao(monkeypatch, models_cache_em_tmp):
    from fastapi.testclient import TestClient
    from app.config import settings
    import app.api as api_mod
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    # O snapshot da lista tem TTL — sem zerar, um teste anterior pode servir a sessão errada.
    api_mod._list_snap["snap"] = None
    return TestClient(api_mod.app)


# ---------------------------------------------------------------------------
# O que falta (bastão v2, item 1)
# ---------------------------------------------------------------------------

def _bloco(md: str, titulo: str) -> str:
    return md.split(f"## {titulo}", 1)[1].split("\n## ", 1)[0]


def test_o_que_falta_cita_o_plano_da_sessao_e_diz_que_concluiu(hangar2):
    jsonl, cwd = hangar2
    md = bastao.montar(jsonl, cwd, "claude", "hangar-2")
    bloco = _bloco(md, "O que falta")
    assert "grupo-pareamento-fim-sessao" in bloco
    assert "concluído" in bloco and "60/60" in bloco
    assert "passagem-de-bastao" not in bloco          # o plano mais recente do REPO não entra aqui


def test_o_que_falta_lista_steps_pendentes_por_task(hangar2):
    jsonl, cwd = hangar2
    plano = __import__("pathlib").Path(cwd) / "docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md"
    txt = plano.read_text(encoding="utf-8").replace("- [x] **Step 3: passo 9.3**", "- [ ] **Step 3: passo 9.3**")
    plano.write_text(txt.replace("- [x] **Step 1: passo 10.1**", "- [ ] **Step 1: passo 10.1**"), encoding="utf-8")
    bloco = _bloco(bastao.montar(jsonl, cwd, "claude", "hangar-2"), "O que falta")
    assert "58/60" in bloco
    # `_STEP_RE` guarda o título do Step COM o prefixo "Step N: "; o da Task vem do heading inteiro.
    assert "Task 9: Tarefa 9: 5/6 — próximo: Step 3: passo 9.3" in bloco
    assert "Task 10: Tarefa 10: 5/6 — próximo: Step 1: passo 10.1" in bloco


def test_o_que_falta_mostra_loop_ativo_e_ignora_terminado(tmp_path):
    from app.loop import LoopLink, new_loop
    LoopLink("s").set(new_loop("passar a suíte", "uv run pytest -q", 5, False) | {"iter": 2})
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "s")
    bloco = _bloco(md, "O que falta")
    assert "Loop `running`" in bloco and "2/5" in bloco and "passar a suíte" in bloco
    LoopLink("s").update(status="done")
    assert "Loop" not in _bloco(bastao.montar(str(FIX / "jsonl_samples.jsonl"), None, "claude", "s"), "O que falta")


def test_sem_plano_citado_cai_no_plano_da_barra(tmp_path, monkeypatch):
    from app import planprog
    from app.planprog import PlanProgress, TaskProgress, StepProgress
    prog = PlanProgress(name="x", path="/p/x.md", task_idx=1, task_total=1, done=1, total=2, complete=False,
                        tasks=(TaskProgress(title="Task 1: A", done=1, total=2,
                                            steps=(StepProgress("Step 1: a", True, False, 0),
                                                   StepProgress("Step 2: b", False, False, 1))),))
    monkeypatch.setattr(planprog, "plan_progress", lambda cwd: prog)
    bloco = _bloco(bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "s"), "O que falta")
    assert "a sessão não citou plano" in bloco and "Task 1: A: 1/2 — próximo: Step 2: b" in bloco


def test_plano_citado_e_lido_do_fim_do_arquivo(tmp_path):
    # 1) a última menção vence; 2) o arquivo não é lido inteiro (o teto de bytes devolve None).
    linhas = [json.dumps({"type": "assistant", "uuid": f"a{i}", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": f"t{i}", "name": "Read",
         "input": {"file_path": f"/repo/docs/superpowers/plans/2026-01-0{i}-p.md"}}]}}) for i in (1, 2)]
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("\n".join(linhas) + "\n", encoding="utf-8")
    assert bastao._plano_citado(str(jsonl)) == "/repo/docs/superpowers/plans/2026-01-02-p.md"
    grande = tmp_path / "g.jsonl"
    grande.write_text(linhas[0] + "\n" + ("{}\n" * 400_000), encoding="utf-8")   # plano só no início, > teto
    assert bastao._plano_citado(str(grande), teto=64 * 1024) is None


def test_sem_plano_nem_loop_lista_os_pedidos_sem_resposta(tmp_path):
    # "agora roda os testes" vem ANTES da fala "Rodei, passou." — já foi respondido e fica fora;
    # os três seguintes vêm depois da última fala do agente e são os pendentes.
    turnos = [("Feito o passo 1.", "agora roda os testes"),
              ("Rodei, passou.", "então commita"),
              (None, "e depois faz o commit do README"),
              (None, "ah, e avisa o par")]
    md = bastao.montar(_conversa(tmp_path, turnos), None, "claude", "s")
    bloco = _bloco(md, "O que falta")
    assert "AINDA SEM resposta" in bloco
    for t in ("então commita", "commit do README", "avisa o par"):
        assert t in bloco


# ---------------------------------------------------------------------------
# Onde está o trabalho: commits da sessão (bastão v2, item 2)
# ---------------------------------------------------------------------------

def test_onde_esta_o_trabalho_lista_os_commits_da_sessao(tmp_path, monkeypatch):
    from app import git_ops, pqueue
    monkeypatch.setattr(git_ops, "head_info", lambda cwd: ("main", False))
    monkeypatch.setattr(git_ops, "git_summary", lambda cwd: {"dirty": 0, "ahead": 14, "behind": 0})
    monkeypatch.setattr(git_ops, "git_diffstat", lambda cwd: None)
    monkeypatch.setattr(git_ops, "changed_files", lambda cwd: [])
    monkeypatch.setattr(git_ops, "git_log_since",
                        lambda cwd, desde, n=14: [{"short": "3c6db932", "subject": "refactor(pair): textos"},
                                                  {"short": "1e52643f", "subject": "fix(pair): varredura"}])
    monkeypatch.setattr(pqueue, "_transcript_start_ts", lambda jsonl: 1.0)
    md = bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "s")
    bloco = _bloco(md, "Onde está o trabalho")
    assert "Commits desde" in bloco
    assert "`3c6db932` refactor(pair): textos" in bloco and "`1e52643f` fix(pair): varredura" in bloco
    assert "à frente" not in bloco            # a contagem só fica quando o log vem vazio


def test_commits_listados_preservam_o_atras(tmp_path, monkeypatch):
    from app import git_ops, pqueue
    monkeypatch.setattr(git_ops, "head_info", lambda cwd: ("main", False))
    monkeypatch.setattr(git_ops, "git_summary", lambda cwd: {"dirty": 0, "ahead": 1, "behind": 3})
    monkeypatch.setattr(git_ops, "git_diffstat", lambda cwd: None)
    monkeypatch.setattr(git_ops, "changed_files", lambda cwd: [])
    monkeypatch.setattr(git_ops, "git_log_since", lambda cwd, desde, n=14: [{"short": "abc1234", "subject": "x"}])
    monkeypatch.setattr(pqueue, "_transcript_start_ts", lambda jsonl: 1.0)
    bloco = _bloco(bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "s"),
                   "Onde está o trabalho")
    assert "3 atrás" in bloco and "à frente" not in bloco


def test_sem_commit_na_sessao_mantem_a_contagem(tmp_path, monkeypatch):
    from app import git_ops, pqueue
    monkeypatch.setattr(git_ops, "head_info", lambda cwd: ("main", False))
    monkeypatch.setattr(git_ops, "git_summary", lambda cwd: {"dirty": 0, "ahead": 2, "behind": 0})
    monkeypatch.setattr(git_ops, "git_diffstat", lambda cwd: None)
    monkeypatch.setattr(git_ops, "changed_files", lambda cwd: [])
    monkeypatch.setattr(git_ops, "git_log_since", lambda cwd, desde, n=30: [])
    monkeypatch.setattr(pqueue, "_transcript_start_ts", lambda jsonl: 1.0)
    bloco = _bloco(bastao.montar(str(FIX / "jsonl_samples.jsonl"), str(tmp_path), "claude", "s"),
                   "Onde está o trabalho")
    assert "2 commit(s) à frente" in bloco
    assert "agora roda os testes" not in bloco
