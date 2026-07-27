"""Onde uma sessao do Pi vive no disco.

O Pi persiste toda sessao em JSONL, igual ao Claude, e aceita um id escolhido pelo caller
(`pi --session-id <id>`) — e por isso que o adapter dele e irmao do ClaudeAdapter e nao do
CodexAdapter: retomar e reabrir um arquivo, nao ressuscitar um app-server.

UMA diferenca de layout importa: o nome do arquivo tem timestamp na frente
(`<ts>_<uuid>.jsonl`), entao o path NAO e derivavel so do session-id como no Claude. Resolver
exige glob pelo sufixo.
"""
import os
import re
from pathlib import Path

# Todo nao-alfanumerico vira '-', e o resultado e envolvido por '-' ... '--'.
# Medido no Pi 0.82.1: /home/jefferson -> --home-jefferson--
_SANITIZE_RE = re.compile(r"[^A-Za-z0-9]")


def sessions_root() -> Path:
    # `--session-dir` do CLI sobrescreve o env; aqui so o env importa, porque as sessoes que o app
    # cria sao spawnadas por nos, sem esse flag.
    env = os.environ.get("PI_CODING_AGENT_SESSION_DIR")
    if env:
        return Path(env)
    return Path.home() / ".pi" / "agent" / "sessions"


def cwd_slug(cwd: str) -> str:
    return "-" + _SANITIZE_RE.sub("-", cwd) + "--"


def transcript_path(cwd: str, session_id: str) -> str:
    """Caminho do JSONL da sessao, ou "" se ela ainda nao escreveu nada.

    "" e nao excecao: o registry chama isto logo depois do spawn, e a TUI so cria o arquivo no
    primeiro turno. Levantar aqui transformaria "sessao novinha" em erro.
    """
    d = sessions_root() / cwd_slug(cwd)
    if not d.is_dir():
        return ""
    cands = sorted(d.glob(f"*_{session_id}.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)
    return str(cands[0]) if cands else ""
