"""Sonda de CLIs instalados — quais binários de agente existem no PATH do shell de login.

Binário por provider é constante; o PATH é o do shell de login do usuário (quem executa o
CLI é o pane via ``$SHELL -c``, tmux.py:391), obtido via ``$SHELL -l -c 'printenv PATH'``
(com ``printenv``, nunca ``echo $PATH`` — fish separa por espaço).

Enumera TODOS os candidatos no PATH na mão (shutil.which só devolve o primeiro) e sonda
cada um com ``[caminho, "--version"]`` e timeout de 2s. Classificação padrão Orca:
timeout ou exit com qualquer código = instalado; FileNotFound/PermissionError/OSError = tenta
próximo; nenhum bom = não instalado. Resultado cacheado por 60s (time.monotonic).
"""

from __future__ import annotations

import errno
import logging
import os
import subprocess
import threading
import time

_log = logging.getLogger("hangar")

_BIN = {"claude": "claude", "codex": "codex", "pi": "pi", "kimi": "kimi"}

# Seam para testes — monkeypatch para forjar o PATH do login (mesma técnica de procinfo.py).
# Quando não-None, _obter_path() devolve este valor (se for callable, chama).
_path_login = None  # type: ignore[assignment]

# Cache do PATH do login (uma vez por processo)
_path_cache: str | None = None

# Cache do resultado (60s)
_cache: dict[str, dict] | None = None
_cache_ts: float = 0
_TTL = 60


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


# Serializa a sondagem: cada chamada roda em `to_thread`, e duas com o cache vencido ao mesmo
# tempo disparavam a varredura inteira (4 binários × PATH, subprocess com timeout de 2s) em
# paralelo. Com o lock, a segunda espera e sai pelo cache que a primeira acabou de encher.
_sonda_lock = threading.Lock()


def sondar_providers() -> dict[str, dict]:
    global _cache, _cache_ts
    now = time.monotonic()
    if _cache is not None and (now - _cache_ts) < _TTL:
        return _cache
    with _sonda_lock:
        now = time.monotonic()
        if _cache is not None and (now - _cache_ts) < _TTL:
            return _cache
        return _sondar_sem_cache()


def _sondar_sem_cache() -> dict[str, dict]:
    global _cache, _cache_ts

    path_str = _obter_path()
    dirs = path_str.split(os.pathsep) if path_str else []

    result: dict[str, dict] = {}
    for provider, bin_name in _BIN.items():
        disponivel = False
        motivo: str | None = "nao_encontrado"
        viu_sem_permissao = False

        for d in dirs:
            if not d:
                continue
            # candidatos: no Windows tenta cada PATHEXT
            if os.name == "nt":
                pathext = os.environ.get("PATHEXT", "")
                exts = [e.strip() for e in pathext.split(";") if e.strip()] if pathext else []
                candidatos = [os.path.join(d, bin_name + ext) for ext in exts]
                candidatos.append(os.path.join(d, bin_name))
            else:
                candidatos = [os.path.join(d, bin_name)]
            for caminho in candidatos:
                try:
                    r = subprocess.run([caminho, "--version"], timeout=2, capture_output=True)
                    # exit com qualquer código = instalado
                    disponivel = True
                    motivo = None
                    break
                except subprocess.TimeoutExpired:
                    disponivel = True
                    motivo = None
                    break
                except PermissionError:
                    viu_sem_permissao = True
                    continue
                except FileNotFoundError:
                    continue
                except OSError as e:
                    # ENOEXEC (8) = arquivo não executável (ex: texto sem shebang + sem exec)
                    # EACCES (13) = sem permissão
                    if e.errno in (errno.EACCES, errno.ENOEXEC):
                        viu_sem_permissao = True
                    continue
                except Exception:
                    continue
            if disponivel:
                break

        if not disponivel:
            motivo = "sem_permissao" if viu_sem_permissao else "nao_encontrado"
        else:
            motivo = None
        result[provider] = {"disponivel": disponivel, "motivo": motivo}

    _cache = result
    _cache_ts = time.monotonic()
    return result
