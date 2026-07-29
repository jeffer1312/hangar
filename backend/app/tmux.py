import os
import shutil
import logging
import subprocess

RUN = subprocess.run


_SCOPE = ["systemd-run", "--user", "--scope", "--collect", "-q", "--"]

# Cache do probe abaixo: None = ainda nao testado. Por processo — se o systemd voltar ao normal,
# um restart do backend re-testa. Nao vale re-testar a cada sessao: o gerenciador raramente muda
# de estado e o probe custa um fork.
_scope_usavel: bool | None = None


def _scope_probe() -> bool:
    # `systemd-run` pode ESTAR instalado e ainda assim falhar: o gerenciador systemd do usuario
    # recusa criar scope transiente ("Failed to start transient scope unit: Unit run-pN.scope not
    # found") e o comando sai 1 sem rodar nada. Aconteceu nesta maquina, 5/5 tentativas, com o
    # binario e o gerenciador na mesma versao — logo NAO da pra inferir do `which`.
    # Sem este probe, toda criacao de sessao morre com "falha ao criar sessao no tmux": o app fica
    # sem abrir sessao por causa de um detalhe de cgroup que e OPCIONAL.
    global _scope_usavel
    if _scope_usavel is None:
        # ponytail: check-then-act sem lock. Duas POST /api/sessions concorrentes no frio (backend
        # recem-subido) podem rodar o probe as duas — mas o probe é idempotente (mesmo comando,
        # mesmo resultado booleano), então o pior caso é um fork() de sobra, nunca um valor
        # inconsistente. Lock só entra se esse fork extra virar custo medido.
        _scope_usavel = _run([*_SCOPE, "true"]).returncode == 0
        if not _scope_usavel:
            # Falha aparece, nao some: sem o scope, uma sessao que TENHA de iniciar o servidor tmux
            # nasce no cgroup do backend, e ai um `systemctl restart` do backend derruba as sessoes.
            # Com o servidor tmux ja de pe (caso normal) o pane herda o cgroup DELE e nada muda.
            _log.warning("systemd-run --user --scope indisponivel; criando sessoes sem scope "
                         "proprio. Se o servidor tmux precisar ser iniciado por aqui, um restart "
                         "do backend pode derrubar as sessoes.")
    return _scope_usavel


def _scope_prefix() -> list[str]:
    # Spawn the tmux SERVER in its OWN transient systemd scope so it does NOT inherit the
    # backend service's cgroup. Without this, `systemctl restart claude-pocket-backend`
    # SIGTERMs the whole control-group -> kills the tmux server and every session (incl. the
    # one driving this app). ponytail: gated on systemd-run + a user runtime dir; on non-systemd
    # hosts returns [] and spawns plainly, where the cgroup teardown problem doesn't exist.
    if os.name == "posix" and os.environ.get("XDG_RUNTIME_DIR") and shutil.which("systemd-run"):
        return _SCOPE if _scope_probe() else []
    return []


def _wayland_display() -> str | None:
    # Paste de IMAGEM no Claude Code depende do wl-paste, que precisa de WAYLAND_DISPLAY. O backend
    # roda como servico systemd (env de boot, sem a var) -> detecta o socket do compositor no
    # runtime dir (ex: wayland-1 no Hyprland; o fallback wayland-0 do wl-paste NAO acha esse).
    if os.environ.get("WAYLAND_DISPLAY"):
        return os.environ["WAYLAND_DISPLAY"]
    run_dir = os.environ.get("XDG_RUNTIME_DIR")
    if not run_dir:
        return None
    try:
        socks = sorted(f for f in os.listdir(run_dir)
                       if f.startswith("wayland-") and not f.endswith(".lock"))
    except OSError:
        return None
    return socks[0] if socks else None


_log = logging.getLogger("claude_pocket.tmux")


