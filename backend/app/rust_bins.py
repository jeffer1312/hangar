# backend/app/rust_bins.py
"""Onde estão os binários Rust do Hangar (`hangar-server`, `hangar-cano`).

Ordem: a variável de ambiente (escolha explícita, vence e não cai para as outras); sem ela, o
`bin/` da raiz da instalação (pacote); senão o mais novo entre o build do checkout (`crates/target/release`, desenvolvimento) e o baixado em
`~/.hangar/bin`.
"""
from __future__ import annotations

import logging
import os
import time
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
    # Pacote: o binário da versão em uso, pelo `current` (a raiz de `app/install_root.py`, repetida
    # porque o `hangar-open` carrega este arquivo solto, sem o pacote `app`), vence os outros dois.
    # Pelo caminho real da versão: o processo de pé não troca de arquivo quando o `current` muda.
    current = Path.home() / ".hangar" / "current"
    # A troca no Windows deixa só o `current.old` por um instante: espera o link novo até 1 s.
    limite = time.monotonic() + 1.0
    while (not os.path.lexists(current) and os.path.lexists(current.with_name("current.old"))
           and time.monotonic() < limite):
        time.sleep(0.05)
    if _executavel(pacote := current.resolve() / "bin" / exe):
        return pacote
    achados = [p for p in (_REPO / "crates" / "target" / "release" / exe, Path.home() / ".hangar" / "bin" / exe)
               if _executavel(p)]
    # O mais novo vence: um build do checkout esquecido depois do `git pull` fala o protocolo antigo,
    # e o Python desligaria o Rust em vez de usar o baixado em dia. Empate fica com o checkout.
    return max(achados, key=lambda p: p.stat().st_mtime, default=None)
