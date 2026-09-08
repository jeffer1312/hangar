"""Fala com o servidor do app desktop (`shell/preview_srv.cjs`) — os mesmos verbos do `hangar-preview`.

Existe porque nem tudo do navegador embutido cabe no CDP direto: o layout (desktop/celular) é
emulação que se perde ao navegar, e quem sabe repor é o controlador do shell. Uma fonte de verdade
só, e ela mora lá.

Endereço e token saem de `~/.hangar/nav/_srv.json`, que o shell reescreve a cada subida — a porta é
efêmera e o token sorteado, então nada aqui pode ser cacheado entre chamadas.
"""
from __future__ import annotations

import json
import logging
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Optional

_log = logging.getLogger("hangar.navshell")

TETO_S = 20.0


class ShellIndisponivel(RuntimeError):
    pass


def _servidor() -> dict[str, Any]:
    arq = Path.home() / ".hangar" / "nav" / "_srv.json"
    try:
        dados = json.loads(arq.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise ShellIndisponivel("o app desktop não está aberto nesta máquina") from e
    if not isinstance(dados, dict) or not dados.get("porta") or not dados.get("token"):
        raise ShellIndisponivel("o app desktop não está aberto nesta máquina")
    return dados


def _chave(name: str) -> Optional[str]:
    """A chave `<servidor>::<sessão>` que o shell usa, lida do sidecar daquele navegador."""
    from app.navsock import alvo_da_sessao
    sc = alvo_da_sessao(name)
    return str(sc.get("chave")) if sc and sc.get("chave") else None


def verbo(name: str, verbo: str, args: Optional[list[str]] = None) -> str:
    """Roda um verbo do navegador daquela sessão. Levanta `ShellIndisponivel` sem app desktop."""
    srv = _servidor()
    chave = _chave(name)
    if not chave:
        raise ShellIndisponivel("esta sessão não tem navegador aberto")
    corpo = json.dumps({"chave": chave, "verbo": verbo, "args": args or []}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{int(srv['porta'])}/cmd", data=corpo, method="POST",
        headers={"Authorization": f"Bearer {srv['token']}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=TETO_S) as r:   # noqa: S310 — loopback fixo
            return r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.read().decode("utf-8", "replace")
    except OSError as e:
        raise ShellIndisponivel(f"o app desktop não respondeu: {e}") from e
