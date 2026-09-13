"""Sidecar durável das sessões Claude SEM terminal.

Não há pane: a sessão existe enquanto existir este arquivo. O processo `claude` é filho do
backend (morre com ele) e sobe de novo, com `--resume`, no próximo prompt — o que sobrevive é a
identidade (nome, cwd, session_id, conta, motor, escolhas de modelo). O histórico continua no
`.jsonl` do próprio Claude, como em qualquer sessão Claude.

Local: ~/.hangar/claude-headless/<nome>.json, um arquivo por sessão, keyed pelo nome sanitizado.
"""
import json
import tempfile
import threading
from pathlib import Path

from app import atomico
from app.names import sanitize_session_name

_write_lock = threading.Lock()


def _dir() -> Path:
    return Path.home() / ".hangar" / "claude-headless"


def _path(name: str) -> Path:
    return _dir() / f"{sanitize_session_name(name)}.json"


def _write(name: str, meta: dict) -> None:
    _dir().mkdir(parents=True, exist_ok=True)
    with _write_lock:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=_dir(), suffix=".tmp", delete=False) as tmp:
            json.dump(meta, tmp)
        try:
            atomico.substituir(tmp.name, _path(name))
        finally:
            Path(tmp.name).unlink(missing_ok=True)


def save(name: str, cwd: str, session_id: str, *, config_dir: str | None = None,
         engine: str | None = None, model: str | None = None, effort: str | None = None,
         context_window: int | None = None, permission_mode: str | None = None) -> dict:
    meta = {
        "name": name, "provider": "claude", "headless": True,
        "cwd": cwd, "session_id": session_id,
        "config_dir": config_dir, "engine": engine,
        "model": model, "effort": effort, "context_window": context_window,
        "permission_mode": permission_mode,
    }
    _write(name, meta)
    return meta


def update(name: str, **campos) -> dict | None:
    meta = load(name)
    if meta is None:
        return None
    meta = {**meta, **campos}
    _write(name, meta)
    return meta


def load(name: str) -> dict | None:
    try:
        return json.loads(_path(name).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def exists(name: str) -> bool:
    return _path(name).exists()


def delete(name: str) -> None:
    try:
        _path(name).unlink(missing_ok=True)
    except OSError:
        pass


def rename(old: str, new: str) -> None:
    src, dst = _path(old), _path(new)
    if not src.exists():
        return
    with _write_lock:
        atomico.substituir(src, dst)
    meta = load(new)
    if meta is not None:
        _write(new, {**meta, "name": new})


def list_all() -> list[dict]:
    out: list[dict] = []
    try:
        files = sorted(_dir().glob("*.json"))
    except OSError:
        return out
    for f in files:
        try:
            meta = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if isinstance(meta, dict) and meta.get("name") and meta.get("session_id"):
            out.append(meta)
    return out
