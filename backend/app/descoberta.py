"""Descoberta de máquinas na tailnet — "quem está online no Tailscale e responde como Hangar?".

Lê `tailscale status --json` (o mesmo que o alcance lê pro nome desta máquina) e bate, sem
credencial, em cada peer online. Um Hangar responde 401 com o `code` nomeado do auth — é a
assinatura que distingue ele de qualquer outro serviço na mesma porta. O token da outra máquina
nunca é descoberto: a pessoa cola no diálogo de adicionar, como hoje.

I/O pelo mesmo seam de `peers_check._bater` — o teste troca ele e não toca rede.
"""
from __future__ import annotations

import logging
from concurrent.futures import ThreadPoolExecutor

from app import alcance, peers_check
from app.config import settings

_log = logging.getLogger("hangar")
_CODIGO_HANGAR = "erro_nao_autorizado"


class SemTailscale(Exception):
    """`tailscale status` não respondeu: ausente, deslogado ou daemon parado — não é "ninguém achado"."""


def _hosts(peer: dict) -> tuple[list[str], str]:
    ips = [ip for ip in peer.get("TailscaleIPs") or [] if isinstance(ip, str) and ":" not in ip]
    return ips, (peer.get("DNSName") or "").rstrip(".")


def _candidatos(peer: dict) -> list[str]:
    # ponytail: assume a porta DESTA máquina na outra; tailscale serve entra como segunda tentativa.
    ips, dns = _hosts(peer)
    urls = [f"http://{ip}:{settings.port}" for ip in ips]
    if dns:
        urls.append(f"https://{dns}")
    return urls


def _e_hangar(url: str) -> bool:
    try:
        status, corpo = peers_check._bater(url, path="/api/peers/ping")
    except Exception as e:
        _log.debug("[descoberta] %s não respondeu: %s", url, alcance._motivo(e))
        return False
    if status == 200:
        return isinstance(corpo, dict) and corpo.get("hangar") is True
    if status != 404:
        return False
    # Hangar anterior ao /ping responde 404 ali; o 401 nomeado do auth na rota antiga ainda
    # identifica (custa uma tentativa errada lá — só nessas máquinas, até atualizarem).
    try:
        status, corpo = peers_check._bater(url)
    except Exception:
        return False
    return status == 401 and isinstance(corpo, dict) and isinstance(corpo.get("detail"), dict) \
        and corpo["detail"].get("code") == _CODIGO_HANGAR


def _sondar(peer: dict) -> dict | None:
    try:
        for url in _candidatos(peer):
            if _e_hangar(url):
                # `hosts` = todos os nomes pelos quais a máquina pode já estar registrada (o front
                # casa por host: registrada pelo nome e achada pelo IP é a MESMA máquina).
                ips, dns = _hosts(peer)
                return {"nome": peer.get("HostName") or "", "base_url": url, "hosts": ips + ([dns] if dns else [])}
    except Exception:
        # Entrada estranha no status de UM peer não derruba a busca inteira.
        _log.warning("[descoberta] peer ignorado: %r", peer.get("HostName"), exc_info=True)
    return None


def descobrir() -> list[dict]:
    """Máquinas online da tailnet que responderam como Hangar: [{nome, base_url, hosts}]."""
    status = alcance._status_tailscale()
    if not status:
        raise SemTailscale()
    online = [p for p in (status.get("Peer") or {}).values() if isinstance(p, dict) and p.get("Online")]
    if not online:
        return []
    with ThreadPoolExecutor(max_workers=min(8, len(online))) as ex:
        achados = list(ex.map(_sondar, online))
    return sorted((a for a in achados if a), key=lambda a: a["nome"])
