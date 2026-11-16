"""Sidecar durável das sessões Claude SEM terminal, gravado pelo Rust; aqui só a leitura.

Local: ~/.hangar/claude-headless/<nome>.json, um arquivo por sessão, keyed pelo nome sanitizado.
"""
import json
from pathlib import Path

from app import atomico
from app.names import sanitize_session_name


def _dir() -> Path:
    return Path.home() / ".hangar" / "claude-headless"


def _path(name: str) -> Path:
    return _dir() / f"{sanitize_session_name(name)}.json"


def exists(name: str) -> bool:
    return _path(name).exists()


def list_all() -> list[dict]:
    out: list[dict] = []
    try:
        files = sorted(_dir().glob("*.json"))
    except OSError:
        return out
    for f in files:
        try:
            meta = json.loads(atomico.read(f))
        except (OSError, ValueError):
            continue
        if isinstance(meta, dict) and meta.get("name") and meta.get("session_id"):
            out.append(meta)
    return out
