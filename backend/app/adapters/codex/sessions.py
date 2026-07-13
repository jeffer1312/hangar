"""Sidecar DURAVEL das sessoes Codex (nao-tmux). Uma sessao Codex nao vive num pane tmux -- vive
num AppServerClient (stdio) que o backend segura em memoria (efemero). Se o backend reiniciar, o
processo app-server morre, mas a IDENTIDADE da sessao (name/thread_id/rollout_path/cwd) precisa
sobreviver pra ela reaparecer na lista e ser retomada sob demanda (resume lazy). Este modulo grava
essa identidade em disco.

O historico do chat SEMPRE persiste no rollout JSONL do proprio Codex (~/.codex/sessions/...); aqui
so guardamos o ponteiro pra ele + o thread_id necessario pro thread/resume.

Local: ~/.claude-pocket/codex-sessions/<name>.json (mesma familia de ~/.claude-pocket usada pelo
sync-vault). Global por usuario (sessao Codex nao pertence a um config-dir do Claude). Um arquivo
por sessao, keyed pelo NOME sanitizado da sessao."""
import json
import re
from pathlib import Path


def _dir() -> Path:
    # NAO cria o dir aqui (load/list nao devem ter efeito colateral); save() cria sob demanda.
    return Path.home() / ".claude-pocket" / "codex-sessions"


def _sanitize(name: str) -> str:
    # Mesma regra de nome do registry.create (nome vai virar basename de arquivo).
    return re.sub(r"[^A-Za-z0-9_-]", "-", name)


def _path(name: str) -> Path:
    return _dir() / f"{_sanitize(name)}.json"


def save(name: str, thread_id: str, rollout_path: str, cwd: str) -> None:
    """Grava (ou sobrescreve) o sidecar duravel da sessao Codex."""
    _dir().mkdir(parents=True, exist_ok=True)
    _path(name).write_text(json.dumps({
        "name": name,
        "provider": "codex",
        "thread_id": thread_id,
        "rollout_path": rollout_path,
        "cwd": cwd,
    }), encoding="utf-8")


def load(name: str) -> dict | None:
    """Le o sidecar de uma sessao (ou None se nao existe / corrompido)."""
    try:
        return json.loads(_path(name).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def delete(name: str) -> None:
    """Remove o sidecar (idempotente)."""
    try:
        _path(name).unlink(missing_ok=True)
    except OSError:
        pass


def list_all() -> list[dict]:
    """Todas as sessoes Codex gravadas (pula arquivos corrompidos). Usado pelo registry.list()."""
    out: list[dict] = []
    try:
        files = sorted(_dir().glob("*.json"))
    except OSError:
        return out
    for f in files:
        try:
            out.append(json.loads(f.read_text(encoding="utf-8")))
        except (OSError, ValueError):
            continue
    return out


def exists(name: str) -> bool:
    return _path(name).exists()