def _run(args: list[str]) -> subprocess.CompletedProcess:
    # timeout: tmux travado nao pode prender o event loop / worker do threadpool pra sempre. Estouro ->
    # trata como falha (returncode=1), igual ao tmux recusar; os callers ja checam returncode != 0.
    try:
        return RUN(args, capture_output=True, text=True, encoding="utf-8", errors="replace",
                   timeout=5)
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


def list_panes_active() -> list[dict]:
    # UMA chamada traz nome + pane_pid + cwd + pane_id da pane ATIVA de TODAS as sessoes. Substitui o
    # list_sessions() + um pane_pid() por sessao (S+1 forks -> 1) no caminho da listagem. pane_id
    # (ex: "%9") identifica o bilhete da extensao Pi (Task 3), que e por-pane, nao por-sessao.
    cp = _run(["tmux", "list-panes", "-a", "-F",
               "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"])
    if cp.returncode != 0:
        return []
    out: dict[str, dict] = {}
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 5:
            continue
        name, active, pid, cwd, pane_id = parts
        if active != "1" or name in out:
            continue
        out[name] = {"name": name, "pid": int(pid) if pid.isdigit() else None, "cwd": cwd,
                     "pane_id": pane_id}
    return list(out.values())


def has_session(name: str) -> bool:
    # `=NAME`: match EXATO, mesma pegadinha do _pane_target acima. Sem o `=`, o target-session do tmux
    # cai em exact -> fnmatch -> PREFIX match: com "pocket-2" viva, `has_session("pocket")` respondia
    # VIVO pra uma sessao que NUNCA existiu. Como o app fabrica nomes que colidem por prefixo
    # (`<base>`, `<base>-2`, `<base>-3`...), a sessao morta herdava o "vivo" da irma -> o /input
    # confirmava "entregue" digitando num pane inexistente e o state.py nunca marcava `dead`.
    # Sem `:` aqui (ao contrario do _pane_target): has-session so resolve SESSAO, nao pane/window.
    return _run(["tmux", "has-session", "-t", f"={name}"]).returncode == 0


def new_session(name: str, cwd: str, command: str, config_dir: str | None = None) -> bool:
    # -e: cores corretas do Claude Code DENTRO do tmux (o claude e spawnado via `exec`, virando o
    # processo do pane sem shell intermediario). COLORTERM=24-bit + CLAUDE_CODE_TMUX_TRUECOLOR curto-circuita o downgrade pra 256
    # (gate pink). O TERM nao-tmux (gate teal) vem do default-terminal no ~/.tmux.conf.
    # Ver docs/tmux-truecolor-setup.md.
    # Retorna False quando o tmux recusa (ex: nome duplicado) -> o caller NAO pode mapear a sessao
    # nova pra um jsonl, senao reusaria a sessao existente de mesmo nome (= "sessao nova foi pra 0").
    cfg = config_dir or os.environ.get("CLAUDE_CONFIG_DIR")
    args = _scope_prefix() + [
        "tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        "-e", "COLORTERM=truecolor",
        "-e", "CLAUDE_CODE_TMUX_TRUECOLOR=1",
    ]
    wl = _wayland_display()
    if wl:
        # sem isto o wl-paste dentro do pane nao conecta -> paste de imagem no Claude Code morre.
        args += ["-e", f"WAYLAND_DISPLAY={wl}"]
    if cfg:
        # sessao app-criada usa o MESMO config dir que o backend (ou o escolhido), em vez de cair
        # no ~/.claude default (deslogado -> tela de boas-vindas).
        args += ["-e", f"CLAUDE_CONFIG_DIR={cfg}"]
    # `exec`: o tmux SEMPRE roda o comando via `$SHELL -c` (fish aqui). Sem exec, o fish fica como
    # dono do tty/grupo de foreground e o `send-keys` (input do app) NAO chega no claude -> ele
    # renderiza mas nunca le o teclado. Com exec o fish vira o claude (dono do tty) -> input chega.
    # SO no POSIX: o psmux (Windows) roda o comando direto no ConPTY, sem shell no meio, e o
    # `exec` viraria um argumento que nenhum shell do Windows conhece — o pane nasce e morre na
    # hora, com o new-session ainda devolvendo 0, ou seja, o app reportaria sessao criada.
    args.append(f"exec {command}" if os.name == "posix" else command)
    return _run(args).returncode == 0


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
    cp = _run(["tmux", "paste-buffer", "-t", _pane_target(name), "-b", buf, "-p", "-d"])
    if cp.returncode == 0:
        return
    _paste_linha_a_linha(name, text)


