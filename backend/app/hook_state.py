import json
import logging
from pathlib import Path
from typing import Callable, Optional

from watchfiles import awatch

_log = logging.getLogger("claude_pocket.hook_state")

_SUBDIR = ".claude-pocket-state"


class HookState:
    """Estado da LISTA por sessao, vindo dos hooks do Claude (state_hook.py grava marcadores).
    Mapa em memoria session_id -> (state, ts). get_state() devolve None se nao ha marcador
    (o caller cai no fallback de raspar o pane)."""

    def __init__(self) -> None:
        self._map: dict[str, tuple[str, float]] = {}
        # Dirs registrados (load_existing/watch) — demote_awaiting precisa achar o sidecar.
        self._dirs: list[Path] = []
        # Disparado na transicao -> awaiting_input (so no watch ao vivo, nao no load_existing do boot).
        # Wiring em api.py manda o push. Recebe o session_id (uuid); best-effort, nunca levanta.
        self.on_awaiting: Optional[Callable[[str], None]] = None
        # Disparado em QUALQUER mudanca de estado ao vivo: (session_id, state). Wiring em api.py:
        # drain server-side (entrega a fila sem depender de conexao SSE) + confirmacao de entrega.
        self.on_transition: Optional[Callable[[str, str], None]] = None

    def get_state(self, session_id: Optional[str]) -> Optional[tuple[str, float]]:
        if not session_id:
            return None
        return self._map.get(session_id)

    def _apply(self, path: Path, notify: bool = False) -> None:
        # Le UM marcador pro mapa. Falha-soft: marcador parcial/corrompido e ignorado.
        try:
            o = json.loads(path.read_text(encoding="utf-8"))
            state, ts = o["state"], float(o["ts"])
        except Exception:
            return
        prev = self._map.get(path.stem)
        self._map[path.stem] = (state, ts)
        # Transicao -> awaiting_input (so ao vivo). prev None = marcador novo aparecendo ja awaiting
        # (sessao acabou de pedir input) tambem conta. awaiting->awaiting (so o ts mudou) nao re-dispara.
        if notify and state == "awaiting_input" and (prev is None or prev[0] != "awaiting_input"):
            cb = self.on_awaiting
            if cb:
                try:
                    cb(path.stem)
                except Exception:
                    pass
        # Mudanca de estado (qualquer) ao vivo -> on_transition (drain server-side / confirmacao).
        if notify and (prev is None or prev[0] != state):
            cb2 = self.on_transition
            if cb2:
                try:
                    cb2(path.stem, state)
                except Exception:
                    pass

    def demote_awaiting(self, session_id: str) -> None:
        """Rebaixa um marcador awaiting_input pra idle — mapa E sidecar. Chamado quando o pane
        raspado contradiz o marcador: o state_hook mapeia QUALQUER Notification pra awaiting, e a
        Notification de "idle 60s" do Claude Code chega DEPOIS do Stop sem nenhum evento posterior
        que corrija -> sem isto a sessao parada fica "aguardando" pra sempre na lista. Persistir no
        sidecar importa: o boot re-semeia o mapa de la (load_existing). ts original preservado."""
        cur = self._map.get(session_id)
        if not cur or cur[0] != "awaiting_input":
            return
        self._map[session_id] = ("idle", cur[1])
        for base in self._dirs:
            f = base / _SUBDIR / f"{session_id}.json"
            if f.is_file():
                try:
                    tmp = f.with_suffix(".json.tmp")
                    tmp.write_text(json.dumps({"state": "idle", "ts": cur[1]}), encoding="utf-8")
                    tmp.replace(f)  # atomico, mesmo padrao do state_hook
                except OSError:
                    # Mapa ja esta idle mas o sidecar ficou awaiting: proximo BOOT re-semeia o
                    # fantasma (load_existing le do disco). Logar e o rastro pra entender o retorno.
                    _log.warning("demote_awaiting: falha ao regravar sidecar %s", f, exc_info=True)

    def load_existing(self, dirs: list[Path]) -> None:
        # Semeia o mapa com os marcadores ja presentes (no startup do backend).
        self._dirs = list(dict.fromkeys([*self._dirs, *dirs]))
        for base in dirs:
            sd = base / _SUBDIR
            if not sd.is_dir():
                continue
            for f in sd.glob("*.json"):
                self._apply(f)

    async def watch(self, dirs: list[Path]) -> None:
        # Loop longo: observa cada <config>/.claude-pocket-state e aplica cada mudanca.
        self._dirs = list(dict.fromkeys([*self._dirs, *dirs]))
        watched = []
        for base in dirs:
            sd = base / _SUBDIR
            sd.mkdir(parents=True, exist_ok=True)  # garante existir pro awatch nao falhar
            watched.append(str(sd))
        async for changes in awatch(*watched):
            for _change, p in changes:
                path = Path(p)
                if path.suffix == ".json":
                    self._apply(path, notify=True)


# Singleton de modulo (igual ao padrao do registry/installer).
hook_state = HookState()
