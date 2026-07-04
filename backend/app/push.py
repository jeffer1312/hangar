"""Web Push: avisa o celular quando uma sessao fica awaiting_input (Claude esperando voce).

Disparado pelo hook Notification do Claude Code (via hook_state) -> funciona com o app FECHADO.
Inscricoes ficam num arquivo duravel; o envio usa pywebpush + VAPID compartilhado (config). Sem
chaves VAPID o modulo degrada gracioso (subscribe guarda, mas send vira no-op silencioso).
"""
import json
import logging
from datetime import datetime, time as dtime
from pathlib import Path
from threading import Lock, Timer

from app.config import settings

_log = logging.getLogger("claude_pocket.push")
_lock = Lock()  # ponytail: lock global — single-user, baixa frequencia; por-endpoint so se virar gargalo

# Coalescing do awaiting (feature #5): varias sessoes indo pra awaiting quase juntas colapsam numa
# unica notificacao agregada em vez de empilhar N. Buffer + timer sao globais (single-user, 1 backend).
_COALESCE_WINDOW = 2.0  # s de debounce
_COALESCE_TAG = "cp-awaiting-coalesced"  # tag CONSTANTE -> o SW substitui o card agregado, nao empilha
_coalesce_lock = Lock()
_coalesce_buf: dict[str, str] = {}  # session_name -> corpo (ultimo por sessao no ciclo atual)
_coalesce_timer: Timer | None = None


def _file() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-push"
    d.mkdir(parents=True, exist_ok=True)
    return d / "subs.json"


def _prefs_file() -> Path:
    # Mesma pasta do subs.json (deriva de _file() -> respeita o monkeypatch dos testes de graca).
    return _file().parent / "push_prefs.json"


