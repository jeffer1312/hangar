import subprocess

RUN = subprocess.run


def _run(args: list[str]) -> subprocess.CompletedProcess:
    return RUN(args, capture_output=True, text=True)


def list_sessions() -> list[dict]:
    cp = _run(["tmux", "list-sessions", "-F", "#{session_name}\t#{pane_current_path}"])
    if cp.returncode != 0:
        return []
    out = []
    for line in cp.stdout.splitlines():
        if not line.strip():
            continue
        name, _, cwd = line.partition("\t")
        out.append({"name": name, "cwd": cwd})
    return out


def has_session(name: str) -> bool:
    return _run(["tmux", "has-session", "-t", name]).returncode == 0


def new_session(name: str, cwd: str, command: str) -> None:
    _run(["tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50", command])


def kill_session(name: str) -> None:
    _run(["tmux", "kill-session", "-t", name])


def send_keys(name: str, keys: str, literal: bool = False) -> None:
    args = ["tmux", "send-keys", "-t", name]
    if literal:
        args += ["-l", "--", keys]
    else:
        args += [keys]
    _run(args)


def capture_pane(name: str, lines: int = 200) -> str:
    cp = _run(["tmux", "capture-pane", "-p", "-t", name, "-S", f"-{lines}"])
    return cp.stdout
