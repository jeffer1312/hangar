import json
from pathlib import Path
from typing import Optional

from watchfiles import awatch

_SUBDIR = ".claude-pocket-state"


class HookState:
    """Estado da LISTA por sessao, vindo dos hooks do Claude (state_hook.py grava marcadores).
    Mapa em memoria session_id -> (state, ts). get_state() devolve None se nao ha marcador
    (o caller cai no fallback de raspar o pane)."""

    def __init__(self) -> None:
        self._map: dict[str, tuple[str, float]] = {}

    def get_state(self, session_id: Optional[str]) -> Optional[tuple[str, float]]:
        if not session_id:
            return None
        return self._map.get(session_id)

    def _apply(self, path: Path) -> None:
        # Le UM marcador pro mapa. Falha-soft: marcador parcial/corrompido e ignorado.
        try:
            o = json.loads(path.read_text(encoding="utf-8"))
            state, ts = o["state"], float(o["ts"])
        except Exception:
            return
        self._map[path.stem] = (state, ts)

    def load_existing(self, dirs: list[Path]) -> None:
        # Semeia o mapa com os marcadores ja presentes (no startup do backend).
        for base in dirs:
            sd = base / _SUBDIR
            if not sd.is_dir():
                continue
            for f in sd.glob("*.json"):
                self._apply(f)

    async def watch(self, dirs: list[Path]) -> None:
        # Loop longo: observa cada <config>/.claude-pocket-state e aplica cada mudanca.
        watched = []
        for base in dirs:
            sd = base / _SUBDIR
            sd.mkdir(parents=True, exist_ok=True)  # garante existir pro awatch nao falhar
            watched.append(str(sd))
        async for changes in awatch(*watched):
            for _change, p in changes:
                path = Path(p)
                if path.suffix == ".json":
                    self._apply(path)


# Singleton de modulo (igual ao padrao do registry/installer).
hook_state = HookState()
