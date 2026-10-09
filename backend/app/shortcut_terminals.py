"""Terminais dos atalhos "shell": cada execucao ganha uma sessao tmux escondida propria.

O tmux e a unica fonte de verdade: dono, id e rotulo moram em opcoes de usuario da sessao
(`@cp_shortcut_*`), entao a lista sobrevive a restart do backend sem arquivo de estado. A marca
`@cp_hidden` e a mesma do shell do painel (`tmux.new_hidden_shell`): sem ela a sessao viraria card.

`remain-on-exit` mantem o pane depois que o comando sai, com a saida na tela (o codigo de saida vai
na aba) — e o que deixa a pessoa conferir o que o atalho fez e fechar quando quiser.

Dono vazio = terminal "No Hangar": uma copia por chave de atalho no servidor, de nenhuma sessao.
No Windows (psmux) o comando roda por um `.cmd` que grava o codigo de saida num arquivo.
"""
import logging
import os
import re
import secrets
import shutil
import signal
import subprocess
import threading
import time
from pathlib import Path

from app import procinfo, terminal_prompt, tmux

_log = logging.getLogger(__name__)

PREFIX = "shortcut-"
_OWNER, _ID, _LABEL = "@cp_shortcut_owner", "@cp_shortcut_id", "@cp_shortcut_label"
# Nanossegundos da criacao: o `session_created` do tmux e por segundo, e dois cliques no mesmo
# segundo trocariam de ordem na barra de abas.
_SEQ = "@cp_shortcut_seq"
_KEY, _ORIGIN, _ASK = "@cp_shortcut_key", "@cp_shortcut_origin", "@cp_shortcut_ask"
# Guardados pra "Rodar de novo" repetir o que rodou. Lidos so no restart, nunca pelo `-F`.
_CWD, _CMD = "@cp_shortcut_cwd", "@cp_shortcut_cmd"
_IS_WINDOWS = os.name == "nt"
# `.cmd` e lido pelo cmd na codepage OEM (regra de encoding do Windows).
_OEM = "oem" if os.name == "nt" else "latin-1"
# Dois cliques ao mesmo tempo (duas sessoes, dois aparelhos) nao podem abrir duas copias.
_HANGAR_LOCK = threading.Lock()
_ID_RE = re.compile(r"^[0-9a-f]{6}$")
_LABEL_MAX = 80


def _slug(owner: str) -> str:
    # O nome vira alvo do tmux: `.` e `:` separam janela/pane la, e o resto fica curto e legivel.
    return re.sub(r"[^A-Za-z0-9_-]", "_", owner)[:40] or "s"


def _clean_label(label: str) -> str:
    # Uma linha so: o rotulo volta pelo `-F` separado por tab e acaba num botao de aba.
    text = " ".join(label.split())
    return text[:_LABEL_MAX]


def _windows_dir() -> Path:
    base = Path(os.environ.get("LOCALAPPDATA") or Path.home()) / "Hangar" / "shortcuts"
    base.mkdir(parents=True, exist_ok=True)
    return base


def _windows_status(ident: str) -> tuple[bool, int | None]:
    # O psmux devolve pane_dead_status 0 pra qualquer saida: o codigo vem do arquivo que o .cmd grava.
    try:
        text = (_windows_dir() / f"{ident}.exit").read_text(errors="replace").strip()
    except OSError:
        return True, None
    return False, int(text) if text.lstrip("-").isdigit() else None


def _write_cmd(path: Path, text: str) -> None:
    # `newline=""`: no Windows o modo texto trocaria `\n` por `\r\n` e o `\r\n` ja escrito viraria `\r\r\n`.
    with open(path, "w", encoding=_OEM, errors="replace", newline="") as f:
        f.write(text)


