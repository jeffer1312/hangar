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


def _titulos(texto: str) -> list[str]:
    return [ln[3:] for ln in texto.splitlines() if ln.startswith("## ")]


TODAS = ["De onde veio", "Onde está o trabalho", "O plano", "Arquivos e comandos",
         "Grupo e par", "Decisões", "Estado agora"]


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
    api_mod._list_snap["infos"] = None
    return TestClient(api_mod.app)
