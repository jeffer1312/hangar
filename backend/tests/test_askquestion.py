import json
from pathlib import Path
from app.askquestion import read_pending_askq, clear_pending_askq
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
    # grava o sidecar do hook PreToolUse em <tmp>/.claude-pocket-askq/<sid>.json com stdin realista.
    proj = tmp_path / "projects" / "home-x"
    proj.mkdir(parents=True)
    jsonl = proj / f"{sid}.jsonl"
    sc_dir = tmp_path / ".claude-pocket-askq"
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