def _paste_linha_a_linha(name: str, text: str) -> None:
    """Plano B do multi-linha: uma chamada por linha, com C-j entre elas.

    Multiplexador sem `paste-buffer` (medido no psmux 3.3.7, o tmux nativo de Windows). Duas coisas
    aprendidas no teste, as duas contra-intuitivas:
      1. o bracketed paste na mao (`send-keys -l` com ESC[200~/201~) NAO resolve — o psmux aceita a
         sequencia mas engole tudo depois do primeiro \n do argumento;
      2. trocar o separador por \r tambem nao: as linhas chegam GRUDADAS numa so.
    O que funciona e nunca ter \n dentro do argumento: C-j (0x0A) vai como TECLA NOMEADA, que a TUI
    insere como quebra sem submeter.

    Custa 2N-1 chamadas pra N linhas, contra 2 do paste-buffer — por isso e plano B e nao padrao.
    NAO roda no Linux: la o paste-buffer devolve 0 e o caller sai antes de chegar aqui.
    """
    alvo = _pane_target(name)
    for i, linha in enumerate(text.split("\n")):
        if i:
            _run(["tmux", "send-keys", "-t", alvo, "C-j"])
        if linha:
            _run(["tmux", "send-keys", "-t", alvo, "-l", "--", linha])


def pane_scrollback(name: str) -> int:
    """Linhas de scrollback que o tmux REALMENTE tem pra este pane (0 = nenhuma).

    Um TUI de tela cheia (Claude Code) roda na TELA ALTERNADA, e nela o tmux nao acumula historico:
    `alternate_on=1` -> `history_size=0`, e `capture-pane -S -N` nunca devolve mais que a tela
    visivel por mais linhas que se peca. A TUI do Codex sobe com `--no-alt-screen`, entao ali o
    scrollback existe de verdade. Quem desenha a UI precisa saber a diferenca pra nao oferecer
    "carregar mais historico" onde nao ha historico nenhum.
    """
    cp = _run(["tmux", "display", "-p", "-t", _pane_target(name),
               "#{?alternate_on,0,#{history_size}}"])
    if cp.returncode != 0:
        # 0 aqui significaria "nao ha historico" — e tmux QUEBRADO (ausente, sem permissao, travado
        # no timeout, sessao zumbi) daria a MESMA resposta, escondendo a falha atras de uma UI que
        # so some com o botao. Ainda devolve 0 (a UI nao pode explodir), mas nao em silencio.
        _log.warning("tmux display falhou pra %r: %s", name, (cp.stderr or "").strip()[:200])
        return 0
    out = cp.stdout.strip()
    return int(out) if out.isdigit() else 0


def capture_pane(name: str, lines: int = 200) -> str:
    cp = _run(["tmux", "capture-pane", "-p", "-t", _pane_target(name), "-S", f"-{lines}"])
    if cp.returncode != 0:
        # stdout vazio numa falha e indistinguivel de um pane genuinamente vazio -> o /pane devolvia
        # 200 com texto "" e ninguem ficava sabendo que o tmux tinha falhado. Devolve "" (o caller
        # degrada), mas registra.
        _log.warning("tmux capture-pane falhou pra %r: %s", name, (cp.stderr or "").strip()[:200])
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
