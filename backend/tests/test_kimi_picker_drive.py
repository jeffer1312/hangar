# Drive do picker de AskUserQuestion do Kimi. Os panes abaixo sao os desenhos REAIS medidos em
# 13/08/2026 (Kimi 0.36.0) — ver a medicao: opcoes numeradas, tecla numerica escolhe E avanca,
# multi-escolha sai com Tab (ali ↵ e toggle), "Other" abre campo de texto e o rodape vira
# "type answer", e sem passar pela tela de Review a resposta NAO chega na ferramenta.
import pytest

from app import terminal_input as ti


def _pane_escolha(cursor=1, n=3, pergunta="Qual cor?"):
    # A pergunta entra no pane porque o drive CONFERE qual aba esta na tela antes de digitar (o
    # texto depois do "? " e a unica pista sem ler cor ANSI).
    linhas = ["  question", "", "   Cor    Submit", "", f"  ? {pergunta}", ""]
    for i in range(1, n + 2):                     # +1 = o "Other" que a TUI adiciona
        marca = "   → " if i == cursor else "     "
        linhas.append(f"{marca}[{i}] Opcao{i}" if i <= n else f"{marca}[{i}] Other")
    linhas += ["", f"   ↑↓ select  1-{n + 1} / ↵ choose  ←/→/tab switch  esc cancel"]
    return "\n".join(linhas)


_PANE_MULTI = "\n".join([
    "  question", "", "   Linguagens    Submit", "", "  ? Quais?", "",
    "   [ ] Python", "   [ ] Rust", "   [ ] Go", "   [ ] Other", "",
    "   ↑↓ select  1-4 / ↵ toggle  ←/→/tab switch  esc cancel"])

_PANE_TEXTO = "\n".join([
    "  question", "", "  ? Qual cor?", "    Type your answer, then press Enter to save.", "",
    "     [1] Python", "   → [2] Other:", "",
    "   type answer  ↵ save  tab switch  esc cancel"])

_PANE_REVIEW = "\n".join([
    "  question", "", "  (✓) Cor   Submit", "", "  Review your answer before submit", "",
    "   Q  Qual cor você prefere?", "   →  Verde", "", "  Ready to submit your answers?", "",
    "   → [1] Submit", "     [2] Cancel", "",
    "   ↑↓ select  1/2 choose  ↵ confirm  ←/→/tab switch  esc cancel"])

_PANE_FECHADO = "╭────────╮\n│ >      │\n╰────────╯\n 🤖 K3 (high✦)"


@pytest.fixture
def teclado(monkeypatch):
    """Grava as teclas e serve os panes na ordem pedida."""
    teclas = []
    monkeypatch.setattr(ti, "send_keys",
                        lambda name, k, literal=False: teclas.append((k, literal)))
    monkeypatch.setattr(ti.time, "sleep", lambda s: None)
    return teclas


def _panes(monkeypatch, seq):
    it = iter(seq)
    ultimo = {}
    def cap(name):
        try:
            ultimo["v"] = next(it)
        except StopIteration:
            pass
        return ultimo.get("v", "")
    monkeypatch.setattr(ti, "_capture", cap)


QS = [{"question": "Qual cor?", "options": [{"label": "Azul"}, {"label": "Verde"}, {"label": "Vermelho"}]}]


def test_escolha_simples_manda_o_numero_e_confirma_no_review(teclado, monkeypatch):
    # Tecla numerica escolhe E avanca — nada de contar linha e mandar (n-1)xDown como no Claude.
    _panes(monkeypatch, [_pane_escolha()] * 3 + [_PANE_REVIEW])
    ti.answer_question_kimi("s", [{"kind": "option", "indices": [1], "labels": ["Verde"]}], QS)
    assert teclado == [("2", True), ("1", True)]        # opcao 2, depois [1] Submit


def test_multi_escolha_sai_com_tab_nao_com_enter(teclado, monkeypatch):
    # No multi-select o ↵ e TOGGLE e nao avanca: quem sai da pergunta e o Tab.
    _panes(monkeypatch, [_PANE_MULTI] * 5 + [_PANE_REVIEW])
    ti.answer_question_kimi("s", [{"kind": "option", "indices": [0, 2], "multi": True,
                                   "labels": ["Python", "Go"]}],
                            [{"question": "Quais?", "options": [{"label": "Python"},
                                                                {"label": "Rust"}, {"label": "Go"}]}])
    assert teclado == [("1", True), ("3", True), ("Tab", False), ("1", True)]
    assert ("Enter", False) not in teclado             # Enter ali so marcaria/desmarcaria


