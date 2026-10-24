"""Tela temporária "Migração para Rust" (sai na parte 7): o que só o Python sabe do processo.

Modo, motivo de o Python atender sozinho, versões e consumo dos processos conhecidos. Com o Rust
na porta, ele pede isto em `/internal/migration/status` e soma a parte dele; sozinho, o Python
responde `/api/migration/status` com o mesmo bloco.
"""
from __future__ import annotations

import json
import os
import sys
import threading
import time
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request

from app import diag, guest_users
from app.auth import require_auth
from app.config import settings
from app.mensagens import erro
from app.guest_header import guest_of

# Código que o Supervisor registra quando desiste do Rust; `None` com o Rust na porta.
_reason: str | None = None
# Porta em que este uvicorn atende: a interna, atrás do Rust, ou a pública.
_listen_port: int | None = None
# pid -> (instante, segundos de CPU) da leitura anterior: a CPU é a média entre duas visitas.
_samples: dict[int, tuple[float, float]] = {}
_samples_lock = threading.Lock()
_SAMPLE_MAX_AGE = 120.0
_canos: tuple[float, list[int]] = (0.0, [])
_CANOS_TTL = 5.0


def set_reason(code: str | None) -> None:
    global _reason
    _reason = code


def set_listen_port(port: int | None) -> None:
    global _listen_port
    _listen_port = port


def reason() -> str | None:
    if _reason:
        return _reason
    # Estes dois nem chegam ao Supervisor; com --reload o processo que atende é o filho do uvicorn.
    if settings.reload:
        return "reload"
    if not settings.rust_server:
        return "desligado"
    return None


def _mode() -> str:
    from app import runtime_coordinator
    current = runtime_coordinator.current()
    return current.mode if current is not None else runtime_coordinator._initial_mode


_branch: str | None = None
_branch_failed_at = 0.0
_BRANCH_RETRY_S = 60.0


def _checkout_branch() -> str | None:
    # Lida uma vez por processo, como a versão: a branch só muda no Atualizar, que reinicia o backend.
    # Falha não fica guardada: a próxima visita tenta de novo.
    global _branch, _branch_failed_at
    if _branch is not None or time.monotonic() - _branch_failed_at < _BRANCH_RETRY_S:
        return _branch
    if m := diag.install_manifest():
        _branch = m["channel"]
        return _branch
    from app import atualizar, git_ops
    try:
        result = git_ops._run(str(atualizar.REPO), "rev-parse", "--abbrev-ref", "HEAD", timeout=5)
    except git_ops.GitError as e:
        _branch_failed_at = time.monotonic()
        diag.registrar("migration_status.branch", "aviso", **diag.erro_campos(e))
        return None
    if result.returncode != 0:
        _branch_failed_at = time.monotonic()
        diag.registrar("migration_status.branch", "aviso", codigo=str(result.returncode))
        return None
    _branch = result.stdout.strip() or None
    return _branch


def _status_memory(text: str) -> tuple[int, int]:
    """(VmRSS, VmSwap) em bytes do `/proc/<pid>/status`: com zram o kernel troca uma pela outra."""
    found = {}
    for line in text.splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "VmSwap"):
            found[key] = int(value.split()[0]) * 1024
    return found.get("VmRSS", 0), found.get("VmSwap", 0)


def _read_proc(pid: int) -> tuple[int, int, float] | None:
    """(RSS e swap em bytes, segundos de CPU) de um pid conhecido; None se ele saiu ou não dá para ler."""
    if sys.platform.startswith("linux"):
        try:
            with open(f"/proc/{pid}/status", encoding="ascii", errors="replace") as f:
                rss, swap = _status_memory(f.read())
            with open(f"/proc/{pid}/stat", encoding="ascii", errors="replace") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            # utime e stime: campos 14 e 15 do stat, contados a partir do estado (campo 3).
            cpu = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
        except (OSError, ValueError, IndexError):
            return None
        return rss, swap, cpu
    import psutil
    try:
        proc = psutil.Process(pid)
        times = proc.cpu_times()
        return proc.memory_info().rss, 0, times.user + times.system
    except (psutil.Error, OSError):
        return None


