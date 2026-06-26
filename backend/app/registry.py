import os
import re
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


def _descendant_pids(root: int) -> list[int]:
    # root + todos os descendentes, via mapa ppid->filhos do /proc/*/stat. O claude pode ser filho do
    # shell do pane (sessao manual) ou o proprio pane (app-criada com `claude` como comando).
    children: dict[int, list[int]] = {}
    try:
        entries = os.listdir("/proc")
    except OSError:
        return [root]
    for entry in entries:
        if not entry.isdigit():
            continue
        try:
            # ppid = 4o campo do stat; usar rsplit(')') pra nao quebrar com espaco/parenteses no comm.
            with open(f"/proc/{entry}/stat", encoding="utf-8", errors="replace") as fh:
                after = fh.read().rsplit(")", 1)[-1].split()
            ppid = int(after[1])
        except (OSError, ValueError, IndexError):
            continue
        children.setdefault(ppid, []).append(int(entry))
    out, stack = [], [root]
    while stack:
        p = stack.pop()
        out.append(p)
        stack.extend(children.get(p, []))
    return out


def _open_jsonl(pid: int, projects_dir: Path) -> Optional[str]:
    # 1o fd aberto apontando pra um *.jsonl dentro do projects_dir (= o transcript ativo do claude).
    fddir = f"/proc/{pid}/fd"
    try:
        fds = os.listdir(fddir)
    except OSError:
        return None
    base = str(projects_dir)
    for fd in fds:
        try:
            target = os.readlink(f"{fddir}/{fd}")
        except OSError:
            continue
        if target.endswith(".jsonl") and target.startswith(base):
            return target
    return None


class SessionRegistry:
    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)

    def resolve_jsonl(self, cwd: str) -> Optional[str]:
        # FALLBACK por cwd: jsonl mais recente do dir do projeto. So usado quando o /proc falha (sem
        # claude rodando, sem permissao). NAO confiavel com varias sessoes no mesmo cwd -> por isso o
        # resolve_via_proc vem primeiro.
        proj = self.projects_dir / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None
        files = sorted(proj.glob("*.jsonl"), key=lambda f: f.stat().st_mtime, reverse=True)
        return str(files[0]) if files else None

    def resolve_via_proc(self, name: str) -> Optional[str]:
        # AUTORITATIVO: o .jsonl que o processo claude DESTA sessao tem aberto. Resolve o bug de sessao
        # nova trazer dados de outro transcript (newest-by-mtime pegava o jsonl errado quando varias
        # sessoes compartilham o cwd). Serve app-criada E manual.
        pid = tmux.pane_pid(name)
        if pid is None:
            return None
        for p in _descendant_pids(pid):
            j = _open_jsonl(p, self.projects_dir)
            if j:
                return j
        return None

    def list(self) -> list[SessionInfo]:
        out = []
        for s in tmux.list_sessions():
            jsonl = self.resolve_via_proc(s["name"]) or self.resolve_jsonl(s["cwd"])
            out.append(SessionInfo(name=s["name"], cwd=s["cwd"], jsonl=jsonl))
        return out

    def create(self, name: str, cwd: str) -> SessionInfo:
        sid = str(uuid.uuid4())
        tmux.new_session(name, cwd, f"claude --session-id {sid}")
        return SessionInfo(name=name, cwd=cwd, jsonl=str(self.projects_dir / sanitize_cwd(cwd) / f"{sid}.jsonl"))

    def kill(self, name: str) -> None:
        tmux.kill_session(name)
