"""Transporte da raiz do agente omp resolvida pelo Rust."""
import logging
import os
from pathlib import Path

_log = logging.getLogger("hangar.omp_dirs")


def agent_dir(home: Path | None = None, env: dict[str, str] | None = None, *, estrito: bool = False) -> Path:
    """Consulta a raiz em vigor, preservando a política de leitura e escrita do chamador."""
    from app.providers_bridge import local

    base = home or Path.home()
    ambiente = dict(os.environ) if env is None else env
    result = local("provider-info", op="omp-directory", home=str(base), env=ambiente, strict=estrito)
    if result.get("warning"):
        _log.warning("%s", result["warning"])
    return Path(result["agent_dir"])