def test_texto_livre_usa_o_other_que_e_sempre_a_ultima(teclado, monkeypatch):
    _panes(monkeypatch, [_pane_escolha()] * 5 + [_PANE_REVIEW])
    ti.answer_question_kimi("s", [{"kind": "text", "value": "Elixir"}], QS)
    # 3 opcoes -> "Other" e a [4]; depois o texto e o Enter que salva
    assert teclado == [("4", True), ("Elixir", True), ("Enter", False), ("1", True)]


def test_recusa_numero_quando_o_picker_virou_campo_de_texto(teclado, monkeypatch):
    # No modo "type answer" a tecla numerica vira CARACTERE: mandaria "2" pro campo em vez de
    # escolher a opcao 2.
    _panes(monkeypatch, [_pane_escolha(), _pane_escolha(), _PANE_TEXTO])
    with pytest.raises(ti.DriveError, match="modo texto"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [1], "labels": ["Verde"]}], QS)
    assert teclado == []                                # nada digitado


def test_sem_review_nao_aperta_nada(teclado, monkeypatch):
    # Sem a tela de Review a resposta NAO chega na ferramenta — e um "1" as cegas cairia numa
    # pergunta ainda aberta e escolheria a opcao errada.
    _panes(monkeypatch, [_pane_escolha(), _pane_escolha(), _pane_escolha(2)])
    with pytest.raises(ti.DriveError, match="Review"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [1], "labels": ["Verde"]}], QS)
    assert teclado == [("2", True)]                     # so a escolha; nenhum Submit


def test_picker_fechado_nao_dirige(teclado, monkeypatch):
    _panes(monkeypatch, [_PANE_FECHADO])
    with pytest.raises(ti.DriveError, match="nao esta aberto"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [0], "labels": ["Azul"]}], QS)
    assert teclado == []


@pytest.mark.parametrize("resposta,erro", [
    ({"kind": "option", "indices": []}, "sem opcao"),
    ({"kind": "option", "indices": [9]}, "fora de"),
    ({"kind": "option", "indices": [0, 1]}, "escolha unica"),
    ({"kind": "text", "value": "   "}, "texto vazio"),
    ({"kind": "text", "value": "a\x01b"}, "caractere de controle"),
    ({"kind": "chat"}, "kind nao suportado"),
])
def test_valida_tudo_antes_de_tocar_no_terminal(teclado, monkeypatch, resposta, erro):
    # Drive que para no meio deixa o picker aberto numa aba qualquer, e o fallback por texto
    # depois disso entra por cima de um overlay meio-navegado.
    _panes(monkeypatch, [_pane_escolha()])
    with pytest.raises(ValueError, match=erro):
        ti.answer_question_kimi("s", [resposta], QS)
    assert teclado == []


# A confirmacao NAO e visual: o Kimi so escreve o `tool.result` daquele toolCallId depois de a
# ferramenta receber as respostas de verdade. O pane diz o que esta desenhado; o wire diz o que
# chegou. Sem isto, um Submit que nao pegou voltaria 200 com cara de sucesso.
def test_resposta_chegou_le_o_result_do_mesmo_id(tmp_path):
    import json
    from app.adapters.kimi.transcript import read_pending_call, resposta_chegou

    def ev(e):
        return json.dumps({"type": "context.append_loop_event", "event": e}) + "\n"

    w = tmp_path / "wire.jsonl"
    w.write_text(ev({"type": "tool.call", "toolCallId": "tool_A", "name": "AskUserQuestion",
                     "args": {"questions": [{"question": "Cor?", "options": [{"label": "Azul"}]}]}}),
                 encoding="utf-8")
    assert read_pending_call(str(w))[0] == "tool_A"
    assert resposta_chegou(str(w), "tool_A") is False        # ainda pendente

    with open(w, "a", encoding="utf-8") as fh:               # o result do MESMO id
        fh.write(ev({"type": "tool.result", "toolCallId": "tool_A",
                     "result": {"output": '{"answers":{"Cor?":"Azul"}}'}}))
    assert resposta_chegou(str(w), "tool_A") is True
    assert read_pending_call(str(w)) is None


def test_result_de_outra_chamada_nao_confirma(tmp_path):
    # Duas perguntas seguidas: o result da PRIMEIRA nao pode dar a segunda por respondida.
    import json
    from app.adapters.kimi.transcript import resposta_chegou

    def ev(e):
        return json.dumps({"type": "context.append_loop_event", "event": e}) + "\n"

    w = tmp_path / "wire.jsonl"
    w.write_text(
        ev({"type": "tool.call", "toolCallId": "tool_A", "name": "AskUserQuestion", "args": {"questions": []}})
        + ev({"type": "tool.result", "toolCallId": "tool_A", "result": {"output": "{}"}})
        + ev({"type": "tool.call", "toolCallId": "tool_B", "name": "AskUserQuestion", "args": {"questions": []}}),
        encoding="utf-8")
    assert resposta_chegou(str(w), "tool_B") is False


