"""Statusline vinda de SIDECAR, quando existe — em vez do que sobrou dela no pane.

Quem desenha a statusline (o script do Claude Code, a extensao rich-status-line do Pi) corta o
texto na largura da janela ANTES de imprimir. Numa janela de 99 colunas a linha morre em
"💬 sessao 568kin/101kout · cache…": some a janela de contexto, some ⚡5h/📅7d, some o custo. O app
lia isso do pane e herdava o corte — o anel de contexto ficava "medicao indisponivel" so porque o
terminal estava estreito (medido 2026-07-30 numa sessão real).

Contrato: quem RENDERIZA a linha grava a versao inteira em
`<config>/.claude-pocket-status/<stem>.json` = {"line": str, "ts": epoch}, mesma chave dos outros
marcadores (o stem do .jsonl da sessao). Aqui so se le. Sem arquivo -> None e o caller segue com o
pane, que e o comportamento de antes: sessao sem o script instrumentado nao pode ficar sem linha
nenhuma.

Sem TTL curto de proposito: a linha do sidecar envelhece junto com a do pane (as duas saem do mesmo
render), entao descartar por idade so devolveria o texto cortado. O teto de 1 dia existe pra
marcador esquecido de uma sessao antiga cujo stem tenha voltado a existir.
"""
import json
import logging
import time
from pathlib import Path
from typing import Optional

from app.config import _backend_config_base, list_config_dirs

_log = logging.getLogger("claude_pocket.statusline")

_SUBDIR = ".claude-pocket-status"
_MAX_AGE = 86400.0

# Config dirs sao estaveis (mudam quando o usuario cria um ~/.claude-outro), e list_config_dirs faz
# glob + stat do disco. Cache com TTL curto: isto roda por sessao, a cada poll da lista.
_dirs_cache: tuple[float, list[Path]] = (0.0, [])
_DIRS_TTL = 60.0


def _dirs() -> list[Path]:
    global _dirs_cache
    now = time.monotonic()
    if now - _dirs_cache[0] < _DIRS_TTL and _dirs_cache[1]:
        return _dirs_cache[1]
    try:
        dirs = list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    except OSError:
        dirs = [_backend_config_base()]
    _dirs_cache = (now, dirs)
    return dirs


def read(stem: Optional[str]) -> Optional[str]:
    """Linha inteira publicada pela sessao `stem`, ou None (o caller cai no pane)."""
    if not stem:
        return None
    for base in _dirs():
        f = base / _SUBDIR / f"{stem}.json"
        try:
            o = json.loads(f.read_text(encoding="utf-8"))
        except OSError:
            continue                 # sidecar ausente e o caso NORMAL (sessao sem a extensao)
        except ValueError:
            # Arquivo existe e nao e JSON: o publisher grava por tmp+rename, entao isto nao devia
            # acontecer — e um bug de escrita, nao "sessao sem instrumentacao". Sem este debug os
            # dois casos ficam indistinguiveis de fora (os dois viram statusline do pane).
            _log.debug("statusline: sidecar ilegivel path=%s", f, exc_info=True)
            continue
        if not isinstance(o, dict):
            # JSON VALIDO do tipo errado (`null`, lista, string) nao levanta ValueError — o .get()
            # abaixo levantaria AttributeError, que ninguem pega: em registry.list_with_state isso
            # derruba a resolucao de estado de TODAS as sessoes, e em StateMonitor.stream() mata a
            # stream daquela sessao. Sidecar e conveniencia; nao pode derrubar nada.
            _log.debug("statusline: sidecar nao e objeto path=%s tipo=%s", f, type(o).__name__)
            continue
        line, ts = o.get("line"), o.get("ts")
        if not isinstance(line, str) or not line.strip():
            continue
        if isinstance(ts, (int, float)) and time.time() - ts > _MAX_AGE:
            continue
        return line
    return None
