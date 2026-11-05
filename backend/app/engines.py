import json

from pathlib import Path

def _read_settings(value: str, *, optional: bool = False, cwd: Path | None = None) -> dict:
    try:
        path = Path(value)
        if cwd is not None and not path.is_absolute():
            path = cwd / path
        text = value if value.lstrip().startswith("{") else path.read_text(encoding="utf-8")
        settings = json.loads(text)
    except FileNotFoundError:
        if optional:
            return {}
        raise ValueError("--settings: arquivo não encontrado") from None
    except (OSError, ValueError):
        raise ValueError("settings: arquivo ilegível ou JSON inválido") from None
    if not isinstance(settings, dict) or not isinstance(settings.get("env", {}), dict):
        raise ValueError("settings: esperado um objeto JSON com env como objeto")
    return settings
