"""Web Push: avisa o celular quando uma sessao fica awaiting_input (Claude esperando voce).

Disparado pelo hook Notification do Claude Code (via hook_state) -> funciona com o app FECHADO.
Inscricoes ficam num arquivo duravel; o envio usa pywebpush + VAPID compartilhado (config). Sem
chaves VAPID o modulo degrada gracioso (subscribe guarda, mas send vira no-op silencioso).
"""
import json
import logging
from pathlib import Path
from threading import Lock

from app.config import settings

_log = logging.getLogger("claude_pocket.push")
_lock = Lock()  # ponytail: lock global — single-user, baixa frequencia; por-endpoint so se virar gargalo


def _file() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-push"
    d.mkdir(parents=True, exist_ok=True)
    return d / "subs.json"


def _load() -> list[dict]:
    try:
        return json.loads(_file().read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return []


def _save(subs: list[dict]) -> None:
    f = _file()
    tmp = f.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(subs, ensure_ascii=False), encoding="utf-8")
    tmp.replace(f)


def add_subscription(subscription: dict, label: str, server_id: str) -> None:
    """Upsert por endpoint (idempotente: re-assinar nao duplica). label/server_id sao do CELULAR
    (nome amigavel + id local do servidor) -> a notificacao mostra 'Casa · sessao' e linka certo."""
    endpoint = subscription.get("endpoint")
    if not endpoint:
        raise ValueError("subscription sem endpoint")
    with _lock:
        subs = [s for s in _load() if s.get("subscription", {}).get("endpoint") != endpoint]
        subs.append({"subscription": subscription, "label": label, "serverId": server_id})
        _save(subs)


def _send_one(entry: dict, session_name: str, body: str) -> bool:
    """Envia 1 push. Retorna False se a inscricao morreu (404/410) -> caller poda."""
    from pywebpush import webpush, WebPushException

    label = entry.get("label") or "claude"
    payload = {
        "title": f"{label} · {session_name}",
        "body": body,
        "session": session_name,
        # deep-link best-effort (App pode honrar ?server/?session; senao so abre o app)
        "url": f"/?server={entry.get('serverId', '')}&session={session_name}",
    }
    try:
        webpush(
            subscription_info=entry["subscription"],
            data=json.dumps(payload, ensure_ascii=False),
            vapid_private_key=settings.vapid_private,
            vapid_claims={"sub": settings.vapid_subject},
        )
        return True
    except WebPushException as e:
        code = getattr(getattr(e, "response", None), "status_code", None)
        if code in (404, 410):
            return False  # inscricao expirada -> podar
        _log.warning("webpush falhou (%s): %s", code, e)
        return True  # erro transitorio: mantem a inscricao
    except Exception as e:  # noqa: BLE001 — nunca derruba o watcher por causa de push
        _log.warning("webpush erro: %s", e)
        return True


def _broadcast(session_name: str, body: str) -> None:
    """Manda push com o corpo dado pra todas as inscricoes; poda as mortas. No-op se nao ha chaves
    VAPID configuradas (push desligado). Compartilhado pelos 3 gatilhos (awaiting/finished/dead) —
    so o texto muda."""
    if not (settings.vapid_private and settings.vapid_public):
        return
    with _lock:
        subs = _load()
        if not subs:
            return
        alive = [s for s in subs if _send_one(s, session_name, body)]
        if len(alive) != len(subs):
            _save(alive)


def notify_awaiting(session_name: str) -> None:
    """Push: sessao ficou awaiting_input (Claude esperando voce)."""
    _broadcast(session_name, "Aguardando sua resposta")


def notify_finished(session_name: str) -> None:
    """Push: sessao terminou um turno longo (working -> idle apos > CP_FINISH_MIN_SECONDS)."""
    _broadcast(session_name, "Terminou")


def notify_dead(session_name: str) -> None:
    """Push: sessao morreu (tmux/pane caiu)."""
    _broadcast(session_name, "Caiu")
