import json
import logging
from pathlib import Path
from app.models import AskQuestion


def _sidecar_path(jsonl: str) -> Path:
    # Deriva o sidecar a partir do path do jsonl do transcript. Ambos saem do mesmo config_dir:
    #   <config_dir>/projects/<sanitized_cwd>/<session_id>.jsonl   (transcript)
    #   <config_dir>/.claude-pocket-askq/<session_id>.json         (sidecar do hook)
    p = Path(jsonl)
    return p.parents[2] / ".claude-pocket-askq" / (p.stem + ".json")


def read_pending_askq(jsonl: str) -> AskQuestion | None:
    """Le o sidecar gravado pelo hook PreToolUse (askq_capture.py) e devolve o AskQuestion pendente,
    ou None. O sidecar fica em <config_dir>/.claude-pocket-askq/<session_id>.json; ambos derivam do
    path do jsonl do transcript: <config_dir>/projects/<sanitized_cwd>/<session_id>.jsonl."""
    try:
        data = json.loads(_sidecar_path(jsonl).read_text(encoding="utf-8"))
        return AskQuestion.model_validate({"questions": data["tool_input"]["questions"]})
    except FileNotFoundError:
        # Caminho NORMAL: nao ha pergunta pendente. Acontece em quase todo poll -> nunca loga.
        return None
    except Exception:
        # O arquivo EXISTE mas nao da pra ler: contrato do hook quebrado (askq_capture.py velho, ou
        # o Claude Code mudou o shape do tool_input). Sem este log isso ficava indistinguivel do
        # caso normal acima e o stepper simplesmente nunca mais abriria, calado.
        logging.getLogger("claude_pocket.askquestion").warning(
            "sidecar do askq malformado jsonl=%s", jsonl, exc_info=True)
        return None


def clear_pending_askq(jsonl: str) -> None:
    """Remove o sidecar do AskUserQuestion da sessao (idempotente; ignora ausencia/erro)."""
    try:
        _sidecar_path(jsonl).unlink(missing_ok=True)
    except Exception:
        pass
