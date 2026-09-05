import json
from pathlib import Path
from app import askquestion
from app.askquestion import read_pending_askq, clear_pending_askq, pergunta_aberta
from app.models import StateEvent
from app.sse import _ask_question_event


def _state_json(state: str, overlay: bool = False, options=None) -> str:
    return StateEvent(session="s", state=state, overlay=overlay, options=options).model_dump_json()


_Q = [{"header": "Cor", "question": "Escolha", "multiSelect": True,
       "options": [{"label": "A", "description": "op A"}, {"label": "B", "description": ""}]},
      {"header": "Fruta", "question": "Escolha fruta", "multiSelect": False,
       "options": [{"label": "X", "description": ""}, {"label": "Y", "description": ""}]}]


def _layout(tmp_path: Path, questions=_Q, sid="sess-123",
            write_sidecar=True, sidecar_text=None):
    # Monta o layout <tmp>/projects/<proj>/<sid>.jsonl (so o PATH do jsonl importa, sem conteudo) e
    # grava o sidecar do hook PreToolUse em <tmp>/.hangar-askq/<sid>.json com stdin realista.
    proj = tmp_path / "projects" / "home-x"
    proj.mkdir(parents=True)
    jsonl = proj / f"{sid}.jsonl"
    sc_dir = tmp_path / ".hangar-askq"
    sc_dir.mkdir(parents=True)
    sc = sc_dir / f"{sid}.json"
    if write_sidecar:
        if sidecar_text is not None:
            sc.write_text(sidecar_text, encoding="utf-8")
        else:
            sc.write_text(json.dumps({
                "session_id": sid, "tool_name": "AskUserQuestion",
                "tool_input": {"questions": questions}, "cwd": "/home/x",
                "transcript_path": str(jsonl),
            }), encoding="utf-8")
    return str(jsonl), sc


def test_read_pending_askq_returns_payload(tmp_path):
    jsonl, _ = _layout(tmp_path)
    out = read_pending_askq(jsonl)
    assert out is not None
    assert out.questions[0].header == "Cor"
    assert out.questions[0].options[0].label == "A"
    assert out.questions[0].multiSelect is True


def test_read_pending_askq_none_when_no_sidecar(tmp_path):
    jsonl, _ = _layout(tmp_path, write_sidecar=False)
    assert read_pending_askq(jsonl) is None


def test_read_pending_askq_none_on_garbage(tmp_path):
    jsonl, sc = _layout(tmp_path, sidecar_text="{not valid json")
    assert read_pending_askq(jsonl) is None
    # JSON valido porem sem tool_input -> tambem None
    sc.write_text(json.dumps({"session_id": "x", "tool_name": "AskUserQuestion"}), encoding="utf-8")
    assert read_pending_askq(jsonl) is None


def test_clear_pending_askq_removes_sidecar(tmp_path):
    jsonl, sc = _layout(tmp_path)
    assert sc.exists()
    clear_pending_askq(jsonl)
    assert not sc.exists()
    clear_pending_askq(jsonl)  # idempotente: chamar de novo nao levanta


# --- _ask_question_event: awaiting_input + sidecar(>=2) + opcoes batendo (SEM depender de overlay) ---

def test_ask_question_event_emits_when_options_match(tmp_path):
    jsonl, _ = _layout(tmp_path)  # _Q[0] = Cor, opcoes A/B
    ev = _ask_question_event(_state_json("awaiting_input", options=["A", "B", "Type something."]), jsonl)
    assert ev is not None
    assert ev["event"] == "ask_question"
    assert json.loads(ev["data"])["questions"][0]["header"] == "Cor"


def test_ask_question_event_emits_even_without_overlay(tmp_path):
    # O BUG corrigido: is_overlay e falso p/ AskUserQuestion (rodape fora das ultimas 8 linhas). O
    # evento DEVE disparar com overlay=False desde que as opcoes do menu batam com o sidecar.
    jsonl, _ = _layout(tmp_path)
    assert _ask_question_event(_state_json("awaiting_input", overlay=False, options=["A", "B"]), jsonl) is not None


def test_ask_question_event_none_when_working(tmp_path):
    jsonl, _ = _layout(tmp_path)
    assert _ask_question_event(_state_json("working", options=["A", "B"]), jsonl) is None


def test_ask_question_event_none_when_options_mismatch(tmp_path):
    # Sidecar velho (Cor: A/B) sobre OUTRO prompt cujo menu e Sim/Nao -> NAO abre o stepper (freshness).
    jsonl, _ = _layout(tmp_path)
    assert _ask_question_event(_state_json("awaiting_input", options=["Sim", "Nao"]), jsonl) is None