def _usage(pid: int | None) -> dict | None:
    if not pid:
        return None
    read = _read_proc(pid)
    if read is None:
        return None
    rss, swap, cpu = read
    now = time.monotonic()
    with _samples_lock:
        before = _samples.get(pid)
        _samples[pid] = (now, cpu)
        for old in [p for p, (at, _) in _samples.items() if now - at > _SAMPLE_MAX_AGE]:
            del _samples[old]
    percent = None
    if before is not None and now - before[0] > 0.2 and now - before[0] < _SAMPLE_MAX_AGE:
        percent = round(max(cpu - before[1], 0.0) / (now - before[0]) * 100, 1)
    return {"pid": pid, "rss_bytes": rss, "swap_bytes": swap, "cpu_seconds": round(cpu, 2), "cpu_percent": percent}


def _is_cano(pid: int) -> bool:
    # O pid vem do sidecar e pode ter sido reaproveitado por outro processo.
    if sys.platform.startswith("linux"):
        try:
            args = [a.decode(errors="replace") for a in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")]
        except OSError:
            return False
    else:
        import psutil
        try:
            args = psutil.Process(pid).cmdline()
        except (psutil.Error, OSError):
            return False
    # O binário (`hangar-cano`, `.exe` no Windows) ou a reserva em Python (`cano.py`).
    return any(Path(a).name in ("hangar-cano", "hangar-cano.exe", "cano.py") for a in args[:3])


def _cano_pids() -> list[int]:
    """Pids dos canos das sessões sem terminal, pelos sidecars; nunca varre a máquina."""
    global _canos
    at, pids = _canos
    if time.monotonic() - at < _CANOS_TTL:
        return pids
    from app.adapters.claude_headless import sessions as claude_sessions
    from app.adapters.codex import sessions as codex_sessions
    found = set()
    for folder in (claude_sessions._dir(), codex_sessions._dir()):
        try:
            files = list(folder.glob("*.json"))
        except OSError:
            continue
        for path in files:
            try:
                pid = ((json.loads(path.read_text(encoding="utf-8")) or {}).get("cano") or {}).get("pid")
            except (OSError, ValueError, AttributeError):
                continue
            if isinstance(pid, int) and pid > 0 and _is_cano(pid):
                found.add(pid)
    _canos = (time.monotonic(), sorted(found))
    return _canos[1]


def _canos_usage() -> dict:
    total = {"count": 0, "rss_bytes": 0, "swap_bytes": 0, "cpu_percent": 0.0}
    for pid in _cano_pids():
        usage = _usage(pid)
        if usage is None:
            continue
        total["count"] += 1
        total["rss_bytes"] += usage["rss_bytes"]
        total["swap_bytes"] += usage["swap_bytes"]
        # Um cano sem a segunda leitura deixa a soma incompleta: a tela mostra "medindo", não um número menor.
        if usage["cpu_percent"] is None or total["cpu_percent"] is None:
            total["cpu_percent"] = None
        else:
            total["cpu_percent"] = round(total["cpu_percent"] + usage["cpu_percent"], 1)
    return total


def facts() -> dict:
    from app import rust_server
    proc = rust_server.child()
    binary = rust_server.current_binary()
    try:
        binary_info = {"path": str(binary), "mtime": int(binary.stat().st_mtime)} if binary else None
    except OSError:
        binary_info = {"path": str(binary), "mtime": None}
    rust_alive = proc is not None and proc.poll() is None
    return {
        "mode": _mode(),
        "reason": reason(),
        "protocol": rust_server.RUST_SERVER_PROTOCOL,
        "port": _listen_port,
        "version": diag.VERSAO_EM_EXECUCAO,
        "branch": _checkout_branch(),
        "update_branch": settings.update_branch or None,
        "binary": binary_info,
        "processes": {
            "python": _usage(os.getpid()),
            "rust": _usage(proc.pid) if rust_alive else None,
            "cano": _canos_usage(),
        },
    }


def _require_owner(request: Request) -> None:
    if guest_of(request) is not None or guest_users.current.get() is not None:
        raise HTTPException(403, detail=erro("migration_status_owner", "Só o dono vê o estado da migração."))


router = APIRouter(dependencies=[Depends(require_auth), Depends(_require_owner)])


@router.get("/api/migration/status")
def read_status() -> dict:
    # Chegou ao Python: é ele que atende a porta pública (o Rust intercepta esta rota).
    return {"served_by": "python", "python": facts(), "rust": None, "areas": None, "private": None}
