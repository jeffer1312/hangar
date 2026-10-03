# backend/app/rust_bins.py
"""Onde estão os binários Rust do Hangar (`hangar-server`, `hangar-cano`).

Ordem: a variável de ambiente (escolha explícita, vence e não cai para as outras), o build
do checkout (`crates/target/release`, desenvolvimento) e o baixado em `~/.hangar/bin`.
"""
from __future__ import annotations

import logging
import os
from pathlib import Path

_log = logging.getLogger(__name__)
_REPO = Path(__file__).resolve().parents[2]


def _executavel(p: Path) -> bool:
    return p.is_file() and os.access(p, os.X_OK)


def find_bin(name: str, env_var: str) -> Path | None:
    exe = name + (".exe" if os.name == "nt" else "")
    escolhido = os.environ.get(env_var, "").strip()
    if escolhido:
        p = Path(escolhido).expanduser()
        if _executavel(p):
            return p
        # Caminho errado não pode virar outro binário calado: a escolha some e aparece no log.
        _log.warning("rust_bins: %s=%s não é executável; seguindo sem %s", env_var, escolhido, name)
        return None
    for p in (_REPO / "crates" / "target" / "release" / exe, Path.home() / ".hangar" / "bin" / exe):
        if _executavel(p):
            return p
    return None
