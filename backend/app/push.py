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

from app import atomico
from app.config import settings

_log = logging.getLogger("claude_pocket.push")
_lock = Lock()  # ponytail: lock global — single-user, baixa frequencia; por-endpoint so se virar gargalo

# Textos que o push monta sozinho, no idioma da inscricao (backend nao tem Paraglide e sao poucas
# frases). A redacao espelha frontend/messages/pt.json + en.json — se mudar la, muda aqui.
_MSG = {
    "pt": {
        "aguardando": "Aguardando sua resposta",
        "terminou": "Terminou",
        "caiu": "Caiu",
        "travada": "Pode estar travada",
        "limite": "Limite de uso atingido",
        "volta": "volta {reset}",
        "sessoes": "{n} sessões aguardando",
        "loop_done": "loop concluído: {reason}",
        "loop_done_claimed": "Claude declarou pronto — confirmar?",
        "loop_stopped": "loop parado: {reason}",
        "loop_exhausted": "loop esgotou as iterações: {reason}",
        "loop_failed": "loop falhou: {reason}",
    },
    "en": {
        "aguardando": "Awaiting your reply",
        "terminou": "Finished",
        "caiu": "Died",
        "travada": "May be stuck",
        "limite": "Usage limit reached",
        "volta": "resets {reset}",
        "sessoes": "{n} sessions waiting",
        "loop_done": "loop finished: {reason}",
        "loop_done_claimed": "Claude declared done — confirm?",
        "loop_stopped": "loop stopped: {reason}",
        "loop_exhausted": "loop exhausted its iterations: {reason}",
        "loop_failed": "loop failed: {reason}",
    },
}

# Coalescing do awaiting (feature #5): varias sessoes indo pra awaiting quase juntas colapsam numa
# unica notificacao agregada em vez de empilhar N. Buffer + timer sao globais (single-user, 1 backend).
_COALESCE_WINDOW = 2.0  # s de debounce
_COALESCE_TAG = "cp-awaiting-coalesced"  # tag CONSTANTE -> o SW substitui o card agregado, nao empilha
_coalesce_lock = Lock()
_coalesce_buf: dict[str, str] = {}  # session_name -> corpo (ultimo por sessao no ciclo atual)
_coalesce_timer: Timer | None = None


def _file() -> Path:
    d = Path(settings.projects_dir).parent / ".hangar-push"
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
    atomico.substituir(tmp, f)


def _load() -> list[dict]:
    try:
        return json.loads(_file().read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return []


def _save(subs: list[dict]) -> None:
    f = _file()
    tmp = f.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(subs, ensure_ascii=False), encoding="utf-8")
    atomico.substituir(tmp, f)


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


def _suppressed(session_name: str) -> bool:
    """True se este push deve ser silenciado: sessao mutada (mute por sessao) OU dentro da janela de
    quiet hours (global). Consultado no topo de TODO notify_* (awaiting/finished/dead/stalled/limited)
    -> quiet hours silencia TODOS os pushes; mutar uma sessao silencia TODOS os tipos de push dela."""
    return is_muted(session_name) or _in_quiet_hours()


def add_subscription(subscription: dict, label: str, server_id: str, locale: str = "pt") -> None:
    """Upsert por endpoint (idempotente: re-assinar nao duplica). label/server_id sao do CELULAR
    (nome amigavel + id local do servidor) -> a notificacao mostra 'Casa · sessao' e linka certo.
    locale e o idioma da inscricao (o front manda o escolhido; inscricao antiga sem o campo
    gravado cai em pt, o idioma de antes da internacionalizacao)."""
    endpoint = subscription.get("endpoint")
    if not endpoint:
        raise ValueError("subscription sem endpoint")
    with _lock:
        subs = [s for s in _load() if s.get("subscription", {}).get("endpoint") != endpoint]
        subs.append({"subscription": subscription, "label": label, "serverId": server_id,
                     "locale": locale})
        _save(subs)


def _msg(locale: str, chave: str, **params) -> str:
    """Texto do push no idioma da inscricao; locale desconhecido cai no pt (registro antigo)."""
    tabela = _MSG.get(locale, _MSG["pt"])
    return tabela[chave].format(**params) if params else tabela[chave]


