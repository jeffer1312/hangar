"""Fatos de uso sem adquirir novamente a proteção de existência da conta."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import os

try:
    import psutil
except ImportError:
    psutil = None

from app.account_lifecycle import AccountKey, Provider


@dataclass
class UsageFacts:
    complete: bool = False
    sessions: list[str] = field(default_factory=list)
    pids: list[int] = field(default_factory=list)

    def ensure_unused(self) -> None:
        if not self.complete:
            raise RuntimeError("account_usage_unknown")
        if self.sessions or self.pids:
            raise RuntimeError("account_in_use")


_HOSTS = {"node", "bun", "python", "python3", "python3.14", "hangar-cano",
          "bash", "sh", "zsh", "fish", "cmd", "powershell", "pwsh"}
_CLIENTS = {"claude", "codex", "pi", "omp"}


_GONE = (FileNotFoundError, ProcessLookupError) + ((psutil.NoSuchProcess, psutil.ZombieProcess) if psutil else ())
_UNREADABLE = (OSError, ValueError, IndexError) + ((psutil.AccessDenied,) if psutil else ())
_SCAN_ERRORS = (OSError,) + ((psutil.Error,) if psutil else ())


class LinuxProcess:
    """Inspeção estrita em /proc; produção Linux não depende de psutil."""
    def __init__(self, pid: int):
        self.pid = pid
        self.root = Path("/proc") / str(pid)

    def name(self):
        return (self.root / "comm").read_text().strip()

    def cmdline(self):
        return [part.decode("utf-8", "surrogateescape") for part in (self.root / "cmdline").read_bytes().split(b"\0") if part]

    def cwd(self):
        return os.readlink(self.root / "cwd")

    def create_time(self):
        stat = (self.root / "stat").read_text()
        return int(stat[stat.rindex(")") + 1:].split()[19])

    def environ(self):
        return dict(entry.decode("utf-8", "surrogateescape").split("=", 1)
                    for entry in (self.root / "environ").read_bytes().split(b"\0") if b"=" in entry)


def system_process(pid: int):
    return LinuxProcess(pid) if os.name == "posix" and Path("/proc").is_dir() else psutil.Process(pid)


def system_processes():
    if os.name == "posix" and Path("/proc").is_dir():
        return (LinuxProcess(int(path.name)) for path in Path("/proc").iterdir() if path.name.isdigit())
    return (psutil.Process(pid) for pid in psutil.pids())


def inspect_processes(key: AccountKey, *, processes=None, process_factory=None) -> UsageFacts:
    """Ambiente ilegível e identidade trocada nunca provam ausência de uso."""
    facts = UsageFacts(complete=True)
    variable = "CODEX_HOME" if key.provider == Provider.CODEX else "CLAUDE_CONFIG_DIR"
    default = Path.home() / (".codex" if key.provider == Provider.CODEX else ".claude")
    try:
        for process in processes if processes is not None else system_processes():
            try:
                name = process.name().lower().removesuffix(".exe")
                if name not in _CLIENTS | _HOSTS:
                    continue
                before = process.create_time()
                argv = process.cmdline()
                import re
                names = {Path(item).name.lower().removesuffix(".exe") for item in argv}
                names.update(re.findall(r"\b(?:claude|codex|pi|omp)\b", " ".join(argv).lower()))
                pertinent = name in _CLIENTS or bool(names & _CLIENTS) or any(
                    "cano.py" in item or "hangar-cano" in item or "codex.js" in item or "claude-code" in item
                    for item in argv)
                if not pertinent:
                    continue
                env = process.environ()
                if not env:
                    facts.complete = False
                    continue
                home = env.get(variable)
                if key.provider == Provider.CODEX:
                    for index, argument in enumerate(argv):
                        if argument == "--codex-home":
                            home = argv[index + 1]
                        elif argument.startswith("--codex-home="):
                            home = argument.split("=", 1)[1]
                    if home == "":
                        raise ValueError("caminho de conta vazio no lançador")
                if not home:
                    # Ambiente pode conter as duas contas herdadas; só o provider pertinente usa o padrão.
                    expected = {"codex"} if key.provider == Provider.CODEX else {"claude", "pi", "omp"}
                    if not names.intersection(expected) and name not in expected:
                        continue
                    home = str(default)
                path = Path(home).expanduser()
                if not path.is_absolute():
                    path = Path(process.cwd()) / path
                owner = AccountKey.new(key.provider, path)
                after = (process_factory or system_process)(process.pid).create_time()
                if before != after:
                    facts.complete = False
                    continue
                if owner == key:
                    facts.pids.append(process.pid)
            except _GONE:
                continue
            except _UNREADABLE:
                facts.complete = False
    except _SCAN_ERRORS:
        facts.complete = False
    facts.pids = sorted(set(facts.pids))
    return facts


def terminal_sessions() -> set[str]:
    return set(terminal_instances())


def terminal_instances() -> dict[str, str]:
    from app import tmux
    result = tmux._run(["tmux", "list-sessions", "-F",
                       "#{session_name}\t#{pid}:#{session_id}:#{session_created}"])
    if result.returncode == 0 and "\ufffd" not in result.stdout:
        instances = {}
        for line in result.stdout.splitlines():
            name, identity = line.split("\t")
            parts = identity.split(":")
            if (not name or len(parts) != 3 or not parts[0].isdigit()
                    or not parts[1].startswith("$") or not parts[1][1:].isdigit()
                    or not parts[2].isdigit()):
                raise RuntimeError("account_mux_unknown")
            instances[name] = identity
        return instances
    error = result.stderr.strip()
    absent = error.startswith(("no server", "no sessions")) or (
        error.startswith("error connecting to ") and error.endswith("(No such file or directory)"))
    if result.returncode == 1 and absent:
        return {}
    raise RuntimeError("account_mux_unknown")


def inspect_usage(key: AccountKey) -> UsageFacts:
    """Inclui sidecars sem filho iniciado: o nascimento adiado ainda usa a conta."""
    from app.adapters.claude_headless import sessions as headless_sessions
    from app.adapters.codex import sessions as codex_sessions

    facts = inspect_processes(key)
    try:
        import json
        from app import account_lifecycle
        births = account_lifecycle.default_lock_root() / "births"
        if births.exists():
            instances = None
            for path in births.iterdir():
                if path.suffix != ".json":
                    continue
                pending = json.loads(path.read_text(encoding="utf-8"))
                owner = AccountKey.new(pending["provider"], Path(pending["canonical_home"]))
                if not pending.get("name") or "instance" not in pending:
                    raise ValueError("registro de nascimento incompleto")
                if owner != key:
                    continue
                if instances is None:
                    instances = terminal_instances()
                if pending["instance"] is None:
                    # Sem identidade, ausência do nome pode ser apenas um rename.
                    facts.complete = False
                else:
                    facts.sessions.extend(name for name, instance in instances.items()
                                          if instance == pending["instance"])
        directory = (codex_sessions if key.provider == Provider.CODEX else headless_sessions)._dir()
        rows = []
        if directory.exists():
            for path in directory.iterdir():
                if path.suffix == ".json":
                    row = json.loads(path.read_text(encoding="utf-8"))
                    if not isinstance(row, dict) or not row.get("name"):
                        raise ValueError("sidecar de sessão incompleto")
                    rows.append(row)
        terminals = None
        for row in rows:
            home = row.get("codex_home") if key.provider == Provider.CODEX else row.get("config_dir")
            if not home and key.provider == Provider.CODEX:
                from app import codex_contas
                home = str(codex_contas.resolve_account(row.get("codex_account") or "default").home)
            home = home or str(Path.home() / ".claude")
            if AccountKey.new(key.provider, Path(home)) == key:
                if key.provider == Provider.CODEX and not row.get("headless") and not row.get("launching"):
                    if terminals is None:
                        terminals = terminal_sessions()
                    if row["name"] not in terminals:
                        continue
                facts.sessions.append(row["name"])
    except Exception:  # noqa: BLE001 — um leitor parcial não pode autorizar exclusão.
        facts.complete = False
    facts.sessions = sorted(set(facts.sessions))
    return facts
