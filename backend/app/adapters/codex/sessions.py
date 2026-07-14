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
