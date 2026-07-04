"""Encadeamento de sessao (feature #12): vinculo 'then' de UM HOP (nao DAG) — quando a sessao FONTE
termina, dispara um prompt na sessao ALVO. Sidecar JSON por sessao FONTE, mesmo padrao do PromptQueue
(app/pqueue.py): dir irmao em ".claude-pocket-chain", keyed pelo NOME (sobrevive ao /clear, que so
troca o session-id/transcript). One JSON pequeno por sessao (nao uma lista) pq so existe 1 vinculo
por vez (one-shot: quem dispara consome e limpa — ver app.api._maybe_chain)."""
import json
from pathlib import Path

from app.config import settings
from app.pqueue import _sanitize


def _chain_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-chain"
    d.mkdir(parents=True, exist_ok=True)
    return d


class ThenLink:
    """Vinculo 'quando terminar -> enviar para' de UMA sessao FONTE (sidecar <nome>.json)."""

    def __init__(self, name: str):
        self.path = _chain_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            return json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None

    def set(self, target: str, text: str) -> None:
        # Escrita atomica (tmp + replace), mesmo padrao do PromptQueue._write_atomic.
        tmp = self.path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps({"target": target, "text": text}, ensure_ascii=False),
                        encoding="utf-8")
        tmp.replace(self.path)

    def clear(self) -> None:
        # Idempotente: chamado tanto no clear explicito (usuario) quanto no one-shot pos-disparo.
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)

    def rename(self, new_name: str) -> None:
        # Espelha PromptQueue.rename: a sessao renomeada nao pode perder o vinculo (nome velho vira
        # orfao). Vinculos de OUTRAS sessoes que apontem pro nome velho como alvo NAO sao migrados
        # (edge case raro; usuario reconfigura o alvo se precisar).
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)
        if self.path.exists():
            self.path.replace(_chain_dir() / f"{_sanitize(new_name)}.json")
