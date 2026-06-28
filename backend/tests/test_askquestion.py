import json
from pathlib import Path
from app.askquestion import read_pending_askq, clear_pending_askq
from app.models import StateEvent
from app.sse import _ask_question_event


def _state_json(state: str, overlay: bool) -> str:
    return StateEvent(session="s", state=state, overlay=overlay).model_dump_json()


_Q = [{"header": "Cor", "question": "Escolha", "multiSelect": True,
       "options": [{"label": "A", "description": "op A"}, {"label": "B", "description": ""}]}]


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


# --- _ask_question_event (gate inalterado: so dispara em awaiting_input + overlay) ---

def test_ask_question_event_emits_when_awaiting_with_overlay(tmp_path):
    jsonl, _ = _layout(tmp_path)
    ev = _ask_question_event(_state_json("awaiting_input", overlay=True), jsonl)
    assert ev is not None
    assert ev["event"] == "ask_question"
    parsed = json.loads(ev["data"])
    assert parsed["questions"][0]["header"] == "Cor"


def test_ask_question_event_none_when_working(tmp_path):
    jsonl, _ = _layout(tmp_path)
    assert _ask_question_event(_state_json("working", overlay=False), jsonl) is None


def test_ask_question_event_none_when_no_overlay(tmp_path):
    # awaiting_input sem rodape de abas = menu nativo simples (nao AskUserQuestion tabulado)
    jsonl, _ = _layout(tmp_path)
    assert _ask_question_event(_state_json("awaiting_input", overlay=False), jsonl) is None
