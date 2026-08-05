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


def parse(texto: str) -> dict | None:
    """Devolve `{"escuro": bool, "cores": {...}}`, ou None quando o texto nao serve.

    None e resposta, nao erro: arquivo de outro rice, arquivo truncado no meio de uma escrita, ou
    versao futura com outro formato — em todos, o certo e o app seguir com a paleta dele.
    """
    if not texto:
        return None
    bruto = {m.group(1): m.group(2).strip() for m in _LINHA.finditer(texto)}
    cores = {k: v for k, v in bruto.items() if _HEX.match(v)}
    faltando = [t for t in TOKENS if t not in cores]
    if faltando:
        _log.debug("paleta do desktop incompleta, faltam %s", faltando)
        return None
    # `$darkmode` vem como `True`/`False` (Python-style, e o gerador que escreve assim). Ausente,
    # assume escuro: e o default do proprio app, entao errar pra ca nao clareia a tela de ninguem.
    return {"escuro": bruto.get("darkmode", "True").strip().lower() != "false", "cores": cores}


def ler() -> dict | None:
    """Le o arquivo do disco. Ausente ou ilegivel -> None, nunca excecao."""
    try:
        texto = _caminho().read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None            # nao existe / sem permissao = normal, a maioria das maquinas
    return parse(texto)