def _load_prefs() -> dict:
    try:
        return json.loads(_prefs_file().read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def _save_prefs(data: dict) -> None:
    f = _prefs_file()
    tmp = f.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    tmp.replace(f)


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


def set_muted(session_name: str, muted: bool) -> None:
    """Liga/desliga o silencio de push (so awaiting, feature #5) pra UMA sessao. Persistido em
    push_prefs.json -> sobrevive restart do backend."""
    with _lock:
        data = _load_prefs()
        s = set(data.get("muted", []))
        if muted:
            s.add(session_name)
        else:
            s.discard(session_name)
        data["muted"] = sorted(s)
        _save_prefs(data)


def is_muted(session_name: str) -> bool:
    return session_name in _load_prefs().get("muted", [])


def set_quiet_hours(start: str | None, end: str | None) -> None:
    """Janela de silencio GLOBAL (HH:MM-HH:MM, pode cruzar meia-noite) pro push de awaiting.
    start/end None (qualquer um) desliga a janela. Levanta ValueError em horario invalido."""
    with _lock:
        data = _load_prefs()
        if start and end:
            try:
                dtime.fromisoformat(start)
                dtime.fromisoformat(end)
            except ValueError:
                raise ValueError("horario invalido (use HH:MM)")
            data["quiet_hours"] = {"start": start, "end": end}
        else:
            data.pop("quiet_hours", None)
        _save_prefs(data)


def get_push_prefs() -> dict:
    """Estado atual pro app mostrar: {"muted": [...], "quiet_hours": {"start","end"} | None}."""
    data = _load_prefs()
    return {"muted": data.get("muted", []), "quiet_hours": data.get("quiet_hours")}


def _in_quiet_hours(now: dtime | None = None) -> bool:
    qh = _load_prefs().get("quiet_hours")
    if not qh:
        return False
    now = now or datetime.now().time()
    start, end = dtime.fromisoformat(qh["start"]), dtime.fromisoformat(qh["end"])
    if start <= end:
        return start <= now < end
    return now >= start or now < end  # janela cruza meia-noite (ex: 22:00-07:00)


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


def _send_one(entry: dict, session_name: str, body: str, *, tag: str | None = None,
              title_suffix: str | None = None, url: str | None = None) -> bool:
    """Envia 1 push. Retorna False se a inscricao morreu (404/410) -> caller poda.

    title_suffix substitui session_name SO no titulo (usado pelo push coalescido: "Casa · 3 sessões
    aguardando" em vez de "Casa · a, b, c"); tag explicito sobrepoe o default (session_name) pro SW
    substituir um card AGREGADO em vez de empilhar por sessao; url explicito pula o deep-link (o
    coalescido nao aponta pra 1 sessao so -> abre a lista)."""
    from pywebpush import webpush, WebPushException

    label = entry.get("label") or "claude"
    payload = {
        "title": f"{label} · {title_suffix if title_suffix is not None else session_name}",
        "body": body,
        "session": session_name,
        # deep-link best-effort (App pode honrar ?server/?session; senao so abre o app)
        "url": url if url is not None else f"/?server={entry.get('serverId', '')}&session={session_name}",
    }
    if tag:
        payload["tag"] = tag
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


def _broadcast(session_name: str, body: str, *, tag: str | None = None,
              title_suffix: str | None = None, url: str | None = None) -> None:
    """Manda push com o corpo dado pra todas as inscricoes; poda as mortas. No-op se nao ha chaves
    VAPID configuradas (push desligado). Compartilhado pelos 3 gatilhos (awaiting/finished/dead) —
    so o texto (e, no coalescido, tag/title/url) muda."""
    if not (settings.vapid_private and settings.vapid_public):
        return
    with _lock:
        subs = _load()
        if not subs:
            return
        alive = [s for s in subs if _send_one(s, session_name, body, tag=tag,
                                              title_suffix=title_suffix, url=url)]
        if len(alive) != len(subs):
            _save(alive)


def notify_awaiting(session_name: str, body: str = "Aguardando sua resposta") -> None:
    """Push: sessao ficou awaiting_input (Claude esperando voce). body = a pergunta REAL (resolvida
    pelo caller via askquestion/classify) ou o fallback estatico se nao deu pra ler nenhuma.

    Silenciada (mute por sessao) ou dentro da janela de quiet hours -> no-op (nem entra no buffer).
    Senao entra no buffer de coalescing: varios awaiting quase-simultaneos (~_COALESCE_WINDOW s)
    colapsam numa unica notificacao agregada em vez de empilhar N."""
    if is_muted(session_name) or _in_quiet_hours():
        return
    _queue_awaiting(session_name, body)


def _queue_awaiting(session_name: str, body: str) -> None:
    global _coalesce_timer
    with _coalesce_lock:
        _coalesce_buf[session_name] = body
        if _coalesce_timer is None:
            _coalesce_timer = Timer(_COALESCE_WINDOW, _flush_coalesce)
            _coalesce_timer.daemon = True  # nao segura o processo vivo
            _coalesce_timer.start()


def _flush_coalesce() -> None:
    """Envia o(s) push acumulado(s): 1 sessao -> push rico normal; N sessoes -> 1 push agregado
    "N sessões aguardando: A, B, C" com tag CONSTANTE (o SW substitui o card anterior em vez de
    empilhar). Chamado pelo Timer em producao; testes chamam direto pra nao esperar o debounce real."""
    global _coalesce_timer
    with _coalesce_lock:
        buf = dict(_coalesce_buf)
        _coalesce_buf.clear()
        _coalesce_timer = None
    if not buf:
        return
    if len(buf) == 1:
        (name, body), = buf.items()
        _broadcast(name, body)
    else:
        names = ", ".join(sorted(buf))
        _broadcast(names, f"{len(buf)} sessões aguardando: {names}",
                  tag=_COALESCE_TAG, title_suffix=f"{len(buf)} sessões aguardando", url="/")


def notify_finished(session_name: str) -> None:
    """Push: sessao terminou um turno longo (working -> idle apos > CP_FINISH_MIN_SECONDS)."""
    _broadcast(session_name, "Terminou")


def notify_dead(session_name: str) -> None:
    """Push: sessao morreu (tmux/pane caiu)."""
    _broadcast(session_name, "Caiu")


def notify_stalled(session_name: str) -> None:
    """Push: sessao travada (feature #7) — "working" silencioso ha muito tempo (loop infinito de
    ferramenta, subprocesso esperando stdin) que nunca vira awaiting/finished/dead sozinho. Disparado
    UMA vez pelo watchdog (app.stall_watch); o dedupe/re-arme mora la, aqui e so o envio."""
    _broadcast(session_name, "Pode estar travada")
