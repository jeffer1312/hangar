"""Raiz da instalação do Hangar e o Python dela. A mesma regra de `crates/hangar-server/src/install_root.rs`.

Com o pacote, a raiz é `~/.hangar/current` SEM resolver o link: o que se grava fora do Hangar (hooks,
atalhos, plugin-dir) continua valendo depois da troca de versão e da poda da antiga. Sem ele, a raiz
do checkout de onde este arquivo roda (máquina não migrada).
"""
import os
import sys
from pathlib import Path


def _current() -> Path:
    return Path.home() / ".hangar" / "current"


def root() -> Path:
    current = _current()
    return current if current.exists() else Path(__file__).resolve().parents[2]


def python_exe(root: Path) -> Path:
    """O Python embutido do pacote; sem ele, o venv do backend (checkout ou modo desenvolvedor)."""
    if os.name == "nt":
        embedded, venv = root / "python" / "python.exe", root / "backend" / ".venv" / "Scripts" / "python.exe"
    else:
        embedded, venv = root / "python" / "bin" / "python3", root / "backend" / ".venv" / "bin" / "python"
    return embedded if embedded.exists() else venv


def interpreter() -> str:
    """Python gravado em comando que roda fora do Hangar: o da raiz com o `current`; sem ele, o que roda
    este backend, como sempre foi."""
    r = root()
    return str(python_exe(r)) if r == _current() else sys.executable
