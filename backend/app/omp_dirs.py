"""Raiz do agente omp: UMA resposta pra todo o backend.

O omp move config, login, sessões e plugins pra `~/.omp/profiles/<perfil>/agent` quando há
perfil (`--profile` ou `OMP_PROFILE`). Quem lê essa raiz por conta própria diverge de quem
escreve nela; aqui todos perguntam ao mesmo resolvedor.
"""
import logging
import os
from pathlib import Path

_log = logging.getLogger("hangar.omp_dirs")


def agent_dir(home: Path | None = None, env: dict[str, str] | None = None, *, estrito: bool = False) -> Path:
    """Diretório do agente omp em vigor. Perfil inválido não derruba quem só lê: cai na raiz
    sem perfil, com aviso. Quem ESCREVE ali (login, plugins) passa `estrito=True` e recebe
    ValueError — gravar credencial no perfil errado calado é pior que falhar."""
    from app.omp_plugin_sync import InventoryError, resolve_omp_directories

    base = home or Path.home()
    ambiente = dict(os.environ) if env is None else env
    try:
        return resolve_omp_directories(base, ambiente, base).agent_dir
    except InventoryError as erro:
        if estrito:
            raise ValueError(f"perfil do omp inválido: {erro}") from None
        _log.warning("perfil do omp ignorado: %s", erro)
        override = ambiente.get("PI_CODING_AGENT_DIR")
        return Path(override) if override else base / ".omp" / "agent"
