import subprocess

RUN = subprocess.run


def _run(args: list[str]) -> subprocess.CompletedProcess:
    # timeout: tmux travado nao pode prender o event loop / worker do threadpool pra sempre. Estouro ->
    # trata como falha (returncode=1), igual ao tmux recusar; os callers ja checam returncode != 0.
    try:
        return RUN(args, capture_output=True, text=True, timeout=5)
    except (subprocess.TimeoutExpired, OSError) as e:
        # OSError = tmux ausente (FileNotFoundError) / sem permissao; timeout = travado. Trata como
        # falha (returncode=1) em vez de 500 com traceback — os callers ja checam returncode != 0.
        return subprocess.CompletedProcess(args, 1, stdout="", stderr=str(e))


def _pane_target(name: str) -> str:
    # Alvo de SESSAO exato pra comandos pane/window-scoped (send-keys, paste-buffer, capture-pane,
    # list-panes). Sem isto, um nome de sessao NUMERICO (0/1/2 — auto-numerado pelo tmux quando cria
    # sem `-s nome`) colide com INDICE de window: `-t 0` vira "window 0 da sessao anexada", nao
    # "sessao 0" -> resolvia o pane ERRADO e vazava conversa/preview entre sessoes. `=NAME:` forca
    # match exato de sessao (`=`) escopado a sessao (`:`, janela/pane ativo). Nomes nao-numericos
    # ja funcionavam; isto cobre os dois casos.
    return f"={name}:"


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


def new_session(name: str, cwd: str, command: str) -> bool:
    # -e: cores corretas do Claude Code DENTRO do tmux (o claude e spawnado direto, sem shell que
    # leia o rc). COLORTERM=24-bit + CLAUDE_CODE_TMUX_TRUECOLOR curto-circuita o downgrade pra 256
    # (gate pink). O TERM nao-tmux (gate teal) vem do default-terminal no ~/.tmux.conf.
    # Ver docs/tmux-truecolor-setup.md.
    # Retorna False quando o tmux recusa (ex: nome duplicado) -> o caller NAO pode mapear a sessao
    # nova pra um jsonl, senao reusaria a sessao existente de mesmo nome (= "sessao nova foi pra 0").
    cp = _run([
        "tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        "-e", "COLORTERM=truecolor",
        "-e", "CLAUDE_CODE_TMUX_TRUECOLOR=1",
        command,
    ])
    return cp.returncode == 0


def kill_session(name: str) -> None:
    _run(["tmux", "kill-session", "-t", name])


def rename_session(old: str, new: str) -> bool:
    return _run(["tmux", "rename-session", "-t", old, new]).returncode == 0


def send_keys(name: str, keys: str, literal: bool = False) -> None:
    args = ["tmux", "send-keys", "-t", _pane_target(name)]
    if literal:
        args += ["-l", "--", keys]
    else:
        args += [keys]
    _run(args)


def paste_text(name: str, text: str) -> None:
    # Envia texto MULTI-LINHA pro pane via bracketed paste: set-buffer + paste-buffer -p. O `-p` faz a
    # TUI (Ink) receber as quebras como newlines DENTRO do input (não submete cada linha). Buffer
    # nomeado (não suja os paste-buffers do usuário) e `-d` apaga depois. Quem submete e o Enter (caller).
    buf = "cp-prompt"
    _run(["tmux", "set-buffer", "-b", buf, "--", text])
    _run(["tmux", "paste-buffer", "-t", _pane_target(name), "-b", buf, "-p", "-d"])


def capture_pane(name: str, lines: int = 200) -> str:
    cp = _run(["tmux", "capture-pane", "-p", "-t", _pane_target(name), "-S", f"-{lines}"])
    return cp.stdout


def pane_pid(name: str) -> int | None:
    # PID do processo raiz do pane (shell ou o proprio claude). Ponto de partida pra achar qual
    # transcript .jsonl o claude da sessao tem aberto (resolucao autoritativa, nao newest-by-mtime).
    cp = _run(["tmux", "list-panes", "-t", _pane_target(name), "-F", "#{pane_pid}"])
    if cp.returncode != 0:
        return None
    for line in cp.stdout.splitlines():
        if line.strip().isdigit():
            return int(line.strip())
    return None
