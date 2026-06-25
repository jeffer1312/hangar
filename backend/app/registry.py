import re
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


class SessionRegistry:
    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)

    def resolve_jsonl(self, cwd: str) -> Optional[str]:
        proj = self.projects_dir / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None
        files = sorted(proj.glob("*.jsonl"), key=lambda f: f.stat().st_mtime, reverse=True)
        return str(files[0]) if files else None

    def list(self) -> list[SessionInfo]:
        out = []
        for s in tmux.list_sessions():
            out.append(SessionInfo(name=s["name"], cwd=s["cwd"], jsonl=self.resolve_jsonl(s["cwd"])))
        return out

    def create(self, name: str, cwd: str) -> SessionInfo:
        sid = str(uuid.uuid4())
        tmux.new_session(name, cwd, f"claude --session-id {sid}")
        return SessionInfo(name=name, cwd=cwd, jsonl=str(self.projects_dir / sanitize_cwd(cwd) / f"{sid}.jsonl"))

    def kill(self, name: str) -> None:
        tmux.kill_session(name)