def _send_one(entry: dict, session_name: str, body_fn, *, tag: str | None = None,
              title_suffix_fn=None, url: str | None = None) -> bool:
    """Envia 1 push. Retorna False se a inscricao morreu (404/410) -> caller poda.

    body_fn/title_suffix_fn recebem o locale da inscricao e devolvem o texto — o idioma do push e o
    da inscricao (cada celular tem o seu). title_suffix_fn substitui session_name SO no titulo
    (usado pelo push coalescido: "Casa · 3 sessões aguardando" em vez de "Casa · a, b, c"); tag
    explicito sobrepoe o default (session_name) pro SW substituir um card AGREGADO em vez de
    empilhar por sessao; url explicito pula o deep-link (o coalescido nao aponta pra 1 sessao so
    -> abre a lista)."""
    from pywebpush import webpush, WebPushException

    label = entry.get("label") or "claude"
    locale = entry.get("locale") or "pt"  # inscricao antiga (sem o campo) cai no pt
    body = body_fn(locale)
    suffix = title_suffix_fn(locale) if title_suffix_fn is not None else session_name
    payload = {
        "title": f"{label} · {suffix}",
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


def _broadcast(session_name: str, body_fn, *, tag: str | None = None,
              title_suffix_fn=None, url: str | None = None) -> None:
    """Manda push com o corpo dado (funcao do locale da inscricao) pra todas as inscricoes; poda as
    mortas. No-op se nao ha chaves VAPID configuradas (push desligado). Compartilhado pelos 3
    gatilhos (awaiting/finished/dead) — so o texto (e, no coalescido, tag/title/url) muda."""
    if not (settings.vapid_private and settings.vapid_public):
        return
    with _lock:
        subs = _load()
        if not subs:
            return
        alive = [s for s in subs if _send_one(s, session_name, body_fn, tag=tag,
                                              title_suffix_fn=title_suffix_fn, url=url)]
        if len(alive) != len(subs):
            _save(alive)


def notify_awaiting(session_name: str, body: str | None = None) -> None:
    """Push: sessao ficou awaiting_input (Claude esperando voce). body = a pergunta REAL (resolvida
    pelo caller via askquestion/classify) ou o fallback estatico (None) se nao deu pra ler nenhuma.

    Silenciada (mute por sessao) ou dentro da janela de quiet hours -> no-op (nem entra no buffer).
    Senao entra no buffer de coalescing: varios awaiting quase-simultaneos (~_COALESCE_WINDOW s)
    colapsam numa unica notificacao agregada em vez de empilhar N."""
    if _suppressed(session_name):
        return
    _queue_awaiting(session_name, body)


def _queue_awaiting(session_name: str, body: str | None) -> None:
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
        # body None = fallback estatico no idioma da inscricao; body real vem pronto do caller
        corpo = (lambda _l: _msg(_l, "aguardando")) if body is None else (lambda _l: body)
        _broadcast(name, corpo)
    else:
        names = ", ".join(sorted(buf))
        n = len(buf)
        _broadcast(names, lambda l: f"{_msg(l, 'sessoes', n=n)}: {names}",
                   tag=_COALESCE_TAG, title_suffix_fn=lambda l: _msg(l, "sessoes", n=n), url="/")


def notify_finished(session_name: str) -> None:
    """Push: sessao terminou um turno longo (working -> idle apos > CP_FINISH_MIN_SECONDS)."""
    if _suppressed(session_name):
        return
    _broadcast(session_name, lambda l: _msg(l, "terminou"))


def notify_dead(session_name: str) -> None:
    """Push: sessao morreu (tmux/pane caiu)."""
    if _suppressed(session_name):
        return
    _broadcast(session_name, lambda l: _msg(l, "caiu"))


def notify_stalled(session_name: str) -> None:
    """Push: sessao travada (feature #7) — "working" silencioso ha muito tempo (loop infinito de
    ferramenta, subprocesso esperando stdin) que nunca vira awaiting/finished/dead sozinho. Disparado
    UMA vez pelo watchdog (app.stall_watch); o dedupe/re-arme mora la, aqui e so o envio."""
    if _suppressed(session_name):
        return
    _broadcast(session_name, lambda l: _msg(l, "travada"))


def notify_limited(session_name: str, reset: str | None = None) -> None:
    """Push: sessao bateu no rate-limit de uso (feature #8) — banner de limite detectado no pane
    (best-effort, ver app.state.rate_limit_reset). Disparado UMA vez pelo watchdog (app.stall_watch,
    que reusa o MESMO ciclo do stall pra isto); dedupe/re-arme mora la, aqui e so o envio."""
    if _suppressed(session_name):
        return
    def corpo(locale: str) -> str:
        base = _msg(locale, "limite")
        return f"{base} · {_msg(locale, 'volta', reset=reset)}" if reset else base
    _broadcast(session_name, corpo)


def notify_loop(session_name: str, body: str) -> None:
    """Push do loop runner (harness bloco A). Envio burro (padrao notify_stalled); o dedupe mora
    no app.loop, por transicao de status do sidecar — aqui e so o envio. O body vem pronto do
    caller (loop._body, sempre em portugues): os cinco formatos fixos sao reconhecidos e a parte
    fixa sai no idioma da inscricao, com o dado dinamico (reason) intacto; corpo que nao casa com
    nenhum (caller custom, versao antiga do loop) segue cru como antes."""
    if _suppressed(session_name):
        return
    estado = _loop_estado(body)
    if estado is None:
        _broadcast(session_name, lambda _l: body)
        return
    status, reason = estado
    def corpo(locale: str) -> str:
        chave = f"loop_{status}"
        return _msg(locale, chave, reason=reason) if reason is not None else _msg(locale, chave)
    _broadcast(session_name, corpo)


def _loop_estado(body: str) -> tuple[str, str | None] | None:
    """Reconhece os formatos que loop._body produz (todos em pt) e devolve (status, reason).
    A fonte das frases fixas e a chave pt do _MSG: se o loop mudar a redacao, o parse para de
    casar e o corpo cai no caminho cru (perde a traducao, nunca quebra a notificacao)."""
    claimed = _MSG["pt"]["loop_done_claimed"]
    if body == claimed:
        return "done_claimed", None
    for status in ("done", "stopped", "exhausted", "failed"):
        prefixo = _MSG["pt"][f"loop_{status}"].split("{reason}")[0]
        if body.startswith(prefixo):
            return status, body[len(prefixo):]
    return None
