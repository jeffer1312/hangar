import json
import logging
import os
from pathlib import Path
from typing import Optional
from app.models import AskQuestion

_log = logging.getLogger("hangar.askquestion")

# Rabo do transcript lido pra achar a resposta da pergunta. Um turno inteiro cabe folgado; o
# arquivo pode ter dezenas de MB, entao e seek, nao varredura.
_SPAN_RESPOSTA = 256 * 1024


def _sidecar_path(jsonl: str) -> Path:
    # Deriva o sidecar a partir do path do jsonl do transcript. Ambos saem do mesmo config_dir:
    #   <config_dir>/projects/<sanitized_cwd>/<session_id>.jsonl   (transcript)
    #   <config_dir>/.hangar-askq/<session_id>.json         (sidecar do hook)
    p = Path(jsonl)
    return p.parents[2] / ".hangar-askq" / (p.stem + ".json")


def read_pending_askq(jsonl: str) -> AskQuestion | None:
    """Le o sidecar gravado pelo hook PreToolUse (askq_capture.py) e devolve o AskQuestion pendente,
    ou None. O sidecar fica em <config_dir>/.hangar-askq/<session_id>.json; ambos derivam do
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
        _log.warning("sidecar do askq malformado jsonl=%s", jsonl, exc_info=True)
        return None


def _quando(obj: dict) -> float:
    """Epoch do `timestamp` ISO da entrada, ou 0.0 se ilegivel."""
    from datetime import datetime
    ts = obj.get("timestamp")
    if not isinstance(ts, str):
        return 0.0
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return 0.0


def _respondida_depois(jsonl: str, desde: float) -> bool:
    """A ultima pergunta ja foi respondida DEPOIS de `desde` (mtime do sidecar)?

    O sidecar nao e apagado quando a resposta vem pela TUI (so no /answer), entao ele sozinho nao
    prova que a pergunta segue aberta. Quem prova e o transcript: responder grava o `tool_use` do
    AskUserQuestion e o `tool_result` dele. Sem transcript legivel devolve True — nao mostrar e o
    comportamento de sempre, mostrar pergunta ja respondida seria pior."""
    ids: set[str] = set()
    try:
        with open(jsonl, "rb") as fh:
            tam = fh.seek(0, os.SEEK_END)
            inicio = max(0, tam - _SPAN_RESPOSTA)
            fh.seek(inicio)
            linhas = fh.read().split(b"\n")
    except OSError:
        return True
    if inicio > 0:
        linhas = linhas[1:]        # o seek caiu no meio de uma linha
    for linha in linhas:
        if not linha.strip():
            continue
        try:
            obj = json.loads(linha.decode("utf-8", errors="replace"))
        except (json.JSONDecodeError, ValueError):
            continue
        if not isinstance(obj, dict):
            continue
        for b in ((obj.get("message") or {}).get("content") or []):
            if not isinstance(b, dict):
                continue
            if b.get("type") == "tool_use" and b.get("name") == "AskUserQuestion":
                ids.add(b.get("id"))
            elif b.get("type") == "tool_result" and b.get("tool_use_id") in ids \
                    and _quando(obj) > desde:
                return True
    return False


def pergunta_aberta(stem: Optional[str]) -> Optional[AskQuestion]:
    """Pergunta pendente da sessao `stem`, quando ela NAO esta visivel no pane.

    O menu do AskUserQuestion pode sair da area visivel do terminal — basta a TUI imprimir texto
    longo (o recado de outra sessao, por exemplo) e manter a viewport onde estava. Ai o `classify`
    nao ve menu nenhum, a sessao vira `idle`, o stepper nunca abre e a pergunta fica feita sem
    ninguem ser avisado. Aqui a fonte e o sidecar do hook PreToolUse, que independe do que coube na
    tela; o transcript diz se ela ja foi respondida."""
    if not stem:
        return None
    from app.statusline import dirs_de_config
    for base in dirs_de_config():
        f = base / ".hangar-askq" / f"{stem}.json"
        try:
            data = json.loads(f.read_text(encoding="utf-8"))
            quando = f.stat().st_mtime
        except OSError:
            continue                 # ausente e o caso NORMAL: nao ha pergunta pendente
        except ValueError:
            _log.debug("askq: sidecar ilegivel path=%s", f, exc_info=True)
            continue
        if not isinstance(data, dict):
            continue
        tp = data.get("transcript_path")
        if not isinstance(tp, str) or _respondida_depois(tp, quando):
            continue
        try:
            return AskQuestion.model_validate({"questions": data["tool_input"]["questions"]})
        except Exception:
            _log.warning("askq: sidecar malformado path=%s", f, exc_info=True)
    return None


def clear_pending_askq(jsonl: str) -> None:
    """Remove o sidecar do AskUserQuestion da sessao (idempotente; ignora ausencia/erro)."""
    try:
        _sidecar_path(jsonl).unlink(missing_ok=True)
    except Exception:
        pass
