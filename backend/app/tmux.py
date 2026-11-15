import os
import logging
import subprocess
import threading
import time

from app import diag

RUN = subprocess.run


_log = logging.getLogger("hangar.tmux")


# Codigo de retorno que NENHUM multiplexador usa, reservado pra "nao consegui nem rodar o comando"
# (travou e estourou o timeout, binario ausente, sem permissao) — em oposicao a "rodou e recusou",
# que e o rc 1 do tmux dizendo, por exemplo, "no server running". Os dois continuam sendo `!= 0`,
# entao todo caller de hoje se comporta igual; quem precisa separar os dois casos e so quem LISTA
# sessoes, porque ali confundi-los significa dizer "voce nao tem sessao nenhuma" quando a verdade e
# "nao sei". Ver MuxIndisponivel.
RC_INDISPONIVEL = 240
_RUN_LOCK = threading.Lock()
_running = 0
_DIAG_COMMANDS = frozenset({
    "list-sessions", "list-panes", "has-session", "show-options", "display-message",
    "display", "new-session", "new-window", "set-option", "kill-session", "rename-session",
    "send-keys", "capture-pane", "set-buffer", "show-buffer", "delete-buffer", "load-buffer",
    "paste-buffer", "resize-window",
})
_ABSENCE_OK = frozenset({"list-sessions", "list-panes", "has-session", "show-options"})


class MuxIndisponivel(RuntimeError):
    """O multiplexador nao respondeu — a lista de sessoes e DESCONHECIDA, nao vazia.

    Nasceu de um caso relatado em 25/08/2026 num Windows (psmux): as sessoes sumiam todas do app
    do nada, com o servidor ainda verde, e so voltavam num "Reconectar". A causa e que
    `list-panes -a` estourando os 5s de timeout virava rc=1, que as funcoes de listagem traduziam
    pra "zero sessoes" — o refresher da lista via um snapshot vazio PERFEITAMENTE VALIDO, a
    assinatura mudava e o SSE mandava `[]` pro app. Nada falhava em lugar nenhum: a conexao seguia
    viva, o ping seguia chegando, e a tela ficava vazia ate o proximo poll bom.

    Levantar aqui e o que reconecta esse caminho com a maquinaria que ja existia pra isso: o
    refresher segura o ultimo snapshot bom e emite `list_error`, e o app mostra a lista de antes
    com aviso em vez de apagar tudo.
    """


def _run(args: list[str], input: bytes | None = None) -> subprocess.CompletedProcess:
    global _running
    started = time.monotonic()
    with _RUN_LOCK:
        _running += 1
        simultaneous = _running
    failure = None
    cp = None
    try:
        # RUN é a costura dos testes; bytes de stdin exigem decode explícito na saída.
        if input is not None:
            cp = RUN(args, capture_output=True, timeout=5, input=input)
            cp = subprocess.CompletedProcess(
                args, cp.returncode,
                cp.stdout.decode("utf-8", "replace"), cp.stderr.decode("utf-8", "replace"))
        else:
            cp = RUN(args, capture_output=True, text=True, encoding="utf-8", errors="replace",
                     timeout=5)
        return cp
    except (subprocess.TimeoutExpired, OSError) as exc:
        failure = exc
        # So o tipo e o subcomando: o argv (que o `str(exc)` repete) pode carregar o texto de um atalho.
        kind = type(exc).__name__
        _log.warning("tmux nao respondeu (%s): %s", args[1] if args[0] == "tmux" and len(args) > 1 else "outro", kind)
        cp = subprocess.CompletedProcess(args, RC_INDISPONIVEL, stdout="", stderr=kind)
        return cp
    finally:
        elapsed = int((time.monotonic() - started) * 1000)
        with _RUN_LOCK:
            _running -= 1
        # Nem argv nem stderr: ambos podem conter a mensagem ou credenciais do processo criado.
        command = args[1] if len(args) > 1 and args[0] == "tmux" and args[1] in _DIAG_COMMANDS else "outro"
        failed = cp is not None and cp.returncode != 0
        stderr = cp.stderr if cp is not None and isinstance(cp.stderr, str) else ""
        absent = stderr.startswith(("no server", "no sessions", "can't find session", "no such session",
                                    "invalid option: @cp_hidden")) or (
            stderr.startswith("error connecting to ") and stderr.rstrip().endswith("(No such file or directory)"))
        expected = failure is None and cp is not None and cp.returncode == 1 and command in _ABSENCE_OK and absent
        if failure is not None or (failed and not expected) or elapsed >= 1000:
            diag.registrar("mux.comando", "erro" if failed and not expected else "aviso",
                           comando=command, ms=elapsed, limite_ms=5000,
                           simultaneos=simultaneous, retorno=cp.returncode if cp is not None else None,
                           erro_tipo=type(failure).__name__ if failure is not None else ("retorno" if failed else ""),
                           errno=getattr(failure, "errno", None), winerror=getattr(failure, "winerror", None),
                           **diag.recursos())


def _exige_resposta(cp: subprocess.CompletedProcess) -> None:
    # Guarda das funcoes que LISTAM sessoes: so elas separam "rodou e nao ha sessao" de "nao rodou".
    if cp.returncode == RC_INDISPONIVEL:
        raise MuxIndisponivel(cp.stderr.strip() or "sem resposta do multiplexador")


def _cwd_vivo(cwd: str) -> str:
    # Pasta apagada com o processo dentro: o Linux devolve o caminho com " (deleted)" no fim.
    if cwd.endswith(" (deleted)") and not os.path.exists(cwd):
        return cwd[: -len(" (deleted)")]
    return cwd


def list_panes_all() -> dict[str, list[dict]]:
    # `#{@cp_hidden}` é opção de usuário por SESSÃO; sessão sem a opção devolve "" -> hidden=False.
    # O parse aceita 5 campos OU MAIS: multiplexador que devolve o campo vazio ou o literal cru
    # (psmux) segue listando; o que RECUSA o comando -> rc != 0 -> {}.
    cp = _run(["tmux", "list-panes", "-a", "-F",
               "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"
               "\t#{@cp_hidden}\t#{CP_PROVIDER}\t#{session_created}"])
    _exige_resposta(cp)
    if cp.returncode != 0:
        return {}
    out: dict[str, list[dict]] = {}
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 5:
            continue
        name, active, pid, cwd, pane_id = parts[:5]
        hidden = parts[5] if len(parts) > 5 else ""
        provider = parts[6] if len(parts) > 6 else ""
        birth = parts[7] if len(parts) > 7 else ""
        out.setdefault(name, []).append({
            "name": name, "pid": int(pid) if pid.isdigit() else None, "cwd": _cwd_vivo(cwd),
            "pane_id": pane_id, "active": active == "1", "hidden": hidden == "1",
            "provider": provider if provider in {"claude", "codex", "pi", "omp", "kimi"} else None,
            "session_created": int(birth) if birth.isdigit() and int(birth) > 0 else None,
        })
    return out


def has_session(name: str) -> bool:
    # `=NAME`: match EXATO. Sem o `=`, o target-session do tmux cai em exact -> fnmatch -> PREFIX
    # match: com "pocket-2" viva, `has_session("pocket")` respondia VIVO pra uma sessão que NUNCA
    # existiu (o app fabrica nomes que colidem por prefixo: `<base>`, `<base>-2`...).
    return _run(["tmux", "has-session", "-t", f"={name}"]).returncode == 0
