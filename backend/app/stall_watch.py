"""Watchdog de sessao travada (feature #7): uma sessao presa em "working" sem avancar (loop infinito
de ferramenta, subprocesso esperando stdin) nunca vira awaiting/finished/dead -> nunca pinga sozinho.

Loop longo (mesmo padrao do hook_state.watch): a cada CP_STALL_POLL_SECONDS, reusa a lista JA
classificada (registry.list_with_state() deriva `stalled`; ver app/registry.py) em vez de escanear de
novo, e dispara push.notify_stalled() UMA vez por sessao ao entrar em stalled. Dedupe/re-arme aqui;
o bool `stalled` (pra UI) e derivado a parte, em list_with_state — concerns separados de proposito.

PIGGYBACK neste MESMO ciclo (em vez de loops/Timers proprios) tambem: (a) o rate-limit radar (feature
#8, campo `limited`) — dedupe/re-arme espelha o padrao do stall, so trocando o campo; (b) o death ping
(feature #2) — o marcador do state_hook NUNCA emite "dead", entao o push de "caiu" nao teria gatilho;
aqui detectamos morte pelo diff do conjunto vivo entre ciclos (sessao que estava na lista e sumiu).
Alem do push (rate-limit), tenta o auto-resume
opt-in (CP_AUTO_RESUME): se a sessao tem fila nao-entregue e o reset parseia num horario confiavel,
arma um Timer pra drenar sozinha (ver _maybe_auto_resume). O auto-resume tambem exige o kill-switch
MESTRE (feature #12, app.config.automations_enabled) — o mesmo portao que governa o encadeamento de
sessao em app/chain.py + app/api.py.
"""
import asyncio
import logging
import re
import threading
from datetime import datetime, timedelta
from pathlib import Path

from app.config import settings, automations_enabled
from app import push

_log = logging.getLogger("claude_pocket.stall_watch")

# Sessoes ja notificadas na janela de stall atual. ponytail: set global — single-user, 1 backend;
# reset natural quando a sessao sai de stalled (state muda ou last_activity avanca).
_notified: set[str] = set()

# Death ping (feature #2): o marcador do state_hook so emite working/awaiting_input/idle — NUNCA
# "dead" — entao o branch dead de _on_hook_transition nunca dispara e o push de "caiu" ficava sem
# gatilho. Sinal confiavel de morte = uma sessao que ESTAVA na lista e sumiu no ciclo seguinte. Este
# watchdog ja varre tudo a cada poll, entao detecta aqui pelo diff do conjunto vivo entre ciclos.
# _seen_live: nome -> session_id (stem do jsonl) da ultima varredura, guardado pra limpar o
# _working_started (api.py, keyed por session_id) da sessao morta. _notified_dead: dedupe (re-arma
# quando o nome reaparece vivo). ponytail: dict/set globais — single-user, 1 backend, igual _notified.
_seen_live: dict[str, str | None] = {}
_notified_dead: set[str] = set()

# Mesmo padrao acima, pro rate-limit (feature #8): sessoes ja notificadas na janela LIMITED atual.
_notified_limited: set[str] = set()
# Sessoes com auto-resume ja ARMADO (Timer pendente) nesta janela limited -- nao arma 2 Timers pro
# mesmo periodo (o watchdog roda a cada poll enquanto a sessao segue limited).
_armed_limited: set[str] = set()

# "HH[:MM][am|pm]" -- o formato que rate_limit_reset (app.state) capturou do banner. ponytail:
# calibration knob junto com _LIMIT_RE (app/state.py): sem fuso horario (assume o do SISTEMA).
_RESET_TIME_RE = re.compile(r"^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$", re.I)


def _parse_reset_delay(reset: str, now: datetime | None = None) -> float | None:
    """Segundos ate o horario de reset, ou None se nao der pra parsear com confianca. NAO arma
    Timer sobre um parse ruim -- ver _maybe_auto_resume."""
    m = _RESET_TIME_RE.match(reset.strip())
    if not m:
        return None
    hour = int(m.group(1))
    minute = int(m.group(2) or 0)
    ampm = (m.group(3) or "").lower()
    if ampm == "pm" and hour != 12:
        hour += 12
    elif ampm == "am" and hour == 12:
        hour = 0
    if not (0 <= hour <= 23 and 0 <= minute <= 59):
        return None
    now = now or datetime.now()
    target = now.replace(hour=hour, minute=minute, second=0, microsecond=0)
    if target <= now:
        target += timedelta(days=1)  # horario ja passou hoje -> e amanha
    return (target - now).total_seconds()


def _has_undelivered_queue(name: str) -> bool:
    # Mesmo cheap-check do drain() (app.terminal_input): so entradas delivered=False contam como
    # "trabalho pendente" pro auto-resume (as ja entregues/confirmadas nao precisam de drain).
    from app.pqueue import PromptQueue
    return any(e.get("delivered") is False for e in PromptQueue(name).load())


