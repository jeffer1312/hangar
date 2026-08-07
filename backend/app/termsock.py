"""Terminal de verdade: um PTY por conexao WebSocket, rodando `tmux attach` na sessao.

O backend NAO interpreta nada aqui — nem ANSI, nem estado, nem o que o agente esta fazendo. Toda a
inteligencia de terminal mora no xterm.js do outro lado; isto e um cano. Mesma escolha do
adapters/codex, que tambem nao parseia TUI.

`pty` NAO e importado no topo: e POSIX-only (puxa tty -> termios) e este modulo e importado pelo
caminho da guarda de 409, que roda no Windows.
"""
import asyncio
import fcntl
import json
import logging
import os
import signal
import struct
import termios
import time
from collections import deque
from typing import Optional

from fastapi import WebSocket
from starlette.websockets import WebSocketDisconnect

from app import tmux
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings

_log = logging.getLogger(__name__)

_LEITURA = 65536
# Um painel por sessao: dois clientes com window-size=latest disputariam o tamanho a cada quadro.
_ativos: dict[str, "Sessao"] = {}


def clientes_ativos() -> set[str]:
    return set(_ativos)


def _winsize(fd: int, cols: int, rows: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def _tamanho_da_janela(name: str) -> Optional[tuple[int, int]]:
    # `={name}` SEM o `:` deixa window_width/window_height VAZIOS no `display -p` (medido: o
    # exact-match de sessao sozinho nao resolve a janela ativa pra essas duas variaveis — outros
    # comandos como resize-window e has-session nao tem esse problema, so a leitura de largura/
    # altura). Com `:` (janela vazia = janela ativa da sessao) volta certo.
    cp = tmux._run(["tmux", "display", "-p", "-t", f"={name}:",
                    "#{window_width}\t#{window_height}"])
    if cp.returncode != 0:
        return None
    w, _, h = cp.stdout.strip().partition("\t")
    return (int(w), int(h)) if w.isdigit() and h.isdigit() else None


class Sessao:
    def __init__(self, name: str, pid: int, master: int, tamanho: Optional[tuple[int, int]]):
        self.name, self.pid, self.master, self.tamanho = name, pid, master, tamanho
        self.desmontada = False


def _abrir_pty(name: str, cols: int, rows: int) -> tuple[int, int]:
    """`pty.fork()` — nao `openpty` + `execvpe`.

    O pty.fork() ja faz login_tty, e sem TERMINAL DE CONTROLE o TIOCSWINSZ no master nao vira
    SIGWINCH no cliente tmux: o redimensionar do painel falharia em silencio.
    """
    import pty                                  # POSIX-only: import aqui, nunca no topo
    env = dict(os.environ)
    # Sem TERM o attach nem abre ("open terminal failed") — o env do servico systemd nao tem. Os
    # outros dois sao o contrato de cor do docs/tmux-truecolor-setup.md.
    env["TERM"] = "xterm-256color"
    env["COLORTERM"] = "truecolor"
    env["CLAUDE_CODE_TMUX_TRUECOLOR"] = "1"
    for herdado in ("NOTIFY_SOCKET", "INVOCATION_ID", "LISTEN_FDS", "LISTEN_PID"):
        env.pop(herdado, None)

    alvo = tmux._pane_target(name)               # roda ANTES do fork: le /proc e forka tmux
    pid, master = pty.fork()
    if pid == 0:
        try:
            os.execvpe("tmux", ["tmux", "attach", "-t", alvo], env)
        except BaseException as e:               # noqa: BLE001
            # Sem este guarda a excecao subiria DENTRO de um fork do processo que tem event loop, e
            # os handlers do pai rodariam no filho. O fd 2 aqui e o proprio PTY: a mensagem chega no
            # terminal do usuario, que e onde ela serve.
            os.write(2, f"\r\n[cockpit] falha ao abrir o terminal: {e}\r\n".encode())
        os._exit(127)
    _winsize(master, cols, rows)
    os.set_blocking(master, False)
    return pid, master


def _desmontar(s: "Sessao") -> None:
    """Idempotente. Ordem importa: fechar -> colher -> esperar o cliente sair -> repor o tamanho."""
    if s.desmontada:
        return                                   # fd/pid podem ter sido REUSADOS pelo processo
    s.desmontada = True
    # `detach-client -s` (servidor) ANTES do SIGHUP: o cliente `tmux attach` instala handler
    # PROPRIO pra SIGHUP (visto em /proc/<pid>/status, SigCgt) e nem sempre sai so com o sinal —
    # medido em loop, ~40% preso ainda anexado so com close+SIGHUP+waitpid. Quem derruba de forma
    # confiavel e o comando do servidor (mesmo que o teste ja usa pra matar o attach de fora);
    # SIGHUP some so como reforco, pro caso do target ja ter sumido (sessao morta por fora).
    tmux._run(["tmux", "detach-client", "-s", f"={s.name}"])
    try:
        os.close(s.master)
    except OSError:
        pass
    try:
        os.kill(s.pid, signal.SIGHUP)
    except ProcessLookupError:
        pass
    try:
        os.waitpid(s.pid, 0)                     # sem isto sobra zumbi por terminal aberto
    except ChildProcessError:
        pass

    if not s.tamanho:
        return
    # Repor ANTES do cliente sair e no-op: com window-size=latest o tmux reimpoe o tamanho dele.
    limite = time.monotonic() + 3.0
    while time.monotonic() < limite:
        cp = tmux._run(["tmux", "list-clients", "-t", f"={s.name}"])
        if cp.returncode != 0:
            return
        linhas = cp.stdout.strip()
        if not linhas:
            break
        time.sleep(0.1)
    else:
        # Estourou os 3s com alguem ainda anexado. Dois casos, mensagens diferentes — no primeiro a
        # janela fica pequena em silencio, e e isso que o log tem que nomear.
        _log.warning("termsock: %r ainda tinha cliente anexado apos 3s; tamanho NAO reposto "
                     "(a janela pode ter ficado no tamanho do painel)", s.name)
        return
    w, h = s.tamanho
    tmux._run(["tmux", "resize-window", "-t", f"={s.name}", "-x", str(w), "-y", str(h)])
    # O resize-window sozinho deixa window-size em MANUAL, e ai um attach nativo posterior abre
    # recortado. Medido: so o par resize + setw devolve o comportamento normal.
    tmux._run(["tmux", "setw", "-t", f"={s.name}", "window-size", "latest"])


async def term_ws(ws: WebSocket, name: str) -> None:
    host = ws.client.host if ws.client else ""
    agora = time.time()
    tok = ws.query_params.get("token", "")
    if _blocked(host, agora):
        await ws.close(code=1008)
        return                                   # NAO registra: registrar aqui estende o bloqueio
    if not settings.auth_token or tok != settings.auth_token:
        if host not in _LOOPBACK:                # mesma isencao do require_auth (auth.py:46)
            _record_fail(host, agora)
        await ws.close(code=1008)                # fecha SEM accept: o PTY nunca chega a nascer
        return
    origem = ws.headers.get("origin")
    if origem and settings.public_url and not origem.startswith(settings.public_url):
        # WebSocket nao e coberto por CORS: sem isto, uma pagina qualquer aberta no navegador do
        # dono poderia abrir um shell usando o cookie/token dele.
        await ws.close(code=1008)
        return
    if not await asyncio.to_thread(tmux.has_session, name):
        await ws.close(code=1008, reason="sessao nao existe")
        return

    try:
        cols = max(20, min(500, int(ws.query_params.get("cols", "80"))))
        rows = max(5, min(200, int(ws.query_params.get("rows", "24"))))
    except ValueError:
        await ws.close(code=1008, reason="cols/rows invalidos")
        return

    if anterior := _ativos.pop(name, None):
        await asyncio.to_thread(_desmontar, anterior)   # gira ate 3s: NUNCA no laco de eventos

    await ws.accept()
    # Tamanho lido ANTES de abrir o PTY: numa unica expressao, o Python avalia da esquerda pra
    # direita e o attach ja teria encolhido a janela — gravariamos 80x24 como "tamanho a repor",
    # que e exatamente como uma sessao fica presa em 99x45 pra sempre (achado do pass).
    tamanho = await asyncio.to_thread(_tamanho_da_janela, name)
    pid, master = await asyncio.to_thread(_abrir_pty, name, cols, rows)
    s = Sessao(name, pid, master, tamanho)
    _ativos[name] = s
    _log.info("termsock: %r anexado (%dx%d)", name, cols, rows)

    loop = asyncio.get_running_loop()
    fim: asyncio.Future = loop.create_future()
    saida: deque[bytes] = deque()
    tem_saida = asyncio.Event()

    def do_pty():
        try:
            dados = os.read(s.master, _LEITURA)
        except BlockingIOError:
            return
        except OSError:
            dados = b""                          # EOF de master no Linux vem como EIO, nao b""
        if not dados:
            loop.remove_reader(s.master)
            if not fim.done():
                fim.set_result(None)
            tem_saida.set()
            return
        saida.append(dados)
        tem_saida.set()

    async def escritor():
        # UMA task escritora, nao uma por leitura: `ensure_future(ws.send_bytes(...))` por leitura
        # embaralha os bytes sob carga (cada send tem await no meio, e a task N pode terminar depois
        # da N+1) e enfileira sem limite num `cat` grande. Achado do pass.
        while True:
            await tem_saida.wait()
            tem_saida.clear()
            while saida:
                await ws.send_bytes(saida.popleft())
            if fim.done() and not saida:
                return

    def escrever_no_pty(b: bytes) -> None:
        # Fd nao-bloqueante: o retorno do os.write e quantos bytes ENTRARAM. Ignorar perde pedaco de
        # paste grande em silencio, e o buffer cheio levanta BlockingIOError.
        while b:
            try:
                n = os.write(s.master, b)
            except BlockingIOError:
                time.sleep(0.005)
                continue
            b = b[n:]

    loop.add_reader(s.master, do_pty)
    tarefa_escritor = asyncio.ensure_future(escritor())

    async def leitor_do_socket():
        while True:
            msg = await ws.receive()
            if msg["type"] == "websocket.disconnect":
                return
            if (b := msg.get("bytes")) is not None:
                escrever_no_pty(b)
            elif (t := msg.get("text")) is not None:
                ctl = json.loads(t)
                if ctl.get("t") == "resize":
                    _winsize(s.master, int(ctl["cols"]), int(ctl["rows"]))

    tarefa_leitor = asyncio.ensure_future(leitor_do_socket())
    try:
        # Esperar os DOIS: so o receive() deixaria o painel congelado pra sempre quando o PTY morre
        # (usuario digita `exit`, sessao acaba). Achado do pass — o `fim` era codigo morto.
        await asyncio.wait({fim, tarefa_leitor}, return_when=asyncio.FIRST_COMPLETED)
    except (WebSocketDisconnect, RuntimeError, ValueError, KeyError):
        pass
    finally:
        for t in (tarefa_leitor, tarefa_escritor):
            t.cancel()
        try:
            loop.remove_reader(s.master)
        except (ValueError, OSError):
            pass
        # Identidade, nao nome: um `pop(name)` cru removeria a Sessao NOVA de uma reconexao que ja
        # tomou o lugar desta, e desmontaria a velha duas vezes (achado do pass).
        if _ativos.get(name) is s:
            del _ativos[name]
        await asyncio.to_thread(_desmontar, s)
        try:
            await ws.close()
        except RuntimeError:
            pass
        _log.info("termsock: %r desanexado", name)