def test_ask_question_event_emits_for_single_question(tmp_path):
    # Pergunta UNICA tambem abre o stepper. Antes havia um gate `len(questions) < 2 -> None` que a
    # jogava no OptionButtons; como aquele le o picker do PANE, e o pane so tem rotulo, a descricao
    # de cada opcao — onde mora a explicacao da escolha — sumia da tela. answer_questions ja trata
    # pergunta unica (terminal_input.py:463, guard de malha fechada porque ali o Enter ja submete).
    one = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
            "options": [{"label": "A", "description": "op A"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    ev = _ask_question_event(_state_json("awaiting_input", options=["A"]), jsonl)
    assert ev is not None
    assert json.loads(ev["data"])["questions"][0]["options"][0]["description"] == "op A"


def test_ask_question_event_single_question_still_checks_freshness(tmp_path):
    # Tirar o gate NAO afrouxa o frescor: sidecar de UMA pergunta sobre outro prompt (menu Sim/Nao,
    # ex. permissao) continua sem abrir o stepper.
    one = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
            "options": [{"label": "A", "description": "op A"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    assert _ask_question_event(_state_json("awaiting_input", options=["Sim", "Nao"]), jsonl) is None


def test_ask_question_event_recusa_menu_com_opcao_a_mais(tmp_path):
    # Sidecar STALE cujos rotulos por acaso APARECEM num menu maior. So o subset nao basta: a
    # submissao e POR INDICE (terminal_input.py:463), entao com o menu real em outra ordem o Enter
    # cairia na linha errada — cross-wire de resposta. Qualquer opcao REAL no pane que o sidecar nao
    # tenha reprova o casamento.
    one = [{"header": "Confirma", "question": "Vai?", "multiSelect": False,
            "options": [{"label": "Sim", "description": ""}, {"label": "Nao", "description": ""}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    ev = _ask_question_event(
        _state_json("awaiting_input", options=["Cancelar", "Sim", "Nao"]), jsonl)
    assert ev is None


def test_ask_question_event_ignora_as_linhas_proprias_do_tui(tmp_path):
    # O TUI acrescenta "Type something." e "Chat about this" a TODA pergunta. Elas nunca estao no
    # sidecar e nao podem reprovar o casamento — senao nenhuma pergunta abriria o stepper.
    one = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
            "options": [{"label": "A", "description": "d"}, {"label": "B", "description": "d"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    ev = _ask_question_event(
        _state_json("awaiting_input", options=["A", "B", "Type something.", "Chat about this"]), jsonl)
    assert ev is not None


def test_ask_question_event_single_question_with_preview_emits(tmp_path):
    # Excecao do gate de 1 pergunta: opcao com `preview` so renderiza no stepper (OptionButtons nao
    # tem o payload) -> emite mesmo com pergunta unica, e o preview vai no data.
    one = [{"header": "Ordem", "question": "Como deixo?", "multiSelect": False,
            "options": [{"label": "System no topo (igual aos irmãos)", "description": "d",
                         "preview": "using System.Reflection;\nusing Xunit;"},
                        {"label": "Alfabético (obedece .editorconfig)", "description": "d"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    ev = _ask_question_event(
        _state_json("awaiting_input",
                    options=["System no topo (igual aos", "Alfabético (obedece"]),  # truncadas (wrap)
        jsonl)
    assert ev is not None
    opts = json.loads(ev["data"])["questions"][0]["options"]
    assert opts[0]["preview"].startswith("using System.Reflection;")


def test_ask_question_event_preview_emite_com_as_linhas_do_tui_no_pane(tmp_path):
    # O pane REAL sempre traz "Type something."/"Chat about this" como opcoes numeradas. Como o ramo
    # de preview compara por CONTAGEM IGUAL, sem tira-las da conta ele reprovava 100% das perguntas
    # com preview — o oposto do que aquele ramo existe pra fazer. O fixture do teste vizinho nunca
    # incluiu essas linhas, por isso o bug passou.
    one = [{"header": "Ordem", "question": "Como deixo?", "multiSelect": False,
            "options": [{"label": "System no topo (igual aos irmãos)", "description": "d",
                         "preview": "using System.Reflection;"},
                        {"label": "Alfabético (obedece .editorconfig)", "description": "d"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    ev = _ask_question_event(
        _state_json("awaiting_input",
                    options=["System no topo (igual aos", "Alfabético (obedece",
                             "Type something.", "Chat about this"]),
        jsonl)
    assert ev is not None


def test_ask_question_event_prefix_match_tolerates_truncated_pane(tmp_path):
    # Freshness por prefixo: label do pane truncada por wrap ainda casa; menu de OUTRO prompt nao.
    jsonl, _ = _layout(tmp_path)  # _Q: A/B + X/Y
    assert _ask_question_event(_state_json("awaiting_input", options=["A", "B"]), jsonl) is not None
    assert _ask_question_event(_state_json("awaiting_input", options=["Sim", "Nao"]), jsonl) is None


def test_ask_question_event_preview_loga_quando_reprova(tmp_path, caplog):
    # Reprovar no ramo de preview era MUDO: o sintoma era a folha nativa virar lista de rotulo
    # truncado, sem uma linha de log. O ramo sem preview ja logava, e foi por ele que se achou o
    # problema de 25/08/2026 — este teste garante que os dois lados aparecem no log.
    one = [{"header": "Ordem", "question": "Como deixo?", "multiSelect": False,
            "options": [{"label": "System no topo", "description": "d", "preview": "using Xunit;"},
                        {"label": "Alfabético", "description": "d"}]}]
    jsonl, _ = _layout(tmp_path, questions=one)
    with caplog.at_level("INFO", logger="hangar.sse"):
        assert _ask_question_event(
            _state_json("awaiting_input", options=["Sim", "Nao"]), jsonl) is None
    assert "sidecar com preview nao casa o menu" in caplog.text
    assert "Sim" in caplog.text and "System no topo" in caplog.text


# ── pergunta_aberta: a pergunta que o pane NAO mostra ──────────────────────────────────────────
# Cenario medido em 01/09/2026: a TUI imprimiu um recado longo de outra sessao sem levar a viewport
# pro fim, o menu do AskUserQuestion ficou abaixo da area visivel, o classify nao viu menu nenhum e
# a sessao ficou `idle` — pergunta feita, celular sem stepper e sem badge. Aqui a fonte e o sidecar
# do hook, e quem diz se ela ja foi respondida e o transcript, nunca a tela.

def _transcript(jsonl: str, linhas: list[dict]) -> None:
    Path(jsonl).write_text(
        "".join(json.dumps(x, ensure_ascii=False) + "\n" for x in linhas), encoding="utf-8")


def _resposta(quando: str, tool_id: str = "toolu_1") -> list[dict]:
    """As duas entradas que RESPONDER grava: o tool_use do AskUserQuestion e o tool_result dele."""
    return [
        {"type": "assistant", "timestamp": quando,
         "message": {"content": [{"type": "tool_use", "id": tool_id, "name": "AskUserQuestion"}]}},
        {"type": "user", "timestamp": quando,
         "message": {"content": [{"type": "tool_result", "tool_use_id": tool_id}]}},
    ]


def _aponta(monkeypatch, tmp_path) -> None:
    # No proprio askquestion: ele importa `dirs_de_config` no TOPO, entao trocar so em
    # app.statusline nao alcanca a referencia que a funcao usa.
    monkeypatch.setattr(askquestion, "dirs_de_config", lambda: [tmp_path])


def test_pergunta_aberta_quando_transcript_nao_tem_resposta(tmp_path, monkeypatch):
    jsonl, _ = _layout(tmp_path)
    _transcript(jsonl, [{"type": "assistant", "timestamp": "2026-09-01T17:27:13.000Z",
                         "message": {"content": [{"type": "text", "text": "oi"}]}}])
    _aponta(monkeypatch, tmp_path)
    out = pergunta_aberta("sess-123")
    assert out is not None
    assert out.questions[0].question == "Escolha"


def test_pergunta_respondida_pela_tui_nao_reabre(tmp_path, monkeypatch):
    # O sidecar NAO e apagado quando a resposta vem pelo terminal (so no /answer). Sem olhar o
    # transcript, toda sessao que ja respondeu uma pergunta ficaria presa em awaiting_input.
    jsonl, _ = _layout(tmp_path)
    _transcript(jsonl, _resposta("2036-01-01T00:00:00.000Z"))   # bem depois do mtime do sidecar
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is None


def test_resposta_anterior_ao_sidecar_nao_esconde_a_pergunta_nova(tmp_path, monkeypatch):
    # Pergunta velha respondida + pergunta nova pendente: o tool_result esta no transcript, mas e de
    # antes do sidecar. Comparar por presenca, e nao por tempo, engoliria a pergunta atual.
    jsonl, _ = _layout(tmp_path)
    _transcript(jsonl, _resposta("2020-01-01T00:00:00.000Z"))
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is not None


def test_sem_sidecar_e_sem_stem_nao_ha_pergunta(tmp_path, monkeypatch):
    _layout(tmp_path, write_sidecar=False)
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is None
    assert pergunta_aberta(None) is None


def test_janela_que_nao_alcanca_a_pergunta_nao_prende_a_sessao(tmp_path, monkeypatch):
    # A janela e contada do FIM do arquivo, entao ela envelhece: a sessao segue trabalhando, o
    # transcript cresce e o par tool_use/tool_result da resposta sai por cima. "Nao achei" ali NAO e
    # "nao existe" — e a pergunta ficava aberta pra sempre, prendendo a sessao em awaiting_input, e
    # com ela a fila (o drain so dispara na borda de subida de "entregavel", que nunca mais subiria).
    jsonl, _ = _layout(tmp_path)
    enche = [{"type": "assistant", "timestamp": "2036-01-01T00:00:00.000Z",
              "message": {"content": [{"type": "text", "text": "x" * 400}]}} for _ in range(900)]
    _transcript(jsonl, enche)                       # tudo POSTERIOR ao sidecar, e > 256KB
    assert Path(jsonl).stat().st_size > 256 * 1024  # a janela nao alcanca o inicio
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is None


def test_transcript_ilegivel_nao_inventa_pergunta(tmp_path, monkeypatch):
    # Sidecar aponta pra transcript que sumiu: sem como provar que segue aberta, cala — mostrar
    # pergunta ja respondida e pior que nao mostrar (que e o comportamento de sempre).
    jsonl, _ = _layout(tmp_path)
    Path(jsonl).unlink(missing_ok=True)
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is None
