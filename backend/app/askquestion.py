import json
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
    except Exception:
        # fail-soft: arquivo ausente, JSON invalido, chave faltando ou payload malformado -> None.
        return None


def clear_pending_askq(jsonl: str) -> None:
    """Remove o sidecar do AskUserQuestion da sessao (idempotente; ignora ausencia/erro)."""
    try:
        _sidecar_path(jsonl).unlink(missing_ok=True)
    except Exception:
        pass
