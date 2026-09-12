"""Logs pertencem ao Hangar da máquina, não à conta de um provedor."""
import os
from pathlib import Path


def base() -> Path:
    if os.name == "nt":
        root = Path(os.environ.get("LOCALAPPDATA") or (Path.home() / "AppData" / "Local"))
        return root / "hangar" / "logs"
    return Path.home() / ".hangar" / "logs"
