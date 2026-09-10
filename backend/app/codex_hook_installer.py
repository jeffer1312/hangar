"""Instalador dos hooks de estado e navegador do Hangar no `~/.codex/hooks.json`.

Irmao do hook_installer.py (Claude) e do kimi_hook_installer.py (Kimi): cada harness recebe o
hook do app pelo instalador dele, nunca pelo importador do Codex — que arrastaria junto os outros
hooks so-Claude do app (previa, AskUserQuestion, pareamento) e reescreveria o comando a cada
checkout diferente. O adapter do Codex le o marcador deste hook como segunda fonte de "turno
fechou" (adapters/codex/adapter.py), entao ele precisa existir mesmo com a integracao desligada.

So ACRESCENTA: entrada que ja aponta pro hook (qualquer formato, qualquer checkout) fica
como esta. Reescrever o comando muda o hook, e no Codex hook alterado e hook nao aprovado.
"""
import logging
import os
import sys
from pathlib import Path

from app.hook_installer import NAV_HOOK, STATE_HOOK, _NAV_COMMAND, _STATE_COMMAND, _load_settings, _refers_to, _write

_log = logging.getLogger("hangar.codex_hook_installer")

# Eventos que o Codex tem e que cobrem working/idle (o Claude tem Notification a mais).
_EVENTOS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]


def codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")


def comando_estado(windows: bool | None = None) -> str:
    """O command do hook de estado no formato que o Codex executa NESTE sistema.

    No Windows o Codex roda hooks pelo PowerShell (medido: 7.6.6), onde `"exe" "arg"` e erro de
    sintaxe (UnexpectedToken) e o hook sai com 1 em todo evento; a forma e `& "exe" "arg"`. O
    `; exit 0` faz o papel do `|| exit 0` sem depender do `||`, que o PowerShell 5.1 nao tem.
    """
    windows = os.name == "nt" if windows is None else windows
    if not windows:
        return _STATE_COMMAND
    return f'& "{sys.executable}" "{STATE_HOOK}" ; exit 0'


def comando_navegador(windows: bool | None = None) -> str:
    windows = os.name == "nt" if windows is None else windows
    if not windows:
        return _NAV_COMMAND
    return f'& "{sys.executable}" "{NAV_HOOK}" ; exit 0'


def _tem_hook(grupos: object, script: str = STATE_HOOK) -> bool:
    return isinstance(grupos, list) and any(
        isinstance(g, dict) and isinstance(g.get("hooks"), list)
        and any(isinstance(h, dict) and _refers_to(h.get("command"), script, por_nome=True)
                for h in g["hooks"])
        for g in grupos)


def _reescrever_formato_cmd(hooks: dict, comando: str) -> list[str]:
    tocados: list[str] = []
    for ev, grupos in hooks.items():
        if not isinstance(grupos, list):
            continue
        for g in grupos:
            for h in (g.get("hooks") if isinstance(g, dict) and isinstance(g.get("hooks"), list) else []):
                # So o formato cmd (sem `&`): um `& ...` de outro venv/checkout ja funciona e
                # reescreve-lo invalidaria a aprovacao a cada subida de outra arvore.
                if (isinstance(h, dict) and _refers_to(h.get("command"), STATE_HOOK, por_nome=True)
                        and not str(h.get("command") or "").lstrip().startswith("&")):
                    h["command"] = comando
                    tocados.append(ev)
    return tocados


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
        comando = comando_estado()
        faltando = [ev for ev in _EVENTOS
                    if isinstance(hooks.get(ev, []), list) and not _tem_hook(hooks.get(ev))]
        for ev in faltando:
            hooks.setdefault(ev, []).append({"hooks": [{"type": "command", "command": comando}]})
        evento = "UserPromptSubmit"
        if isinstance(hooks.get(evento, []), list) and not _tem_hook(hooks.get(evento), NAV_HOOK):
            hooks.setdefault(evento, []).append({"hooks": [{
                "type": "command", "command": comando_navegador(),
            }]})
            if evento not in faltando:
                faltando.append(evento)
        # Entrada nossa no formato cmd (`"exe" "arg" || exit 0`) e reescrita SO no Windows: la ela
        # falha em todo evento, e hook que falha nao tem aprovacao a preservar. No POSIX o formato
        # antigo fica como esta (reescrever invalidaria a confianca dada no Codex).
        if os.name == "nt":
            faltando += _reescrever_formato_cmd(hooks, comando)
        if faltando:
            _write(path, data)
        return faltando
    except Exception:
        _log.exception("hooks do Hangar no Codex não instalados")
        return []
