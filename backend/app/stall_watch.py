"""Watchdog de sessao travada (feature #7): uma sessao presa em "working" sem avancar (loop infinito
de ferramenta, subprocesso esperando stdin) nunca vira awaiting/finished/dead -> nunca pinga sozinho.

Loop longo (mesmo padrao do hook_state.watch): a cada CP_STALL_POLL_SECONDS, reusa a lista JA
classificada (registry.list_with_state() deriva `stalled`; ver app/registry.py) em vez de escanear de
novo, e dispara push.notify_stalled() UMA vez por sessao ao entrar em stalled. Dedupe/re-arme aqui;
o bool `stalled` (pra UI) e derivado a parte, em list_with_state — concerns separados de proposito.
"""
import asyncio
import logging

from app.config import settings
from app import push

_log = logging.getLogger("claude_pocket.stall_watch")

# Sessoes ja notificadas na janela de stall atual. ponytail: set global — single-user, 1 backend;
# reset natural quando a sessao sai de stalled (state muda ou last_activity avanca).
_notified: set[str] = set()


async def _default_list_fn():
    # Import local: evita ciclo (sse importa registry; stall_watch e independente de ambos ate aqui).
    # Reusa o snapshot compartilhado do SSE (_cached_list, TTL curto) em vez de escanear tmux//proc de
    # novo — o list_events da lista ja costuma ter rodado ha <1s.
    from app import sse
    snap = [i.model_copy() for i in await sse._cached_list()]
    return await sse._list_registry.list_with_state(snap)


async def _tick(list_fn) -> None:
    """Uma varredura: notifica quem ENTROU em stalled desde a ultima vez; libera (re-arma) quem saiu."""
    infos = await list_fn()
    live = {i.name for i in infos if i.stalled}
    for name in live - _notified:
        push.notify_stalled(name)
    # _notified passa a ser EXATAMENTE `live`: quem segue travado nao re-notifica no proximo tick; quem
    # saiu (state mudou/last_activity avancou) sai do set e pode notificar de novo se travar outra vez.
    _notified.clear()
    _notified.update(live)


async def watch(list_fn=_default_list_fn, poll_seconds: float | None = None) -> None:
    """Loop longo: nunca levanta (falha de 1 ciclo nao mata o watchdog — so loga e tenta de novo)."""
    interval = settings.stall_poll_seconds if poll_seconds is None else poll_seconds
    while True:
        try:
            await _tick(list_fn)
        except Exception:
            _log.warning("stall watch: ciclo falhou", exc_info=True)
        await asyncio.sleep(interval)