_FILE_SUFFIXES = (".cmd", "-cmd.cmd", "-cmd.ps1", ".exit")
_FILE_RE = re.compile(r"^([0-9a-f]{6})(?:-cmd\.cmd|-cmd\.ps1|\.cmd|\.exit)$")
# Arquivo mais novo que isto pode ser de um start que ainda nao criou a sessao.
_ORPHAN_GRACE = 60.0
# Variaveis que o cmd expande sem estarem no ambiente.
_DYNAMIC_VARS = frozenset({"CD", "DATE", "TIME", "RANDOM", "ERRORLEVEL", "CMDCMDLINE", "CMDEXTVERSION",
                           "HIGHESTNUMANODENUMBER"})
_VAR_RE = re.compile(r"%([A-Za-z_][A-Za-z0-9_()]*)%")


def _batch_escape(command: str) -> str:
    """Numa linha de `cmd /c` o `%` e literal; num arquivo de lote ele expande (`%20`, `%1`, `%i`).
    Todo `%` vira `%%`, menos o `%NOME%` de variavel que existe (ambiente ou dinamica): essa segue
    expandindo nos dois, como a pessoa escreveria. `%VAR:~0,3%` e `%VAR:a=b%` ficam literais."""
    known = _DYNAMIC_VARS | {k.upper() for k in os.environ}
    out, i = [], 0
    while i < len(command):
        if command[i] != "%":
            out.append(command[i])
            i += 1
            continue
        m = _VAR_RE.match(command, i)
        if m and m.group(1).upper() in known:
            out.append(m.group(0))
            i = m.end()
        else:
            out.append("%%")
            i += 1
    return "".join(out)


def _batch_unescape(text: str) -> str:
    # Inverso do escape: `%%` volta a `%`; `%NOME%` inteiro nao e tocado (o par `%%` vem sempre antes).
    return re.sub(r"%%|%[A-Za-z_][A-Za-z0-9_()]*%", lambda m: "%" if m.group(0) == "%%" else m.group(0), text)


def _forget_files(ident: str) -> None:
    for suffix in _FILE_SUFFIXES:
        (_windows_dir() / f"{ident}{suffix}").unlink(missing_ok=True)


def _sweep_orphans(live: set[str]) -> None:
    # O `.cmd` guarda o texto do comando (pode ter credencial): nao pode sobrar sem terminal dono.
    now = time.time()
    for path in _windows_dir().iterdir():
        m = _FILE_RE.match(path.name)
        if not m or m.group(1) in live:
            continue
        try:
            if now - path.stat().st_mtime > _ORPHAN_GRACE:
                path.unlink()
        except OSError:
            pass


