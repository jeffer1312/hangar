import hashlib
import json
import os
import re
import shlex
import subprocess
import tomllib
from pathlib import Path
from typing import Optional

from app.config import settings
from app.models import Runner, RunInfo
from app.tmux import _scope_prefix, _pane_target

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


RUN = subprocess.run
SOCK = "cppkt-run"  # socket tmux dedicado -> nao aparece na lista de sessoes do app


def _sock(args: list[str]) -> subprocess.CompletedProcess:
    try:
        return RUN(["tmux", "-L", SOCK, *args], capture_output=True, text=True, timeout=5)
    except (subprocess.TimeoutExpired, OSError) as e:
        return subprocess.CompletedProcess(args, 1, "", str(e))


def _slug(cwd: str) -> str:
    base = re.sub(r"[^a-zA-Z0-9_-]", "-", Path(cwd).name) or "proj"
    return f"{base}-{hashlib.sha1(cwd.encode()).hexdigest()[:6]}"


def start_run(cwd: str, command: str) -> RunInfo:
    """Mata o run anterior do projeto (1 por projeto), inicia o novo numa sessao tmux
    no socket dedicado, grava como lembrado, devolve o status."""
    name = _slug(cwd)
    _sock(["kill-session", "-t", name])
    # remain-on-exit ANTES do spawn (global no socket, que so tem runs): processo que morre
    # logo apos o play mantem pane+log e vira "failed" com exit code, em vez de sumir sem
    # rastro. Setar depois do new-session deixava exatamente essa janela aberta.
    _sock(["start-server"])
    _sock(["set-option", "-g", "remain-on-exit", "on"])
    shell = os.environ.get("SHELL", "/bin/sh")
    # login shell (-lc) herda env/PATH do projeto; exec faz o comando virar dono do pane.
    spawn = _scope_prefix() + [
        "tmux", "-L", SOCK, "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        f"exec {shell} -lc {shlex.quote(command)}",
    ]
    try:
        RUN(spawn, capture_output=True, text=True, timeout=5)
    except (subprocess.TimeoutExpired, OSError):
        pass
    remember(cwd, command)
    return run_status(cwd) or RunInfo(command=command)


def stop_run(cwd: str) -> None:
    _sock(["kill-session", "-t", _slug(cwd)])


def all_runs() -> dict[str, RunInfo]:
    """Todos os runs do socket dedicado, por nome de sessao — INCLUSIVE os de pane morto
    (remain-on-exit), que carregam exited/exit_status. Uma chamada tmux so, pro /api/projects
    nao pagar um subprocesso por projeto."""
    cp = _sock(["list-panes", "-a", "-F",
                "#{session_name}\t#{session_created}\t#{pane_dead}\t#{pane_dead_status}"])
    out: dict[str, RunInfo] = {}
    if cp.returncode != 0:
        return out
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 4:
            continue
        sn, created, dead, status = parts
        out[sn] = RunInfo(command="", since=int(created) if created.isdigit() else None,
                          exited=dead == "1",
                          # vazio quando vivo ou morto por sinal — ai nao ha exit code.
                          exit_status=int(status) if status.lstrip("-").isdigit() else None)
    return out


def run_status(cwd: str) -> Optional[RunInfo]:
    info = all_runs().get(_slug(cwd))
    if info:
        info.command = remembered(cwd) or ""
    return info


def run_pane(cwd: str) -> str:
    cp = _sock(["capture-pane", "-p", "-t", _pane_target(_slug(cwd)), "-S", "-200"])
    return cp.stdout