def _maybe_auto_resume(name: str, reset: str | None) -> None:
    """Arma o auto-resume (feature #8, opt-in via CP_AUTO_RESUME) SE: o kill-switch MESTRE
    (automations_enabled, feature #12) E a flag propria estao ligados, ha fila NAO entregue pra esta
    sessao, e o reset parseia num delay confiavel. So arma 1x por janela limited (ver _armed_limited);
    sem reset parseavel, so loga e desiste (nao arma timer sobre chute ruim -- prefere SHIP
    deteccao+chip+push a arriscar um drain na hora errada)."""
    if not automations_enabled() or not settings.auto_resume or name in _armed_limited or not reset:
        return
    if not _has_undelivered_queue(name):
        return
    delay = _parse_reset_delay(reset)
    if delay is None:
        _log.info("auto-resume: reset '%s' nao parseou -- nao armando timer (sessao %s)", reset, name)
        return
    from app.api import _drain_session  # import local: api importa stall_watch (evita ciclo)
    _log.info("auto-resume: sessao %s limited, armando drain em %.0fs (reset=%s)", name, delay, reset)
    t = threading.Timer(delay, _drain_session, args=(name,))
    t.daemon = True  # nao segura o interpretador vivo (delay pode ser horas) — igual ao coalesce de push.py
    t.start()
    _armed_limited.add(name)


async def _default_list_fn():
    # Import local: evita ciclo (sse importa registry; stall_watch e independente de ambos ate aqui).
    # Reusa o snapshot compartilhado do SSE (_cached_list, TTL curto) em vez de escanear tmux//proc de
    # novo — o list_events da lista ja costuma ter rodado ha <1s.
    from app import sse
    snap = [i.model_copy() for i in await sse._cached_list()]
    return await sse._list_registry.list_with_state(snap)


async def _tick(list_fn) -> None:
    """Uma varredura: notifica quem ENTROU em stalled/limited desde a ultima vez; libera (re-arma)
    quem saiu de cada um (concerns independentes -- uma sessao pode estar nos dois, so um, ou nenhum)."""
    infos = await list_fn()

    # Death ping (feature #2): quem estava vivo no ciclo anterior e sumiu agora = morreu (ver docstring
    # dos sets no topo). Dispara notify_dead 1x (respeita CP_NOTIFY_DEAD) e limpa o _working_started da
    # api pra a sessao morta nao vazar no dict. Dedupe/re-arme por _notified_dead.
    now_live = {i.name: (Path(i.jsonl).stem if getattr(i, "jsonl", None) else None) for i in infos}
    for name in _seen_live.keys() - now_live.keys():
        if name in _notified_dead:
            continue
        if settings.notify_dead:
            push.notify_dead(name)
        sid = _seen_live.get(name)
        if sid:
            from app import api  # import local: api importa stall_watch (evita ciclo)
            api._working_started.pop(sid, None)
        _notified_dead.add(name)
    _notified_dead.difference_update(now_live)  # reaparecer vivo -> pode re-notificar se morrer de novo
    _seen_live.clear()
    _seen_live.update(now_live)

    live = {i.name for i in infos if i.stalled}
    for name in live - _notified:
        push.notify_stalled(name)
    # _notified passa a ser EXATAMENTE `live`: quem segue travado nao re-notifica no proximo tick; quem
    # saiu (state mudou/last_activity avancou) sai do set e pode notificar de novo se travar outra vez.
    _notified.clear()
    _notified.update(live)

    # Feature #8: mesmo padrao acima, pro rate-limit (`limited`). Piggyback neste ciclo (ver docstring
    # do modulo) -- reusa a MESMA lista ja classificada, sem 2o loop.
    limited_reset = {i.name: i.limit_reset for i in infos if i.limited}
    limited_now = set(limited_reset)
    for name in limited_now - _notified_limited:
        push.notify_limited(name, limited_reset[name])
        _maybe_auto_resume(name, limited_reset[name])
    _notified_limited.clear()
    _notified_limited.update(limited_now)
    # Saiu de limited -> pode re-armar auto-resume se voltar a entrar (nao afeta o Timer ja disparado).
    _armed_limited.intersection_update(limited_now)


async def watch(list_fn=_default_list_fn, poll_seconds: float | None = None) -> None:
    """Loop longo: nunca levanta (falha de 1 ciclo nao mata o watchdog — so loga e tenta de novo)."""
    interval = settings.stall_poll_seconds if poll_seconds is None else poll_seconds
    while True:
        try:
            await _tick(list_fn)
        except Exception:
            _log.warning("stall watch: ciclo falhou", exc_info=True)
        await asyncio.sleep(interval)