def test_espera_resposta_estoura_e_vira_fallback(tmp_path, monkeypatch):
    # O caminho de ESTOURO do prazo nao da pra exercer ao vivo sem atrasar o Kimi de proposito (o
    # teste ao vivo voltou bem antes dos 5s nas 4 chamadas). Aqui ele e forcado: sem o result no
    # wire, `_espera_resposta_kimi` devolve False — e o endpoint trata isso como drive falhado e cai
    # no fallback por texto, em vez de responder 200 com cara de sucesso.
    import json
    import app.api as api

    w = tmp_path / "wire.jsonl"
    w.write_text(json.dumps({"type": "context.append_loop_event",
                             "event": {"type": "tool.call", "toolCallId": "tool_X",
                                       "name": "AskUserQuestion", "args": {"questions": []}}}) + "\n",
                 encoding="utf-8")
    monkeypatch.setattr(api.time, "sleep", lambda s: None)
    assert api._espera_resposta_kimi(str(w), "tool_X", timeout=0.3) is False
    # sem jsonl nao ha como provar entrega -> False (nunca True por omissao)
    assert api._espera_resposta_kimi(None, "tool_X", timeout=0.3) is False


# Confirmacao por AUSENCIA de dado era o furo: deduzir "chegou" de "nao ha pergunta pendente" dava
# True tambem pra wire sumido e pra linha corrompida — um transcript ilegivel virava "entregue com
# sucesso". Agora procura o result DIRETO; nao deu pra ler = nao chegou.
def test_resposta_chegou_nao_confirma_por_wire_ilegivel(tmp_path):
    import json
    from app.adapters.kimi.transcript import resposta_chegou

    assert resposta_chegou(str(tmp_path / "nao-existe.jsonl"), "tool_X") is False

    quebrado = tmp_path / "quebrado.jsonl"
    quebrado.write_text('{"type": "context.append_loop_event", "event": {quebrado\n',
                        encoding="utf-8")
    assert resposta_chegou(str(quebrado), "tool_A") is False

    vazio = tmp_path / "vazio.jsonl"
    vazio.write_text("", encoding="utf-8")
    assert resposta_chegou(str(vazio), "tool_A") is False

    # E o positivo continua valendo: com o result do id na mao, confirma.
    ok = tmp_path / "ok.jsonl"
    ok.write_text(json.dumps({"type": "context.append_loop_event",
                              "event": {"type": "tool.result", "toolCallId": "tool_A",
                                        "result": {"output": "{}"}}}) + "\n", encoding="utf-8")
    assert resposta_chegou(str(ok), "tool_A") is True
    assert resposta_chegou(str(ok), "tool_OUTRO") is False


def test_drive_falhando_no_meio_de_varias_perguntas(teclado, monkeypatch):
    # O caso que faltava: picker de DUAS perguntas, a primeira ja respondida (aba avancou) e a
    # segunda travando. O DriveError sai com teclas JA mandadas — o picker fica meio-navegado, e e
    # por isso que o caller manda Escape antes do fallback por texto.
    _panes(monkeypatch, [_pane_escolha(pergunta="Cor?"), _pane_escolha(pergunta="Cor?"),
                         _pane_escolha(pergunta="Ling?"),
                         _PANE_TEXTO.replace("? Qual cor?", "? Ling?")])
    qs = [{"question": "Cor?", "options": [{"label": "A"}, {"label": "B"}, {"label": "C"}]},
          {"question": "Ling?", "options": [{"label": "X"}, {"label": "Y"}, {"label": "Z"}]}]
    with pytest.raises(ti.DriveError, match="modo texto"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [0], "labels": ["A"]},
                                      {"kind": "option", "indices": [1], "labels": ["Y"]}], qs)
    assert teclado == [("1", True)]        # a 1a passou; a 2a travou ANTES de digitar


def test_nao_digita_na_aba_errada(teclado, monkeypatch):
    # O buraco que a malha fechada por aba fecha: a tecla numerica avanca de aba sozinha, entao um
    # redesenho atrasado deixava a tela AINDA na pergunta 1 quando o drive ia digitar a resposta da
    # 2. O rodape casa igual em qualquer aba do mesmo modo — nada levantava erro: o Submit saia, o
    # tool.result chegava, e a resposta ia pra pergunta ERRADA com cara de sucesso.
    qs = [{"question": "Cor?", "options": [{"label": "A"}, {"label": "B"}, {"label": "C"}]},
          {"question": "Ling?", "options": [{"label": "X"}, {"label": "Y"}, {"label": "Z"}]}]
    # a tela NUNCA sai da pergunta 1
    _panes(monkeypatch, [_pane_escolha(pergunta="Cor?")] * 30)
    with pytest.raises(ti.DriveError, match="nao chegou na pergunta 2"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [0], "labels": ["A"]},
                                      {"kind": "option", "indices": [1], "labels": ["Y"]}], qs)
    assert teclado == [("1", True)]        # so a resposta da 1a; nada digitado as cegas na 2a


