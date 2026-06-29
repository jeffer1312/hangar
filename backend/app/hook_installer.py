import json
from pathlib import Path

from app.config import list_config_dirs, _backend_config_base

# Caminho absoluto do script de captura (resolvido uma vez). O command vai literal pro
# settings.json do Claude, entao precisa ser absoluto — o hook roda com cwd arbitrario.
HOOK = str((Path(__file__).parent.parent / "hooks" / "askq_capture.py").resolve())
_COMMAND = f"python3 {HOOK}"
_MATCHER = "AskUserQuestion"


def _ensure_settings_file(settings_path: Path) -> bool:
    """Garante o bloco PreToolUse/AskUserQuestion num unico settings.json, PRESERVANDO
    todo o resto: outros hooks do usuario (GateGuard, caveman, ponytail, matcher 'Bash'…)
    e qualquer outra chave (model, env, permissions…). So acrescenta; nunca reescreve o
    que ja existe. Retorna True se gravou (mudou), False se nada mudou.

    Bulletproof: um settings.json quebrado/estranho a mao e PULADO (retorna False), nunca
    sobrescrito — perder a config do usuario seria pior que nao instalar o hook."""
    data: dict = {}
    if settings_path.exists():
        raw = settings_path.read_text(encoding="utf-8").strip()
        if raw:
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                return False  # JSON invalido editado a mao -> NAO clobbra
        if not isinstance(data, dict):
            return False  # raiz nao-objeto -> arquivo estranho, nao mexe

    existing_hooks = data.get("hooks")
    if existing_hooks is not None and not isinstance(existing_hooks, dict):
        return False  # 'hooks' nao e objeto -> nao mexe
    pre = (existing_hooks or {}).get("PreToolUse")
    if pre is not None and not isinstance(pre, list):
        return False  # 'PreToolUse' nao e lista -> nao mexe

    # Idempotencia: se qualquer bloco PreToolUse ja tem o nosso command, nada a fazer.
    for block in pre or []:
        if not isinstance(block, dict):
            continue
        for h in block.get("hooks") or []:
            if isinstance(h, dict) and h.get("command") == _COMMAND:
                return False  # ja instalado

    # Navega/cria so o necessario e acrescenta o nosso bloco, preservando o resto.
    hooks = data.setdefault("hooks", {})
    pre_list = hooks.setdefault("PreToolUse", [])
    pre_list.append({
        "matcher": _MATCHER,
        "hooks": [{"type": "command", "command": _COMMAND}],
    })
    settings_path.write_text(
        json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return True


STATE_HOOK = str((Path(__file__).parent.parent / "hooks" / "state_hook.py").resolve())
_STATE_COMMAND = f"python3 {STATE_HOOK}"
_STATE_EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"]


def _ensure_event_hook(settings_path: Path, event: str, command: str) -> bool:
    """Acrescenta {command} sob settings['hooks'][event], idempotente, preservando todo o resto.
    Mesma blindagem do _ensure_settings_file: settings.json quebrado/estranho e PULADO (False)."""
    data: dict = {}
    if settings_path.exists():
        raw = settings_path.read_text(encoding="utf-8").strip()
        if raw:
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                return False
        if not isinstance(data, dict):
            return False
    existing_hooks = data.get("hooks")
    if existing_hooks is not None and not isinstance(existing_hooks, dict):
        return False
    ev_list = (existing_hooks or {}).get(event)
    if ev_list is not None and not isinstance(ev_list, list):
        return False
    for block in ev_list or []:
        if not isinstance(block, dict):
            continue
        for h in block.get("hooks") or []:
            if isinstance(h, dict) and h.get("command") == command:
                return False  # ja instalado
    hooks = data.setdefault("hooks", {})
    hooks.setdefault(event, []).append({"hooks": [{"type": "command", "command": command}]})
    settings_path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
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
