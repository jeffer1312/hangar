import json
import re
import tomllib
from pathlib import Path
from typing import Optional

from app.config import settings
from app.models import Runner

# nome -> peso pra escolher o melhor palpite de "dev" (so um vence).
_DEV_RANK = {"dev": 5, "start": 4, "serve": 3, "watch": 2, "run": 1}
_MAKE_TARGET = re.compile(r"^([a-zA-Z0-9_-]+):", re.MULTILINE)


def _pm(cwd: Path) -> str:
    # package manager pelo lockfile; default npm.
    if (cwd / "pnpm-lock.yaml").is_file():
        return "pnpm"
    if (cwd / "bun.lockb").is_file():
        return "bun"
    if (cwd / "yarn.lock").is_file():
        return "yarn"
    return "npm"


def _scan_package_json(cwd: Path) -> list[Runner]:
    try:
        data = json.loads((cwd / "package.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    scripts = data.get("scripts") if isinstance(data, dict) else None
    if not isinstance(scripts, dict):
        return []
    pm = _pm(cwd)
    out = []
    for name in scripts:
        if isinstance(name, str) and name:
            out.append(Runner(label=name, command=f"{pm} run {name}", source="npm"))
    return out


def _scan_makefile(cwd: Path) -> list[Runner]:
    try:
        text = (cwd / "Makefile").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    seen, out = set(), []
    for m in _MAKE_TARGET.finditer(text):
        t = m.group(1)
        if t not in seen:
            seen.add(t)
            out.append(Runner(label=t, command=f"make {t}", source="make"))
    return out


def _scan_stack(cwd: Path) -> list[Runner]:
    out = []
    if (cwd / "Cargo.toml").is_file():
        out.append(Runner(label="cargo run", command="cargo run", source="stack"))
    pyproj = cwd / "pyproject.toml"
    if pyproj.is_file():
        try:
            d = tomllib.loads(pyproj.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            d = {}
        project = d.get("project") if isinstance(d, dict) else None
        scripts = project.get("scripts") if isinstance(project, dict) else None
        if isinstance(scripts, dict):
            for name in scripts:
                if isinstance(name, str) and name:
                    out.append(Runner(label=name, command=f"uv run {name}", source="stack"))
    return out


def detect_runners(cwd: str) -> list[Runner]:
    """Comandos de execucao detectados no projeto. Tolerante a arquivos ausentes/malformados."""
    base = Path(cwd)
    runners = _scan_package_json(base) + _scan_makefile(base) + _scan_stack(base)
    best_i, best_score = -1, 0
    for i, r in enumerate(runners):
        score = _DEV_RANK.get(r.label.lower(), 0)
        if score > best_score:
            best_i, best_score = i, score
    if best_i >= 0:
        runners[best_i].is_dev_guess = True
    return runners


def _prefs_path() -> Path:
    return Path(settings.projects_dir).parent / ".claude-pocket-runner.json"


def _load_prefs() -> dict:
    try:
        data = json.loads(_prefs_path().read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def remembered(cwd: str) -> Optional[str]:
    v = _load_prefs().get(cwd)
    return v if isinstance(v, str) else None


def remember(cwd: str, command: str) -> None:
    p = _prefs_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    d = _load_prefs()
    d[cwd] = command
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(d), encoding="utf-8")
    tmp.replace(p)  # escrita atomica
