"""Instalador do hook de estado do Hangar no `~/.codex/hooks.json`.

Irmao do hook_installer.py (Claude) e do kimi_hook_installer.py (Kimi): cada harness recebe o
hook do app pelo instalador dele, nunca pelo importador do Codex — que arrastaria junto os outros
hooks so-Claude do app (previa, AskUserQuestion, pareamento) e reescreveria o comando a cada
checkout diferente. O adapter do Codex le o marcador deste hook como segunda fonte de "turno
fechou" (adapters/codex/adapter.py), entao ele precisa existir mesmo com a integracao desligada.

So ACRESCENTA: entrada que ja aponta pro state_hook.py (qualquer formato, qualquer checkout) fica
como esta. Reescrever o comando muda o hook, e no Codex hook alterado e hook nao aprovado.
"""
import logging
import os
from pathlib import Path

from app.hook_installer import STATE_HOOK, _STATE_COMMAND, _load_settings, _refers_to, _write

_log = logging.getLogger("hangar.codex_hook_installer")

# Eventos que o Codex tem e que cobrem working/idle (o Claude tem Notification a mais).
_EVENTOS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]


def codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")


def _tem_hook(grupos: object) -> bool:
    return isinstance(grupos, list) and any(
        isinstance(g, dict) and isinstance(g.get("hooks"), list)
        and any(isinstance(h, dict) and _refers_to(h.get("command"), STATE_HOOK, por_nome=True)
                for h in g["hooks"])
        for g in grupos)


def ensure_codex_state_hook_installed(home: Path | None = None) -> list[str]:
    """Idempotente e fail-soft: devolve os eventos em que gravou (so pra log), [] senao."""
    try:
        base = home or codex_home()
        if not base.is_dir():
            return []
        path = base / "hooks.json"
        data = _load_settings(path)
        if data is None:
            return []
        hooks = data.setdefault("hooks", {})
        # Evento com valor que não é lista é arquivo editado à mão: não se mexe (mesma regra do
        # _sync_hook do Claude), em vez de zerar o que estava lá.
        faltando = [ev for ev in _EVENTOS
                    if isinstance(hooks.get(ev, []), list) and not _tem_hook(hooks.get(ev))]
        for ev in faltando:
            hooks.setdefault(ev, []).append({"hooks": [{"type": "command", "command": _STATE_COMMAND}]})
        if faltando:
            _write(path, data)
        return faltando
    except Exception:
        _log.exception("hook de estado do Codex nao instalado")
        return []
