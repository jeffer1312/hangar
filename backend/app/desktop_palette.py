"""Paleta Material You gerada pelo desktop a partir do papel de parede.

Fonte unica: `material_colors.scss`, escrito pelo quickshell (rice end-4/dots-hyprland) a cada
troca de wallpaper ou de esquema. Formato: uma variavel por linha, `$nome: valor;`.

Por que NAO o `colors.json` da mesma pasta: medido em 05/08/2026, ele traz a variante CLARA
(`background: #f9f9ff`) com a mesma hora de escrita, enquanto o `.scss` diz `$darkmode: True`.
Ler o json pinta o app de branco num desktop escuro.

Por que NAO o `terminal/kitty-theme.conf`: e outra derivacao, so pro terminal (`#171B20`), que nao
bate com `$background` nem com os `$surfaceContainer*`.

Stdlib pura de proposito — sem FastAPI e sem pydantic aqui: e o que deixa o parser testavel sem
subir o app, mesmo motivo que mantem `engines.py` enxuto.
"""
from __future__ import annotations

import logging
import re
from pathlib import Path

_log = logging.getLogger(__name__)


def _caminho() -> Path:
    # Funcao, e nao constante, porque o teste troca o caminho por monkeypatch — mesma costura de
    # `statusline._dirs`.
    return Path.home() / ".local/state/quickshell/user/generated/material_colors.scss"


# Os tokens que o mapeamento do front consome. Se UM faltar, a paleta inteira e recusada: meia
# paleta pintaria o fundo novo e deixaria o texto no valor velho, que e pior que nao mudar nada.
TOKENS: tuple[str, ...] = (
    "background",
    "surface",
    "surfaceContainerLow",
    "surfaceContainer",
    "surfaceContainerHigh",
    "onSurface",
    "onSurfaceVariant",
    "outline",
    "outlineVariant",
    "primary",
    "onPrimary",
)

_LINHA = re.compile(r"^\$([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^;]+);", re.MULTILINE)
_HEX = re.compile(r"^#[0-9A-Fa-f]{6}$")

# Ultimo motivo de falha ja avisado. O arquivo e relido a cada foco da janela (front chama
# GET /api/desktop/palette no focus/visibilitychange), entao logar SEMPRE que a paleta falha
# viraria um WARNING por foco, pra sempre, numa maquina cujo rice mudou de formato de vez — log que
# rola e log que ninguem le. So interessa a MUDANCA de estado.
_ultimo_aviso: str | None = None


def _avisar(msg: str) -> None:
    global _ultimo_aviso
    if msg != _ultimo_aviso:
        _log.warning(msg)
    _ultimo_aviso = msg


def parse(texto: str) -> dict | None:
    """Devolve `{"escuro": bool, "cores": {...}}`, ou None quando o texto nao serve.

    None e resposta, nao erro: arquivo de outro rice, arquivo truncado no meio de uma escrita, ou
    versao futura com outro formato — em todos, o certo e o app seguir com a paleta dele.
    """
    global _ultimo_aviso
    if not texto:
        return None
    # BOM na primeira linha (alguns editores/geradores gravam UTF-8 com BOM): sem o strip, o `^\$`
    # da regex (MULTILINE) nao casa o `$primeiro-token` e a paleta perde a primeira declaracao.
    if texto.startswith("﻿"):
        texto = texto[1:]
    bruto = {m.group(1): m.group(2).strip() for m in _LINHA.finditer(texto)}
    cores = {k: v for k, v in bruto.items() if _HEX.match(v)}
    faltando = [t for t in TOKENS if t not in cores]
    if faltando:
        # WARNING, nao DEBUG: o nivel efetivo do root e WARNING (main.py so eleva "claude_pocket"
        # pra INFO), entao um debug aqui nunca chega no stderr/journald mesmo com handler — o unico
        # sinal de que o rice mudou de formato ficava mudo. `getLogger(__name__)` (linha 22) ja e o
        # padrao usado por auth.py/engines.py/default_model.py/pi_inbox.py, que contam com esse
        # mesmo WARNING de root pra aparecer.
        _avisar(f"paleta do desktop incompleta, faltam {faltando}")
        return None
    _ultimo_aviso = None   # voltou a parsear certo — a proxima falha e um estado NOVO, avisa de novo
    # `$darkmode` vem como `True`/`False` (Python-style, e o gerador que escreve assim). Ausente,
    # assume escuro: e o default do proprio app, entao errar pra ca nao clareia a tela de ninguem.
    return {"escuro": bruto.get("darkmode", "True").strip().lower() != "false", "cores": cores}


def ler() -> dict | None:
    """Le o arquivo do disco. Ausente ou ilegivel -> None, nunca excecao."""
    global _ultimo_aviso
    try:
        texto = _caminho().read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        _ultimo_aviso = None   # sem rice e normal — o proximo problema de verdade e um estado NOVO
        return None            # a maioria das maquinas nao tem esse rice — silencio e a resposta certa
    except (OSError, RuntimeError) as e:
        # Permissao negada, erro de disco, laco de symlink, HOME ausente: NAO e rotina. `Path.home()`
        # (dentro de `_caminho()`, chamada aqui dentro do try) levanta RuntimeError — nao OSError —
        # quando $HOME nao esta setado e o passwd nao tem home dir, plausivel numa unit systemd
        # minima ou container. A feature degrada igual (o app fica com a paleta dele), mas quem for
        # depurar precisa de um rastro.
        _avisar(f"paleta do desktop ilegivel: {e!r}")
        return None
    return parse(texto)