def test_multi_escolha_recusa_indice_repetido(teclado, monkeypatch):
    # A tecla numerica e TOGGLE: o mesmo indice duas vezes liga e desliga, e a opcao terminaria
    # DESMARCADA — o drive seguiria ate o Submit e entregaria resposta diferente da pedida.
    _panes(monkeypatch, [_PANE_MULTI])
    with pytest.raises(ValueError, match="indice repetido"):
        ti.answer_question_kimi("s", [{"kind": "option", "indices": [0, 0], "multi": True,
                                       "labels": ["Python"]}],
                                [{"question": "Quais?", "options": [{"label": "Python"},
                                                                    {"label": "Go"}]}])
    assert teclado == []


def test_confirmacao_atrasada_nao_entrega_duas_vezes(monkeypatch, tmp_path):
    # Prazo estourado NAO prova que nada foi submetido: pode ser o Kimi demorando pra gravar. Cair
    # no fallback ali mandaria Escape num turno que ja processa a resposta certa e entregaria a
    # mesma resposta DUAS vezes — uma pela ferramenta, outra como mensagem no chat.
    import json
    from types import SimpleNamespace
    from fastapi import HTTPException
    import app.api as api

    w = tmp_path / "wire.jsonl"
    w.write_text(json.dumps({"type": "context.append_loop_event",
                             "event": {"type": "tool.call", "toolCallId": "tool_X",
                                       "name": "AskUserQuestion",
                                       "args": {"questions": [{"question": "Cor?",
                                                               "options": [{"label": "A"}]}]}}}) + "\n",
                 encoding="utf-8")
    info = SimpleNamespace(name="k", jsonl=str(w), provider="kimi")
    monkeypatch.setattr(api.registry, "list", lambda: [info])
    monkeypatch.setattr(api, "_recusa_se_painel_aberto", lambda n: None)
    monkeypatch.setattr(api.terminal_input, "answer_question_kimi", lambda *a: None)
    monkeypatch.setattr(api, "_espera_resposta_kimi", lambda *a, **k: False)
    enviados, escapes = [], []
    monkeypatch.setattr(api, "_send_one", lambda n, t: enviados.append(t) or {"ok": True})
    monkeypatch.setattr(api.terminal, "interrupt", lambda n: escapes.append(n))

    body = api.AnswerBody(answers=[api.AnswerItem(kind="option", indices=[0], labels=["A"])])

    # picker SUMIU da tela = alguem submeteu: admite a duvida, nao reenvia nem interrompe
    monkeypatch.setattr(api.terminal_input, "picker_kimi_aberto", lambda n: False)
    with pytest.raises(HTTPException) as e:
        api.answer("k", body)
    assert e.value.status_code == 409 and e.value.detail["code"] == "erro_sem_confirmacao_resposta"
    assert enviados == [] and escapes == []       # nada duplicado, nenhum turno interrompido

    # picker AINDA aberto = prova de que nada pegou: fallback por texto, como sempre
    monkeypatch.setattr(api.terminal_input, "picker_kimi_aberto", lambda n: True)
    monkeypatch.setattr(api, "_espera_picker_fechar", lambda n: True)
    assert api.answer("k", body) == {"ok": True, "fallback": True}
    assert len(enviados) == 1 and escapes == ["k"]


def test_steer_sem_marcador_nao_tecla(teclado, monkeypatch):
    # O chip pode existir sem a TUI ter fila (msg já entrou no turno por outra via): sem o
    # marcador, o ctrl-s seria no-op e confirmar a entrega ali seria mentira.
    _panes(monkeypatch, [_PANE_FECHADO])
    assert ti.steer_now("s") == "sem-fila"
    assert teclado == []


def test_steer_com_marcador_manda_ctrl_s(teclado, monkeypatch):
    _panes(monkeypatch, ["↑ to edit · ctrl-s to steer immediately"])
    assert ti.steer_now("s") is True
    assert teclado == [("C-s", False)]


def test_steer_pane_ilegivel_degrada_pra_tecla(teclado, monkeypatch):
    # Pane ilegível NUNCA bloqueia: cai no comportamento de sempre (tecla, que é no-op se vazio).
    def cap(name):
        raise OSError("tmux fora")
    monkeypatch.setattr(ti, "_capture", cap)
    assert ti.steer_now("s") is True
    assert teclado == [("C-s", False)]
