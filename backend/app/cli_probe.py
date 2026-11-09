from __future__ import annotations

import logging
import os
import subprocess

_log = logging.getLogger("hangar")

# Seam para testes — monkeypatch para forjar o PATH do login (mesma técnica de procinfo.py).
# Quando não-None, _obter_path() devolve este valor (se for callable, chama).
_path_login = None  # type: ignore[assignment]

# Cache do PATH do login (uma vez por processo)
_path_cache: str | None = None

def _obter_path() -> str:
    global _path_cache
    # seam de teste tem precedência
    if _path_login is not None:  # type: ignore[truthy-function]
        try:
            if callable(_path_login):  # type: ignore[arg-type]
                val = _path_login()  # type: ignore[operator]
                return str(val) if val is not None else ""
            return str(_path_login)
        except Exception:
            return ""
    if _path_cache is not None:
        return _path_cache
    # Windows não tem /bin/sh nem SHELL; usa PATH direto
    if os.name == "nt":
        val = os.environ.get("PATH", "")
        _path_cache = val
        return val
    shell = os.environ.get("SHELL", "/bin/sh")
    try:
        r = subprocess.run(
            [shell, "-l", "-c", "printenv PATH"],
            capture_output=True,
            text=True,
            # Sem `encoding`, o `text=True` decodifica pelo locale — cp1252 no Windows. O PATH
            # carrega o nome do perfil do usuario, que e onde acento aparece com mais frequencia.
            encoding="utf-8",
            errors="replace",
            timeout=5,
        )
        if r.returncode == 0:
            val = r.stdout.strip()
            if val:
                _path_cache = val
                return val
    except Exception:
        # Shell de login quebrado (rc com erro) some daqui e vira "provider indisponível" sem
        # pista nenhuma pra quem for depurar.
        _log.debug("cli_probe: PATH do shell de login falhou; usando o do processo", exc_info=True)
    val = os.environ.get("PATH", "")
    _path_cache = val
    return val
