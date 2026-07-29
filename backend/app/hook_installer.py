import json
import shlex
import sys
from pathlib import Path

from app.config import list_config_dirs, _backend_config_base

# Caminho absoluto do script de captura (resolvido uma vez). O command vai literal pro
# settings.json do Claude, entao precisa ser absoluto — o hook roda com cwd arbitrario.
HOOK = str((Path(__file__).parent.parent / "hooks" / "askq_capture.py").resolve())
# `python3` nao existe no Windows (la o binario e python.exe; `python3` e um alias da Microsoft
# Store que ABRE A LOJA em vez de rodar o hook). sys.executable ainda garante o Python do venv do
# backend em vez de torcer pelo que estiver no PATH quando o Claude dispara o hook. Aspas nos dois
# porque qualquer um dos caminhos pode ter espaco (C:\Program Files\..., "Application Support").
_COMMAND = f'"{sys.executable}" "{HOOK}"'
_MATCHER = "AskUserQuestion"


def _script_of(command: str) -> str:
    """Caminho do script dentro de um command nosso ('"py" "X"' -> 'X'). E o ultimo token."""
    return _tokens(command)[-1]


def _tokens(command: str) -> list[str]:
    try:
        return shlex.split(command) or [command]
    except ValueError:  # aspas desbalanceadas a mao
        return command.split() or [command]


def _refers_to(command: object, script: str) -> bool:
    """True se este hook e NOSSO: o command referencia o arquivo `script`, nao importa o
    formato ('python3 X', '"/venv/bin/python3" "X"', com ou sem aspas).

    Comparar a string INTEIRA foi o que duplicou os hooks quando o formato do command
    mudou — a entrada antiga deixou de ser reconhecida e uma nova foi acrescentada.
    Casa pelo caminho: outro checkout do repo aponta pra outro arquivo, logo e outro hook."""
    if not isinstance(command, str):
        return False
    return any(t.strip("\"'") == script for t in _tokens(command))


def _load_settings(settings_path: Path) -> dict | None:
    """Le o settings.json. None = arquivo quebrado/estranho a mao -> nao mexer nele.
    Perder a config do usuario seria pior que nao instalar o hook."""
    data: dict = {}
    if settings_path.exists():
        raw = settings_path.read_text(encoding="utf-8").strip()
        if raw:
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                return None  # JSON invalido editado a mao -> NAO clobbra
        if not isinstance(data, dict):
            return None  # raiz nao-objeto
    hooks = data.get("hooks")
    if hooks is not None and not isinstance(hooks, dict):
        return None  # 'hooks' nao e objeto
    return data


def _sync_hook(data: dict, event: str, command: str, matcher: str | None = None) -> bool:
    """Deixa EXATAMENTE UMA entrada nossa sob hooks[event], com o command atual.
    Formato antigo e substituido no lugar; duplicatas colapsam na primeira ocorrencia
    (o installer roda a cada subida do backend, entao ele cura a bagunca que criou).
    Hooks de terceiros ficam intactos, na mesma ordem. Retorna True se mudou algo."""
    script = _script_of(command)
    hooks = data.setdefault("hooks", {})
    ev_list = hooks.setdefault(event, [])
    if not isinstance(ev_list, list):
        return False  # lista do evento estranha -> nao mexe

    changed = False
    kept = False
    empties: list[int] = []
    for block in ev_list:
        if not isinstance(block, dict):
            continue
        entries = block.get("hooks")
        if not isinstance(entries, list):
            continue
        surviving = []
        for h in entries:
            if isinstance(h, dict) and _refers_to(h.get("command"), script):
                if kept:
                    changed = True  # duplicata nossa -> descarta
                    continue
                kept = True
                if h.get("command") != command:
                    h["command"] = command  # formato antigo -> atual, no lugar
                    changed = True
                h.setdefault("type", "command")
            surviving.append(h)
        if len(surviving) != len(entries):
            if surviving:
                block["hooks"] = surviving
            else:
                empties.append(id(block))  # bloco que era so nosso duplicado
    if empties:
        ev_list = [b for b in ev_list if id(b) not in empties]
        hooks[event] = ev_list
    if not kept:
        block: dict = {"hooks": [{"type": "command", "command": command}]}
        if matcher is not None:
            block["matcher"] = matcher
        ev_list.append(block)
        changed = True
    return changed


def _write(settings_path: Path, data: dict) -> None:
    settings_path.write_text(
        json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )


def _ensure_settings_file(settings_path: Path) -> bool:
    """Garante o bloco PreToolUse/AskUserQuestion num unico settings.json, PRESERVANDO
    todo o resto: outros hooks do usuario (GateGuard, caveman, ponytail, matcher 'Bash'…)
    e qualquer outra chave (model, env, permissions…). Retorna True se gravou (mudou)."""
    data = _load_settings(settings_path)
    if data is None:
        return False
    if not _sync_hook(data, "PreToolUse", _COMMAND, matcher=_MATCHER):
        return False
    _write(settings_path, data)
    return True


STATE_HOOK = str((Path(__file__).parent.parent / "hooks" / "state_hook.py").resolve())
_STATE_COMMAND = f'"{sys.executable}" "{STATE_HOOK}"'  # mesma razao do _COMMAND acima
# SessionStart inclui o caso de abrir ja com --resume/clear (fixa o transcript ativo na hora); os
# demais eventos cobrem o /resume feito DENTRO da sessao (marca no 1o prompt/tool depois dele).
_STATE_EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "SessionStart"]


def _ensure_event_hook(settings_path: Path, event: str, command: str) -> bool:
    """Garante UMA entrada {command} sob settings['hooks'][event], preservando todo o resto.
    Mesma blindagem do _ensure_settings_file: settings.json quebrado/estranho e PULADO (False)."""
    data = _load_settings(settings_path)
    if data is None:
        return False
    if not _sync_hook(data, event, command):
        return False
    _write(settings_path, data)
    return True


def ensure_state_hooks_installed() -> list[str]:
    """Instala (idempotente) o state_hook nos 5 eventos, em cada config dir. Fail-soft por arquivo;
    nunca derruba o startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            changed = False
            for ev in _STATE_EVENTS:
                if _ensure_event_hook(d / "settings.json", ev, _STATE_COMMAND):
                    changed = True
            if changed:
                touched.append(str(d))
        except Exception:
            continue
    return touched


def ensure_askq_hook_installed() -> list[str]:
    """Instala (idempotente) o hook PreToolUse de captura do AskUserQuestion no settings.json
    de cada config dir do Claude. Fail-soft por arquivo: um settings.json problematico nunca
    derruba o backend no startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []  # descoberta de dirs falhou (ex: HOME ausente) -> startup NUNCA quebra
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            if _ensure_settings_file(d / "settings.json"):
                touched.append(str(d))
        except Exception:
            continue  # installer de startup NUNCA propaga excecao
    return touched
