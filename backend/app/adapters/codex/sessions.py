"""Sidecar DURAVEL das sessoes Codex. Cada sessao tem uma TUI no tmux e um AppServerClient
WebSocket que o backend segura em memoria (efemero). Se o backend reiniciar, o processo app-server
morre, mas a IDENTIDADE (name/thread_id/rollout_path/cwd) sobrevive para recriar o servidor, retomar
a thread e ligar uma nova TUI tmux sob demanda (resume lazy). Este modulo grava essa identidade.

O historico do chat SEMPRE persiste no rollout JSONL do proprio Codex (~/.codex/sessions/...); aqui
so guardamos o ponteiro pra ele + o thread_id necessario pro thread/resume.

Local: ~/.claude-pocket/codex-sessions/<name>.json (mesma familia de ~/.claude-pocket usada pelo
sync-vault). Global por usuario (sessao Codex nao pertence a um config-dir do Claude). Um arquivo
por sessao, keyed pelo NOME sanitizado da sessao."""
import json
from pathlib import Path

from app.names import sanitize_session_name


def _dir() -> Path:
    # NAO cria o dir aqui (load/list nao devem ter efeito colateral); save() cria sob demanda.
    return Path.home() / ".claude-pocket" / "codex-sessions"


def _sanitize(name: str) -> str:
    # MESMA funcao do registry (app.names), nao uma copia da regra: a copia daqui nao recebeu a
    # correcao de acentuacao e teria trazido o bug de volta so no lado Codex no dia em que alguem
    # chamasse codex_sessions.* com um nome cru, sem passar pelo registry antes.
    return sanitize_session_name(name)


def _path(name: str) -> Path:
    return _dir() / f"{_sanitize(name)}.json"


def save(name: str, thread_id: str, rollout_path: str, cwd: str,
         model: str | None = None, effort: str | None = None) -> None:
    """Grava (ou sobrescreve) o sidecar duravel da sessao Codex. Escrita ATOMICA (tmp + replace,
    mesmo padrao de PromptQueue._write_atomic em pqueue.py) -- write_text direto podia corromper
    o sidecar em crash/concorrencia no meio da escrita.

    model/effort (Task C): escolha de modelo/reasoning effort da sessao, opcional -- None pra
    sessao nova (usa o default da thread) ou sidecar antigo (chave ausente = load().get() -> None,
    sem quebrar)."""
    _dir().mkdir(parents=True, exist_ok=True)
    p = _path(name)
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps({
        "name": name,
        "provider": "codex",
        "thread_id": thread_id,
        "rollout_path": rollout_path,
        "cwd": cwd,
        "model": model,
        "effort": effort,
    }), encoding="utf-8")
    tmp.replace(p)


def update_model(name: str, model: str | None, effort: str | None) -> None:
    """Atualiza SO a escolha de modelo/effort no sidecar existente, preservando thread_id/
    rollout_path/cwd (re-le e regrava via save()). No-op silencioso se o sidecar nao existe
    (nome desconhecido) -- quem chama (CodexAdapter.set_model) ja mantem a copia em memoria."""
    meta = load(name)
    if meta is None:
        return
    save(name, meta["thread_id"], meta["rollout_path"], meta["cwd"], model=model, effort=effort)


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


def rename(old: str, new: str) -> None:
    """Move o sidecar junto com a sessao tmux, preservando a identidade da thread."""
    src, dst = _path(old), _path(new)
    if not src.exists():
        return
    dst.parent.mkdir(parents=True, exist_ok=True)
    src.replace(dst)
    meta = load(new)
    if meta is not None:
        save(new, meta["thread_id"], meta["rollout_path"], meta["cwd"],
             model=meta.get("model"), effort=meta.get("effort"))


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
