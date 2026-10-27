"""Transporte da triagem compartilhada com o servidor Rust."""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "backend"))
MAX_LEN = 600

def decide(text: str) -> tuple[str, str | None]:
    from app.providers_bridge import local
    return tuple(local("orq-info", "triage", text=text))