def _windows_command(ident: str, command: str, powershell: bool = False, shell: str | None = None) -> str:
    # O comando fica num .cmd proprio (nada de aspas do psmux no caminho dele) e roda num `cmd /c`
    # filho: `exit 3` sem /b mataria um `call`. O de fora grava o codigo com o redirecionamento na
    # frente (`echo 3>x` redirecionaria o handle 3) e segura o pane com `pause` em laco, porque tecla
    # na aba nao pode encerrar o terminal e o remain-on-exit na criacao derruba a sessao no psmux.
    d = _windows_dir()
    inner, outer, exit_file = d / f"{ident}-cmd.cmd", d / f"{ident}.cmd", d / f"{ident}.exit"
    exit_file.unlink(missing_ok=True)
    if powershell:
        script = d / f"{ident}-cmd.ps1"
        body = command.replace("\r\n", "\n").replace("\n", "\r\n")
        with open(script, "w", encoding="utf-8-sig", newline="") as f:
            f.write(body + "\r\n$__hangar_ok = $?\r\nif (-not $__hangar_ok) { if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; exit 1 }\r\n")
        executable = shell or shutil.which("powershell.exe") or str(Path(os.environ.get("SystemRoot", "C:\\Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe")
        launch = f'@"{executable}" -NoProfile -ExecutionPolicy Bypass -File "{script}"'
    else:
        body = _batch_escape(command).replace("\r\n", "\n").replace("\n", "\r\n")
        _write_cmd(inner, f"@echo off\r\n{body}\r\n")
        launch = f'@cmd /d /c "{inner}"'
    _write_cmd(outer, f'{launch}\r\n@>"{exit_file}" echo %ERRORLEVEL%\r\n:h\r\n@pause >nul\r\n@goto h\r\n')
    return f'cmd /d /c "{outer}"'


def _set_options(target: str, options) -> bool:
    # Uma chamada por opcao: texto livre terminado em `;` viraria separador de comando no tmux, e no
    # psmux opcao na mesma chamada do new-session derruba a sessao.
    # Mesmo sozinho, o `;` no fim do valor e tirado como separador; `\;` o mantem literal no tmux.
    for opt, value in options:
        cp = tmux._run(["tmux", "set-option", "-t", f"={target}:", opt, _keep_semicolon(value)])
        if cp.returncode != 0:
            _log.warning("shortcut: set-option %s falhou em %r (rc=%s): %s", opt, target, cp.returncode,
                         (cp.stderr or "").strip()[:200])
            return False
    return True


def _keep_semicolon(value: str) -> str:
    return value[:-1] + "\\;" if value.endswith(";") and not _IS_WINDOWS else value


def _abort(target: str) -> None:
    # Sessao que nasceu sem as marcas: sai inteira (com o grupo de processos, se ainda vive).
    try:
        row = next((r for r in _rows() if r["tmux"] == target), None)
    except MuxUnavailable:
        row = None
    if row is not None:
        if not _close_row(row):
            _log.warning("shortcut: sessao %r sem marcas nao saiu ao abortar", target)
    elif tmux.kill_session(target):
        if _IS_WINDOWS:
            # Sem `_ID` a lista nao o ve: o id vem do nome.
            _forget_files(target.rsplit("-", 1)[-1])
    else:
        _log.warning("shortcut: sessao %r sem marcas nao saiu ao abortar", target)


def start(owner: str, cwd: str, command: str, label: str, env: dict[str, str],
          key: str = "", origin: str = "", ask: bool = True, powershell: bool = False, shell: str | None = None) -> dict | None:
    """Cria a sessao escondida rodando o comando. None = o multiplexador recusou.
    Dono vazio = terminal No Hangar: nenhuma sessao o lista nem o fecha."""
    ident = secrets.token_hex(3)
    target = f"{PREFIX}{_slug(owner or 'hangar')}-{ident}"
    label = _clean_label(label) or _clean_label(command)
    args = [*tmux._scope_prefix(), "tmux", "new-session", "-d", "-s", target, "-c", cwd]
    for k, value in env.items():
        args += ["-e", f"{k}={value}"]
    # Opcao ausente volta vazia no `-F`: valor vazio nem e gravado. No Windows o comando nao vai pra
    # opcao (argv do psmux quebra em `\n` e `;`): o "Rodar de novo" le o `.cmd`.
    fixed = tuple(kv for kv in (("@cp_hidden", "1"), (_OWNER, owner), (_ID, ident), (_SEQ, str(time.time_ns()))) if kv[1])
    free = tuple(kv for kv in ((_LABEL, label), (_KEY, " ".join(key.split())), (_ORIGIN, _clean_label(origin)),
                               (_ASK, "1" if ask else "0"), (_CWD, cwd), *(() if _IS_WINDOWS else ((_CMD, command),)))
                 if kv[1])
    if _IS_WINDOWS:
        cp = tmux._run([*args, _windows_command(ident, command, powershell, shell)])
        if cp.returncode != 0 and not tmux.has_session(target):
            _log.warning("shortcut: psmux recusou criar %r: %s", target, (cp.stderr or "").strip()[:200])
            _forget_files(ident)
            return None
        # Escondida ANTES de tudo, senao a lista de sessoes ve um card no meio do caminho.
        if not _set_options(target, fixed) or not _set_options(target, (*free, ("status", "off"))):
            _abort(target)
            return None
        return {"id": ident, "label": label, "tmux": target}
    shell = shell or os.environ.get("SHELL") or "/bin/sh"
    args += ["--", shell, "-c", command]
    # Invocacao unica pras opcoes fixas: o tmux executa a lista inteira antes de tratar a saida do
    # filho, entao um comando que morre na hora ainda encontra o remain-on-exit ligado, e a lista
    # de sessoes nunca ve a sessao sem a marca de escondida.
    for opt, value in fixed:
        args += [";", "set-option", "-t", f"={target}:", opt, value]
    args += [";", "set-option", "-w", "-t", f"={target}:", "remain-on-exit", "on"]
    # Linha "Pane is dead" vazia: com texto o tmux a escreve no rodape e rola a tela uma linha,
    # levando a primeira linha da saida pro historico. A aba ja diz "saiu com N".
    args += [";", "set-option", "-w", "-t", f"={target}:", "remain-on-exit-format", ""]
    args += [";", "set-option", "-t", f"={target}:", "status", "off"]
    cp = tmux._run(args)
    if cp.returncode != 0 and not tmux.has_session(target):
        _log.warning("shortcut: tmux recusou criar %r: %s", target, (cp.stderr or "").strip()[:200])
        return None
    # Sem a chave o No Hangar perderia a copia unica: falha aparece em vez de terminal orfao.
    if not _set_options(target, free):
        _abort(target)
        return None
    return {"id": ident, "label": label, "tmux": target}


class MuxUnavailable(tmux.MuxIndisponivel):
    """O multiplexador nao respondeu: nao da pra saber se ja existe uma copia do atalho.
    Herda o erro do tmux pra rota sem tratamento proprio sair no 503 do handler global."""


def _rows() -> list[dict]:
    rows = _read_rows()
    if rows is None:
        raise MuxUnavailable("sem resposta do multiplexador")
    return rows


def _read_rows() -> list[dict] | None:
    """None = o multiplexador nao respondeu (diferente de "sem servidor", que e lista vazia)."""
    cp = tmux._run(["tmux", "list-sessions", "-F",
                    f"#{{session_name}}\t#{{{_OWNER}}}\t#{{{_ID}}}\t#{{session_created}}"
                    f"\t#{{pane_dead}}\t#{{pane_dead_status}}\t#{{pane_pid}}\t#{{{_SEQ}}}"
                    f"\t#{{{_KEY}}}\t#{{{_ORIGIN}}}\t#{{{_ASK}}}\t#{{{_LABEL}}}"])
    if cp.returncode == tmux.RC_INDISPONIVEL:
        return None
    if cp.returncode != 0:
        return []
    out = []
    for line in cp.stdout.splitlines():
        parts = line.split("\t", 11)
        if len(parts) != 12 or not parts[0].startswith(PREFIX) or not _ID_RE.match(parts[2]):
            continue
        name, owner, ident, created, dead, status, pid, seq, key, origin, ask, label = parts
        alive, code = dead != "1", (int(status) if dead == "1" and status.lstrip("-").isdigit() else None)
        if _IS_WINDOWS:
            alive, code = _windows_status(ident)
        out.append({"tmux": name, "owner": owner, "id": ident,
                    "created": int(created) if created.isdigit() else 0, "alive": alive, "exit_code": code,
                    "pid": int(pid) if pid.isdigit() else None, "label": label,
                    "seq": int(seq) if seq.isdigit() else 0, "key": key, "origin": origin, "ask": ask != "0"})
    return out


def _option(target: str, opt: str) -> str:
    return tmux._run(["tmux", "show-options", "-v", "-t", f"={target}:", opt]).stdout.rstrip("\n")


def _question(r: dict) -> dict | None:
    return terminal_prompt.pending_question(r["tmux"], r["pid"]) if r["alive"] and r["ask"] else None


def list_for(owner: str) -> list[dict]:
    """Terminais de atalho da sessao `owner`, do mais antigo pro mais novo."""
    rows = [r for r in _rows() if r["owner"] == owner]
    rows.sort(key=lambda r: (r["created"], r["seq"], r["tmux"]))
    return [{**{k: r[k] for k in ("id", "label", "alive", "exit_code", "created", "ask", "key")},
             "question": _question(r)} for r in rows]


def find(owner: str, ident: str) -> str | None:
    """Alvo tmux do terminal `ident` SE ele pertence a `owner`. O dono e conferido aqui, no
    servidor: o id vem do cliente e nunca pode alcancar a sessao de outra conversa."""
    if not _ID_RE.match(ident or ""):
        return None
    return next((r["tmux"] for r in _rows() if r["owner"] == owner and r["id"] == ident), None)


def status(target: str) -> tuple[bool, int | None]:
    """(vivo, codigo de saida). Sem resposta do tmux conta como vivo: nada a relatar ainda."""
    if _IS_WINDOWS:
        return _windows_status(target.rsplit("-", 1)[-1])
    cp = tmux._run(["tmux", "display", "-p", "-t", f"={target}:", "#{pane_dead}\t#{pane_dead_status}"])
    dead, _, code = cp.stdout.strip().partition("\t")
    if cp.returncode != 0 or dead != "1":
        return True, None
    return False, int(code) if code.lstrip("-").isdigit() else None


def output(target: str) -> str:
    # O "Pane is dead" do remain-on-exit e do tmux, nao do comando: fica fora do resumo do erro.
    text = tmux.capture_pane(target, lines=200)
    return "\n".join(l for l in text.splitlines() if not l.startswith("Pane is dead"))


def _kill_group(pid: int) -> None:
    if _IS_WINDOWS:
        taskkill = procinfo.taskkill_path()
        if taskkill is None:
            _log.warning("shortcut: taskkill nao encontrado; pid %s segue vivo", pid)
            return
        try:
            subprocess.run([taskkill, "/T", "/F", "/PID", str(pid)], capture_output=True, timeout=10)
        except (subprocess.TimeoutExpired, OSError) as e:
            # Sem o argv: nao carrega nada do atalho, mas o tipo basta.
            _log.warning("shortcut: taskkill nao concluiu para o pid %s: %s", pid, type(e).__name__)
        return
    # O pane e lider de sessao e de grupo (o tmux faz setsid): o grupo leva junto o que o comando
    # abriu em primeiro plano (o xfreerdp do atalho do RDP). O kill-session so fecha o pty, e
    # programa que ignora SIGHUP sobreviveria a ele.
    try:
        os.killpg(pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        return
    limit = time.monotonic() + 1.0
    while time.monotonic() < limit:
        try:
            os.killpg(pid, 0)
        except (ProcessLookupError, PermissionError):
            return
        time.sleep(0.05)
    try:
        os.killpg(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


def _close_row(row: dict) -> bool:
    # No Windows o terminal que "saiu" ainda tem o `cmd` do `pause`, entao mata mesmo sem estar vivo.
    if row["pid"] and (row["alive"] or _IS_WINDOWS):
        _kill_group(row["pid"])
    gone = tmux.kill_session(row["tmux"])
    if _IS_WINDOWS and gone:
        _forget_files(row["id"])
    return gone


def close(owner: str, ident: str) -> bool | None:
    """None = nao existe (ou nao e dessa sessao); False = a sessao tmux sobreviveu."""
    row = next((r for r in _rows() if r["owner"] == owner and r["id"] == ident), None)
    if row is None:
        return None
    return _close_row(row)


def close_all(owner: str) -> None:
    """Best-effort, chamado quando a conversa fecha: falhar aqui nao pode desfazer o kill dela."""
    try:
        rows = _rows()
    except MuxUnavailable:
        _log.warning("shortcut: multiplexador sem resposta; terminais de %r ficaram abertos", owner)
        return
    for row in rows:
        if row["owner"] == owner and not _close_row(row):
            _log.debug("shortcut: %r nao saiu ao fechar %r", row["tmux"], owner)


def rename_owner(old: str, new: str) -> None:
    # O dono e a opcao, nao o nome tmux: basta reapontar a opcao pra lista seguir a conversa.
    try:
        rows = _rows()
    except MuxUnavailable:
        _log.warning("shortcut: multiplexador sem resposta; terminais de %r nao seguiram o rename", old)
        return
    for row in rows:
        if row["owner"] == old:
            tmux._run(["tmux", "set-option", "-t", f"={row['tmux']}:", _OWNER, new])


def start_hangar(key: str, cwd: str, command: str, label: str, env: dict[str, str],
                 origin: str, ask: bool) -> tuple[dict | None, bool]:
    """Copia unica do atalho `key` no servidor. (terminal, reaproveitou). A copia morta sai e
    da lugar a uma nova."""
    key = " ".join(key.split())
    with _HANGAR_LOCK:
        rows = _read_rows()
        if rows is None:
            raise MuxUnavailable(key)
        for row in rows:
            if row["owner"] or row["key"] != key:
                continue
            if row["alive"]:
                return {"id": row["id"], "label": row["label"], "tmux": row["tmux"]}, True
            _close_row(row)
        return start("", cwd, command, label, env, key=key, origin=origin, ask=ask), False


def list_all() -> list[dict]:
    """Todos os terminais de atalho (das sessoes e No Hangar), do mais antigo pro mais novo."""
    # Sem resposta levanta: lista vazia apagaria as abas e as amostras de pergunta pendente.
    found = _rows()
    if _IS_WINDOWS:
        _sweep_orphans({r["id"] for r in found})
    rows = sorted(found, key=lambda r: (r["created"], r["seq"], r["tmux"]))
    terminal_prompt.forget({r["tmux"] for r in rows if r["alive"]})
    return [{**{k: r[k] for k in ("id", "label", "alive", "exit_code", "created", "owner", "key", "origin", "ask")},
             "question": _question(r)} for r in rows]


def hangar_row(ident: str) -> dict | None:
    if not _ID_RE.match(ident or ""):
        return None
    return next((r for r in _rows() if not r["owner"] and r["id"] == ident), None)


def find_hangar(ident: str) -> str | None:
    row = hangar_row(ident)
    return row["tmux"] if row else None


def close_hangar(ident: str) -> bool | None:
    """None = nao existe; False = a sessao sobreviveu."""
    row = hangar_row(ident)
    return None if row is None else _close_row(row)


class RestartError(Exception):
    """O comando ou a pasta do terminal nao puderam ser recuperados inteiros."""


def restart_hangar(ident: str, env: dict[str, str]) -> tuple[dict | None, bool]:
    """"Rodar de novo" de um No Hangar que saiu: mesmo comando, mesma pasta. Vivo = reaproveita."""
    row = hangar_row(ident)
    if row is None:
        return None, False
    cwd, command = _option(row["tmux"], _CWD), _option(row["tmux"], _CMD)
    if _IS_WINDOWS:
        # O psmux come contrabarra no argv (a opcao guardada vem corrompida): o comando original
        # esta inteiro no .cmd dele, e sem ele nao ha o que rodar de novo.
        try:
            with open(_windows_dir() / f"{ident}-cmd.cmd", encoding=_OEM, newline="") as f:
                command = _batch_unescape(f.read().split("\r\n", 1)[1].rstrip("\r\n"))
        except (OSError, IndexError) as e:
            _log.warning("shortcut: sem o .cmd do terminal %s para rodar de novo: %r", ident, e)
            raise RestartError(ident) from e
    if not command.strip() or not cwd:
        _log.warning("shortcut: comando ou pasta do terminal %s nao recuperados para rodar de novo", ident)
        raise RestartError(ident)
    return start_hangar(row["key"], cwd, command, row["label"], env, row["origin"], row["ask"])
