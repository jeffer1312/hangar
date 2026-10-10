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
import time
import uuid
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
         context_window: int | None = None, permission_mode: str | None = None,
         previous_non_plan: str | None = None, subagent_model: str | None = None,
         jev: bool = False, key: str | None = None, transfer_id: str | None = None,
         engine_account: str | None = None, engine_credential_id: str | None = None,
         engine_account_base_url: str | None = None, service_tier: str | None = None,
         claude_settings: dict | None = None, read_only: bool = False) -> dict:
    if service_tier is not None and service_tier not in ("default", "priority"):
        raise ValueError("service_tier: use default ou priority")
    meta = {
        "name": name, "provider": "claude", "headless": True,
        # Identidade estável do processo pros scripts de dentro da sessão (hangar-send, hooks):
        # o nome muda no rename e o session_id no /clear; a chave, nunca. Vai no env como
        # CP_SESSION_KEY e o script acha o sidecar por ela.
        "key": key or uuid.uuid4().hex,
        "cwd": cwd, "session_id": session_id,
        "config_dir": config_dir, "engine": engine,
        "model": model, "effort": effort, "context_window": context_window,
        "permission_mode": permission_mode, "previous_non_plan": previous_non_plan,
        "subagent_model": subagent_model,
        # Escolhido na abertura: o adapter põe o ambiente do Jev no filho a partir daqui, e a
        # troca para terminal o repassa pro `-e` do pane.
        "jev": jev,
    }
    if service_tier is not None:
        meta["service_tier"] = service_tier
    if claude_settings is not None:
        meta["claude_settings"] = claude_settings
    if engine_account is not None:
        meta.update(engine_account=engine_account, engine_credential_id=engine_credential_id,
                    engine_account_base_url=engine_account_base_url)
    if transfer_id is not None:
        meta["transfer_id"] = transfer_id
    if read_only:
        # Estacionada só durante a troca de conta: o terminal reabre dentro do bwrap, nunca sem ele.
        meta["read_only"] = True
    _write(name, meta)
    return meta


def restaurar(meta: dict, *, preserve_process: bool = False) -> None:
    """Regrava o sidecar; preserva o cano apenas quando seu encerramento falhou."""
    _write(meta["name"], meta if preserve_process else {**meta, "cano": None})


# Troca terminal ⇄ sem terminal: por alguns segundos nenhum dos dois lados existe, e os monitores
# de estado diriam `dead` — o chat mostraria "sessão encerrada" até o SSE trocar de adapter.
_TROCA_S = 15.0
_trocando: dict[str, float] = {}


def marcar_troca(name: str) -> None:
    _trocando[name] = time.monotonic() + _TROCA_S
    from app import state_facts
    state_facts.notify(name)


def troca_restante_ms(name: str) -> int:
    """Quanto falta da janela de `marcar_troca`, em ms (0 sem troca marcada)."""
    return max(0, round((_trocando.get(name, 0.0) - time.monotonic()) * 1000))


def em_troca(name: str) -> bool:
    from app.conversation_transfer import transfer_active
    return _trocando.get(name, 0.0) > time.monotonic() or transfer_active(name)


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
