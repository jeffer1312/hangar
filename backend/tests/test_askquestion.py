import json
import time
from pathlib import Path
from app import askquestion
from app.askquestion import read_pending_askq, clear_pending_askq, pergunta_aberta


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


def test_pergunta_sem_a_chamada_no_transcript_expira(tmp_path):
    # Sidecar que ficou para trás: a chamada dele nunca chegou ao transcript, e responder dá 409.
    jsonl = str(tmp_path / "t.jsonl")
    _transcript(jsonl, _resposta("2020-01-01T00:00:00.000Z", "toolu_viva")[:1])
    agora = time.time()
    assert not askquestion._respondida_depois(jsonl, 10.0, "toolu_viva"), "chamada no transcript: aberta"
    assert not askquestion._respondida_depois(jsonl, agora, "toolu_outra"), "na carência: aberta"
    assert askquestion._respondida_depois(jsonl, 10.0, "toolu_outra"), "sem a chamada depois da carência"


def test_transcript_ilegivel_nao_inventa_pergunta(tmp_path, monkeypatch):
    # Sidecar aponta pra transcript que sumiu: sem como provar que segue aberta, cala — mostrar
    # pergunta ja respondida e pior que nao mostrar (que e o comportamento de sempre).
    jsonl, _ = _layout(tmp_path)
    Path(jsonl).unlink(missing_ok=True)
    _aponta(monkeypatch, tmp_path)
    assert pergunta_aberta("sess-123") is None
