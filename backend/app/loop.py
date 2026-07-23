"""Loop runner (harness bloco A): objetivo -> executa -> verifica -> re-prompta -> para.
Sidecar JSON por sessao FONTE em ".claude-pocket-loop", keyed pelo NOME (sobrevive ao /clear),
mesmo padrao do chain/pqueue. Um loop por sessao; loop novo sobrescreve o anterior.
Spec: docs/superpowers/specs/2026-07-22-loop-runner-design.md"""
import json
import time
from pathlib import Path

from app.config import settings
from app.pqueue import _sanitize

ACTIVE = {"running", "paused_awaiting", "done_claimed"}


def _loop_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-loop"
    d.mkdir(parents=True, exist_ok=True)
    return d


def new_loop(goal: str, check_cmd: str | None, max_iters: int, require_branch: bool) -> dict:
    return {
        "goal": goal, "check_cmd": check_cmd, "max_iters": max_iters,
        "require_branch": require_branch, "status": "running", "iter": 0,
        "goal_entry_id": None, "goal_delivered_ts": None, "history": [],
        "started_ts": time.time(), "ended_ts": None, "ended_reason": None,
    }


class LoopLink:
    """Sidecar de UMA sessao (<nome>.json). get/set/update/clear; escrita atomica."""

    def __init__(self, name: str):
        self.path = _loop_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            return json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None

    def set(self, data: dict) -> None:
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        tmp.replace(self.path)

    def update(self, **fields) -> dict | None:
        cur = self.get()
        if cur is None:
            return None
        cur.update(fields)
        self.set(cur)
        return cur

    def clear(self) -> None:
        try:
            self.path.unlink()
        except OSError:
            pass
