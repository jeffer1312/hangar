"""Quantos agentes e shells em segundo plano ainda rodam numa sessão Claude, pelo transcript.

O `Stop` do Claude marca "parou" a cada fim de turno, também quando a sessão só está esperando o
fim de um trabalho em segundo plano que vai acordá-la de novo: o push de "terminou" não pode sair
nesse intervalo. Mesma leitura do painel de atividade (`packages/core/src/activity.ts`), sem os
colegas de equipe: só o lançamento e a `<task-notification>` que o fecha.
"""
import re
from pathlib import Path

from app.transcript import parse_line

_SHELL_LAUNCH = re.compile(r"Command running in background with ID:\s*([A-Za-z0-9_-]+)")
# A cauda basta: o que está rodando foi lançado na parte recente da conversa.
_TAIL_BYTES = 4 * 1024 * 1024


def _tail_lines(path: Path) -> list[str]:
    with path.open("rb") as fh:
        size = fh.seek(0, 2)
        fh.seek(max(0, size - _TAIL_BYTES))
        data = fh.read()
    lines = data.decode("utf-8", errors="replace").splitlines()
    return lines[1:] if size > _TAIL_BYTES else lines  # a primeira pode estar cortada no meio


def count(jsonl: str | Path) -> int:
    launched: set[str] = set()
    closed: set[str] = set()
    shell_calls: set[str] = set()
    for line in _tail_lines(Path(jsonl)):
        for e in parse_line(line):
            tid = e.tool_use_id or ""
            if e.kind == "tool_use" and e.tool_name == "Bash" and (e.tool_input or {}).get("run_in_background"):
                shell_calls.add(tid)
            elif e.kind == "tool_result":
                if tid.startswith("task:"):
                    closed.add(tid[len("task:"):])
                elif e.bg_agent_id and not e.bg_agent_id.startswith("teammate:"):
                    launched.add(e.bg_agent_id)
                elif tid in shell_calls and (m := _SHELL_LAUNCH.search(e.result or "")):
                    launched.add(m.group(1))
    return len(launched - closed)
