"""Contexto de uma sessão Claude lido do transcript, sem depender da statusline.

A statusline só traz o contexto quando é a do Hangar; com outra, o app ficava sem o número. O
transcript sempre traz: o `usage` da última resposta do agente principal é o pedido inteiro
(entrada + cache lido + cache escrito). O tamanho da janela não vem nele (o id do modelo não diz
se é a versão de 1M), então sai do modelo configurado e do próprio uso.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

# Fim do arquivo lido: a última resposta fica perto do fim, e um transcript longo pesa megabytes.
_TAIL = 512 * 1024
WINDOW_DEFAULT = 200_000
WINDOW_1M = 1_000_000


def from_transcript(jsonl: str | Path | None, config_dir: str | Path | None = None) -> dict | None:
    """`{"used", "window"}` da última resposta do agente principal, ou None sem resposta com uso."""
    if not jsonl:
        return None
    try:
        with open(jsonl, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            fh.seek(max(0, fh.tell() - _TAIL))
            lines = fh.read().split(b"\n")
    except OSError:
        return None
    for line in reversed(lines):
        if b'"usage"' not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        # Subagente roda noutro contexto; a resposta sintética (erro, interrupção) não tem uso real.
        if not isinstance(obj, dict) or obj.get("type") != "assistant" or obj.get("isSidechain"):
            continue
        message = obj.get("message") or {}
        usage = message.get("usage") if isinstance(message, dict) else None
        if not isinstance(usage, dict) or message.get("model") == "<synthetic>":
            continue
        used = sum(int(usage.get(k) or 0) for k in
                   ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
        if used > 0:
            return {"used": used, "window": window(used, config_dir)}
    return None


def window(used: int, config_dir: str | Path | None = None) -> int:
    """1M quando o modelo configurado é a variante `[1m]` ou quando o uso já passou da janela
    padrão (só cabe na de 1M); senão a padrão."""
    if used > WINDOW_DEFAULT or _model_1m(config_dir):
        return WINDOW_1M
    return WINDOW_DEFAULT


def _model_1m(config_dir: str | Path | None) -> bool:
    base = Path(config_dir) if config_dir else Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude")
    try:
        model = json.loads((base / "settings.json").read_text(encoding="utf-8")).get("model")
    except (OSError, ValueError, AttributeError):
        return False
    return isinstance(model, str) and model.lower().endswith("[1m]")
