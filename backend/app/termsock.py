"""Terminal de verdade: um PTY por conexao WebSocket, rodando `tmux attach` na sessao.

O backend NAO interpreta nada aqui — nem ANSI, nem estado, nem o que o agente esta fazendo. Toda a
inteligencia de terminal mora no xterm.js do outro lado; isto e um cano. Mesma escolha do
adapters/codex, que tambem nao parseia TUI.

`pty`, `fcntl` e `termios` NAO sao importados no topo: sao POSIX-only (puxam tty -> termios) e este
modulo e importado pelo caminho da guarda de 409 (Task 3), que roda no Windows — um `import fcntl`
cru no topo vira `ModuleNotFoundError` la, em feature que hoje funciona.
"""
import asyncio
import contextlib
import importlib
import json
import logging
import os
import re
import secrets
import signal
import struct
import threading
import time
from collections import deque
from typing import Optional

from fastapi import WebSocket
from starlette.websockets import WebSocketDisconnect
from uvicorn.protocols.utils import ClientDisconnected

from app import tmux
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings
from app.guest_header import guest_of

_log = logging.getLogger(__name__)


def _importavel(modulo: str) -> bool:
    """O modulo existe E carrega AQUI? — a pergunta e de capacidade, entao a resposta e tentar.

    `os.name == "posix"` responderia outra coisa. E `importlib.util.find_spec("pty")` responderia
    ERRADO: o `pty.py` da stdlib esta presente no Windows tambem (o find_spec acha), e so estoura
    quando ele importa `tty` -> `termios`. Medido nesta VM: `import pty` la levanta
    `ModuleNotFoundError: No module named 'termios'`.
    """
    try:
        importlib.import_module(modulo)
        return True
    except Exception:                            # noqa: BLE001 — qualquer falha e "nao da"
        return False


# Qual motor de PTY esta disponivel nesta maquina — por CAPACIDADE, nao por nome de sistema (mesmo
# motivo do `procinfo._TEM_PROC`: "e unix?" responde SIM no macOS, que nao tem /proc).
#
# O do POSIX e decidido no IMPORT: `import pty` e stdlib e nao faz nada ao processo.
# O do Windows e PREGUICOSO, e isso NAO e estilo — medido em 22/08/2026: importar o `winpty` tinha
# EFEITO COLATERAL no processo (carregava a DLL do ConPTY), e com esse import no topo o
# `test_sessao_escondida_nao_muda_o_custo_da_listagem` passava a falhar quando o arquivo roda junto
# com test_api.py — 204 passed viravam 1 failed, e o teste sozinho passava. O `winpty` saiu (o
# motor de hoje e `app/conpty.py`, ctypes puro), mas a REGRA fica: importar `conpty` nao carrega
# DLL nenhuma, e mesmo assim quem responde a pergunta e quem vai abrir o pty, nao o /api/config.
_PTY_POSIX = _importavel("pty")            # pty.fork() + os.read no master


def tem_pty_windows() -> bool:
    """Da pra abrir um ConPTY aqui? Responde uma vez e guarda (dentro do proprio `conpty`)."""
    from app import conpty
    return conpty.disponivel()


def _rust_owner() -> Optional[object]:
    """O coordenador quando o Rust é (ou será, em `pending`) o dono do painel; senão None."""
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    return owner if owner is not None and owner.mode != "python" else None


def painel_aberto(name: str) -> bool:
    """Há painel anexado à sessão? Com o Rust dono, pergunta a ele (falha levanta
    `ListBridgeError`): o painel do Python não existe nesse modo."""
    if _rust_owner() is not None:
        from app import list_bridge
        return list_bridge.term_active(name)
    return name in _ativos


def painel_disponivel() -> bool:
    """Da pra abrir o painel de terminal NESTA maquina? (o que o /api/config publica)

    Sao DUAS perguntas: o pty tem que abrir E tem que haver laco de leitura pra ele. Hoje as duas
    sao SIM nos dois sistemas — no POSIX por `pty.fork()` + `add_reader`, no Windows por ConPTY +
    `connect_read_pipe` do Proactor (`_motor_windows`). Enquanto faltou o segundo, este `return`
    era so `_PTY_POSIX`: oferecer o painel antes do motor seria um botao que abre terminal morto.

    A pergunta do Windows e feita por ULTIMO de proposito. Ela e barata (o `conpty` nao carrega
    DLL no import, e a resposta e cacheada la dentro), mas o `or` curto-circuita e o caminho POSIX
    — que e o de producao do Linux — nem chega a tocar no modulo.
    """
    owner = _rust_owner()
    if owner is not None and owner.mode == "rust":
        from app import rust_server
        return rust_server.terminal_panel is True
    return _PTY_POSIX or tem_pty_windows()


_LEITURA = 65536
# Teto da fila de saida (bytes acumulados esperando o `send_bytes`): sem isso um cliente lento
# (rede ruim, aba em segundo plano) deixa a fila crescer sem limite num `cat` grande — a task
# escritora unica so resolve a ORDEM dos bytes, nao o volume (achado da revisao).
_SAIDA_MAX = 1 << 20
# Um painel por sessao: dois clientes com window-size=latest disputariam o tamanho a cada quadro.
_ativos: dict[str, "Sessao"] = {}
# Guarda so a checagem+escrita de `Sessao.desmontada`: o caminho de derrubada (nova conexao) e o
# `finally` do handler antigo rodam os dois em `to_thread`, em threads DIFERENTES, e podem entrar
# juntos entre o `if` e o `= True` — sem lock, os dois vencem a corrida e o `os.close` roda 2x num
# fd que o processo ja pode ter reusado (o `except OSError` nao salva: o SEGUNDO close funciona e
# mata o descritor de outra conexao). Achado da revisao.
_lock_desmontagem = threading.Lock()


def clientes_ativos() -> set[str]:
    return set(_ativos)


def _winsize(fd: int, cols: int, rows: int) -> None:
    import fcntl, termios                        # POSIX-only: import aqui, nunca no topo (C1)
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




_DIAG_RESPOSTA_RE = re.compile(rb"^[0-9;?$]*[Rcn~y]?$")


def _diag_entrada(name: str, b: bytes) -> None:
    """Registra RESPOSTA de terminal vinda do cliente, nunca o que a pessoa digita.

    Existe para um defeito medido no Windows: com o painel do terminal aberto, caracteres que
    ninguem digitou aparecem no composer da TUI (ex. "3", "33", "434" na frente do texto). A
    suspeita e a conversa de capacidades do attach — o cliente RESPONDE a pergunta que o tmux/psmux
    fez (CPR `ESC[linha;colunaR`, DA, XTVERSION) e a resposta atravessa ate o pane em vez de ser
    consumida por quem perguntou.

    Filtro deliberado: so passa o que tem ESC ou o que e curto e parece resposta (digitos, `;`, `R`).
    Tecla e texto de quem digita ficam FORA do log — o diagnostico nao pode virar gravador do
    terminal. Desligado por padrao: `CP_DIAG_TERM_INPUT=1` no `.env` liga.
    """
    if not settings.diag_term_input:
        return
    if b"\x1b" not in b and not (len(b) <= 8 and _DIAG_RESPOSTA_RE.match(b)):
        return
    _log.warning("termsock: %r entrada do cliente %r (%d bytes)", name, b[:64], len(b))






class Sessao:
    def __init__(self, name: str, pid: int, master: int, tty: str,
                 tamanho: Optional[tuple[int, int]], ws: WebSocket):
        self.name, self.pid, self.master, self.tty = name, pid, master, tty
        self.tamanho, self.ws = tamanho, ws
        self.desmontada = False
        self.tarefa_escritor: Optional[asyncio.Task] = None   # setado logo apos a criacao


def _abrir_pty(name: str, cols: int, rows: int) -> tuple[int, int, str]:
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

    pid, master = pty.fork()
    if pid == 0:
        try:
            # Mira a SESSAO (={name}:), NAO o pane do agente (tmux._pane_target da Task 1):
            # `attach -t %N` troca a janela/pane ATIVO da sessao pra TODOS os clientes anexados —
            # abrir o painel no navegador arrastaria a visao do `tmux attach` nativo do dono pro
            # pane do agente. `_pane_target` continua valendo pra send-keys/capture-pane; aqui e
            # diferente DE PROPOSITO (achado medido na revisao: %N muda o foco compartilhado).
            os.execvpe("tmux", ["tmux", "attach", "-t", f"={name}:"], env)
        except BaseException as e:               # noqa: BLE001
            # Sem este guarda a excecao subiria DENTRO de um fork do processo que tem event loop, e
            # os handlers do pai rodariam no filho. O fd 2 aqui e o proprio PTY: a mensagem chega no
            # terminal do usuario, que e onde ela serve. O write tambem protegido: se ATE ISSO
            # falhar (pty ja quebrado), sai quieto em vez de propagar dentro do filho forkado.
            try:
                os.write(2, f"\r\n[cockpit] falha ao abrir o terminal: {e}\r\n".encode())
            except BaseException:                # noqa: BLE001
                pass
        os._exit(127)
    # O mestre do `pty.fork()` nasce HERDAVEL: `os.forkpty` nao aplica o PEP 446 que `os.openpty` e
    # `os.pipe` aplicam. Como o backend e de vida longa e guarda uma Sessao (com o `master` dela) por
    # conexao viva, abrir um SEGUNDO painel com o primeiro ainda conectado fazia o `tmux attach` novo
    # -- e tudo que rodasse dentro dele -- herdar um fd de leitura E escrita pro PTY do primeiro:
    # qualquer coisa na segunda sessao podia ler ou injetar bytes na outra. Reproduzido isolado na
    # revisao, com dois `pty.fork()` seguidos no mesmo processo. NAO remover "limpando": e a unica
    # linha que separa os dois terminais.
    os.set_inheritable(master, False)
    tty = os.ptsname(master)                    # guardado AGORA: depois do close() nao da mais
    _winsize(master, cols, rows)
    os.set_blocking(master, False)
    return pid, master, tty


def _desmontar(s: "Sessao") -> None:
    """Idempotente (via lock). Ordem importa: fechar -> colher -> esperar o cliente sair -> repor
    o tamanho."""
    with _lock_desmontagem:
        if s.desmontada:
            return                               # fd/pid podem ter sido REUSADOS pelo processo
        s.desmontada = True
    # `detach-client -t <tty>` (NOSSO cliente, pelo pts) — nao `-s` (sessao inteira): `-s` derruba
    # TODOS os clientes anexados aquela sessao, inclusive um `tmux attach` nativo do dono na kitty
    # (medido na revisao: com dois clientes anexados, `-s` esvazia a lista dos dois). O ptsname foi
    # guardado no `_abrir_pty`, antes do close — depois dele nao da mais pra ler.
    tmux._run(["tmux", "detach-client", "-t", s.tty])
    try:
        os.close(s.master)
    except OSError:
        pass
    try:
        os.kill(s.pid, signal.SIGHUP)
    except ProcessLookupError:
        pass
    # WNOHANG em laco com prazo, nao `waitpid` bloqueante: o cliente `tmux attach` as vezes nao sai
    # nem com detach+SIGHUP (medido em loop: instala handler proprio pra SIGHUP). Um waitpid
    # bloqueante prenderia o worker do threadpool pra sempre — a MESMA regressao que
    # test_fechamento_feio_... existe pra pegar (achado da revisao).
    limite = time.monotonic() + 3.0
    while time.monotonic() < limite:
        try:
            colhido, _ = os.waitpid(s.pid, os.WNOHANG)
        except ChildProcessError:
            break
        if colhido:
            break
        time.sleep(0.05)
    else:
        _log.warning("termsock: %r processo do pty (pid %d) nao saiu apos 3s; seguindo sem "
                     "colher (zumbi ate o backend reiniciar)", s.name, s.pid)

    if not s.tamanho:
        return
    # Repor ANTES do cliente sair e no-op: com window-size=latest o tmux reimpoe o tamanho dele.
    # Espera o NOSSO cliente (pelo tty) sumir da lista — nao a lista inteira esvaziar: um `tmux
    # attach` nativo anexado ao lado nunca sai por nossa conta, e esperar a lista vazia estourava
    # os 3s TODA vez que havia um cliente nativo (achado da revisao).
    limite = time.monotonic() + 3.0
    while time.monotonic() < limite:
        cp = tmux._run(["tmux", "list-clients", "-t", f"={s.name}", "-F", "#{client_tty}"])
        if cp.returncode != 0:
            return
        if s.tty not in cp.stdout.split():
            break
        time.sleep(0.1)
    else:
        # Estourou os 3s com NOSSO cliente ainda anexado. Dois casos, mensagens diferentes — no
        # primeiro a janela fica pequena em silencio, e e isso que o log tem que nomear.
        _log.warning("termsock: %r ainda tinha cliente anexado apos 3s; tamanho NAO reposto "
                     "(a janela pode ter ficado no tamanho do painel)", s.name)
        return
    w, h = s.tamanho
    tmux._run(["tmux", "resize-window", "-t", f"={s.name}", "-x", str(w), "-y", str(h)])
    # O resize-window sozinho deixa window-size em MANUAL, e ai um attach nativo posterior abre
    # recortado. Medido: so o par resize + setw devolve o comportamento normal.
    tmux._run(["tmux", "setw", "-t", f"={s.name}", "window-size", "latest"])


async def term_ws(ws: WebSocket, name: str, resolve=None) -> None:
    """Porta de entrada compartilhada -> motor da plataforma.

    DOIS MOTORES, nao um compartilhado. O que muda entre os sistemas nao e detalhe de chamada: no
    POSIX o pty e um fd que o `add_reader` observa e o `os.read` puxa; no Windows e um ConPTY cujo
    transporte EMPURRA os bytes por callback, e o desmonte nao tem identidade de cliente pra
    esperar (o psmux ignora `list-clients -F`). Espremer os dois num motor so daria um denominador
    comum pior que qualquer um deles — e o Linux nao pode piorar em nada.
    O que e MESMO nos dois — autenticacao, Origin, sessao existe, clamp de cols/rows — fica aqui em
    cima, uma vez so: e o caminho que abre um shell completo, e duplica-lo seria duplicar a trava.
    """
    prep = await _porta_de_entrada(ws, name, resolve)
    if prep is None:
        return                                   # ja fechou o ws com o codigo certo
    cols, rows, name = prep
    from app import runtime_coordinator
    try:
        rust = await _terminal_do_rust()
    except runtime_coordinator.RuntimeStarting:
        await _recusa_ponte(ws, name, "terminal_runtime_starting")
        return
    if rust:
        await _motor_rust(ws, name, cols, rows)
    elif _PTY_POSIX:
        await _motor_posix(ws, name, cols, rows)
    else:
        await _motor_windows(ws, name, cols, rows)


async def _porta_de_entrada(ws: WebSocket, name: str, resolve=None) -> Optional[tuple[int, int, str]]:
    """Trava e validacao, iguais nos dois motores. Devolve (cols, rows, alvo tmux) ou None se ja
    recusou. `resolve` troca o alvo (terminal de atalho da sessao) e so roda depois do token."""
    host = ws.client.host if ws.client else ""
    agora = time.time()
    tok = ws.query_params.get("token", "")
    from app import shared_auth
    decision = await shared_auth.websocket(ws, shared_auth.TERMINAL_WS,
                                          verified_guest=guest_of(ws) is not None)
    if decision is False:
        return None
    if decision is not None and decision.status not in (shared_auth.OWNER, shared_auth.GUEST):
        await ws.close(code=1008)
        return None
    if decision is None and _blocked(host, agora):
        await ws.close(code=1008)
        return                                   # NAO registra: registrar aqui estende o bloqueio
    # Convidado ja passou pelo porteiro (token dele + a sessao dele); a Origin e a do app DELE.
    if guest_of(ws) is None:
        # compare_digest, nao `!=` de string (mesmo cuidado do require_auth, auth.py:104): `!=` sai fora
        # na primeira letra diferente e vira canal lateral de tempo. Este e o endpoint que abre um shell
        # completo. `.encode()` tambem evita o TypeError do compare_digest com string nao-ASCII.
        dono = (decision.status == shared_auth.OWNER if decision is not None else
                bool(settings.auth_token) and secrets.compare_digest(tok.encode(), settings.auth_token.encode()))
        if not dono:
            if host not in _LOOPBACK:            # mesma isencao do require_auth (auth.py:46)
                _record_fail(host, agora)
            await ws.close(code=1008)            # fecha SEM accept: o PTY nunca chega a nascer
            return
    if resolve is not None:
        try:
            name = await asyncio.to_thread(resolve)
        except tmux.MuxIndisponivel:
            # Sem resposta nao da pra dizer que o terminal nao existe: o cliente tenta de novo.
            _log.warning("termsock: multiplexador sem resposta ao resolver o terminal")
            await ws.close(code=1013, reason="multiplexador indisponivel")
            return None
        if not name:
            await ws.close(code=1008, reason="terminal nao existe")
            return None
    if not await asyncio.to_thread(tmux.has_session, name):
        await ws.close(code=1008, reason="sessao nao existe")
        return

    try:
        cols = max(20, min(500, int(ws.query_params.get("cols", "80"))))
        rows = max(5, min(200, int(ws.query_params.get("rows", "24"))))
    except ValueError:
        await ws.close(code=1008, reason="cols/rows invalidos")
        return None
    return cols, rows, name


async def _terminal_do_rust() -> bool:
    """O PTY é do Rust? Em `pending` espera o desfecho (levanta `RuntimeStarting` no prazo)."""
    owner = _rust_owner()
    if owner is None:
        return False
    if owner.mode == "pending":
        await owner.await_mode()
    return owner.mode != "python"


async def _recusa_ponte(ws: WebSocket, name: str, codigo: str, **campos) -> None:
    """Sem o Rust, nenhum PTY abre aqui: 1013 (o cliente tenta de novo) e o código no diário."""
    from app import diag
    _log.warning("termsock: %r sem o terminal do Rust (%s %s)", name, codigo, campos)
    diag.registrar("terminal.ponte", "erro", codigo=codigo, sessao=name, **campos)
    await ws.accept()
    await ws.close(code=1013, reason="terminal indisponivel")


async def _motor_rust(ws: WebSocket, name: str, cols: int, rows: int) -> None:
    """Liga os bytes do cliente ao painel do Rust (`/__hangar_server/term` na porta privada).

    A porta de entrada já rodou aqui; o Rust recebe só o alvo conferido e tem o painel único de
    todas as portas. Fechamento do Rust (1000 "outra conexao assumiu", 1011, 1013) chega igual ao
    cliente; cliente que sai fecha o lado do Rust, que desmonta.
    """
    from urllib.parse import urlencode
    from websockets.asyncio.client import connect
    from websockets.exceptions import ConnectionClosed, InvalidStatus
    from websockets.protocol import State
    from app import diag, list_bridge
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        await _recusa_ponte(ws, name, "terminal_bridge_off")
        return
    address, secret = endpoint
    url = f"ws://{address}/__hangar_server/term?" + urlencode({"target": name, "cols": cols, "rows": rows})
    try:
        # Sem proxy do ambiente, sem compressão e sem ping: o Rust pinga este lado, e o uvicorn
        # pinga o cliente. Quadros do Rust têm até 64 KiB; o teto é o do cliente nativo.
        upstream = await connect(url, additional_headers={"x-hangar-internal": secret}, proxy=None,
                                 compression=None, ping_interval=None, open_timeout=5,
                                 close_timeout=2, max_size=_SAIDA_MAX)
    except InvalidStatus as e:
        status = e.response.status_code
        # 403 é a sessão que morreu depois da porta de entrada: como o `has_session` daqui.
        if status == 403:
            await ws.close(code=1008, reason="sessao nao existe")
            return
        await _recusa_ponte(ws, name, "terminal_bridge_refused", status=status)
        return
    except Exception as e:                       # noqa: BLE001 — qualquer falha da ponte vira código
        codigo = "terminal_bridge_timeout" if isinstance(e, TimeoutError) else "terminal_bridge_unavailable"
        await _recusa_ponte(ws, name, codigo, tipo=type(e).__name__)
        return

    async def do_cliente():
        while True:
            msg = await ws.receive()
            if msg["type"] == "websocket.disconnect":
                return
            if (b := msg.get("bytes")) is not None:
                await upstream.send(b)
            elif (t := msg.get("text")) is not None:
                await upstream.send(t)

    async def do_rust():
        async for dados in upstream:
            if isinstance(dados, bytes):
                await ws.send_bytes(dados)
            else:
                await ws.send_text(dados)

    tarefas: set[asyncio.Future] = set()
    rust_fechou = False
    try:
        await ws.accept()
        _log.info("termsock: %r ligado ao terminal do Rust (%dx%d)", name, cols, rows)
        tarefas = {asyncio.ensure_future(do_cliente()), asyncio.ensure_future(do_rust())}
        await asyncio.wait(tarefas, return_when=asyncio.FIRST_COMPLETED)
    finally:
        # Cancelamento que chega durante a limpeza interrompe qualquer `await` daqui; o `abort` do
        # fim roda mesmo assim, senão o Rust fica com a conexão do repasse presa.
        try:
            for t in tarefas:
                t.cancel()
            if tarefas:
                # `wait`, não `await` em cada uma: o cancelamento desta task (convidado revogado)
                # segue valendo em vez de ser engolido junto com o das filhas.
                await asyncio.wait(tarefas)
            for t in tarefas:
                erro_t = None if t.cancelled() else t.exception()
                if erro_t is not None and not isinstance(
                        erro_t, (ConnectionClosed, WebSocketDisconnect, ClientDisconnected)):
                    _log.error("termsock: %r — repasse terminou com %s", name, type(erro_t).__name__)
            # O Rust fechou primeiro: o código dele vale, quem terminou antes aqui não importa.
            rust_fechou = upstream.state is not State.OPEN
            await upstream.close()
        finally:
            if upstream.state is not State.CLOSED:
                upstream.transport.abort()
    if rust_fechou:
        code, reason = upstream.close_code, upstream.close_reason or ""
        if code in (None, 1005, 1006):
            # Sem código: o painel do outro lado caiu sem fechar.
            diag.registrar("terminal.ponte", "erro", codigo="terminal_bridge_dropped", sessao=name)
            code, reason = 1011, "terminal do Rust caiu"
        with contextlib.suppress(RuntimeError, WebSocketDisconnect, ClientDisconnected):
            await ws.close(code=code, reason=reason)
    _log.info("termsock: %r desligado do terminal do Rust", name)


async def _motor_posix(ws: WebSocket, name: str, cols: int, rows: int) -> None:
    """Motor POSIX: `pty.fork()` + `add_reader`/`add_writer` no fd do mestre.

    Movido INTEIRO de dentro do `term_ws`, sem uma linha mudada — o caminho do Linux sai
    byte-identico ao que estava aqui antes do Windows existir.
    """
    loop = asyncio.get_running_loop()

    if anterior := _ativos.pop(name, None):
        # Remove o reader ANTES de fechar o fd: senao o epoll larga o descritor com o handler
        # antigo ainda parado no asyncio.wait (o remove_reader dele so roda no finally, que essa
        # tarefa nunca alcanca sozinha), e a conexao NOVA nasce muda — o pty.fork() reusa o MESMO
        # numero de fd e o add_reader dela vira um `modify` que o kernel ignora, porque a mascara
        # nao mudou (medido com asyncio real na revisao: fd reusado, callback do novo nunca
        # dispara). Fecha o ws antigo tambem: sem isso quem foi derrubado fica com terminal
        # congelado pra sempre, sem nenhum aviso de desconexao (achado da revisao).
        try:
            loop.remove_reader(anterior.master)
        except (ValueError, OSError):
            pass
        try:
            loop.remove_writer(anterior.master)   # simetria do C3: o writer tambem mira este fd
        except (ValueError, OSError):
            pass
        # Para a task escritora ANTES de fechar o ws, e ESPERA ela parar: sem isto, o escritor da
        # conexao velha pode estar no meio de um `send_bytes` (bytes que ja tinham sido lidos do
        # pty antes do remove_reader acima) bem na hora que o `close()` roda logo abaixo — o
        # Starlette recusa `send()` depois de `close()` com RuntimeError, e a excecao nasce numa
        # task que ninguem espera (medido rodando o teste de derrubada em loop: corrida real, nao
        # hipotetica). `cancel()` so AGENDA a interrupcao; so o `await` garante que ela chegou
        # antes da gente tocar no mesmo ws.
        if anterior.tarefa_escritor is not None:
            anterior.tarefa_escritor.cancel()
            with contextlib.suppress(BaseException):
                await anterior.tarefa_escritor
        try:
            await anterior.ws.close(code=1000, reason="outra conexao assumiu")
        except RuntimeError:
            pass
        await asyncio.to_thread(_desmontar, anterior)   # gira ate 6s: NUNCA no laco de eventos

    await ws.accept()
    # Tamanho lido ANTES de abrir o PTY: numa unica expressao, o Python avalia da esquerda pra
    # direita e o attach ja teria encolhido a janela — gravariamos 80x24 como "tamanho a repor",
    # que e exatamente como uma sessao fica presa em 99x45 pra sempre (achado do pass).
    tamanho = await asyncio.to_thread(_tamanho_da_janela, name)
    pid, master, tty = await asyncio.to_thread(_abrir_pty, name, cols, rows)
    s = Sessao(name, pid, master, tty, tamanho, ws)
    _ativos[name] = s
    _log.info("termsock: %r anexado (%dx%d)", name, cols, rows)

    fim: asyncio.Future = loop.create_future()
    saida: deque[bytes] = deque()
    saida_bytes = 0
    saida_pausada = False
    tem_saida = asyncio.Event()
    entrada: deque[bytes] = deque()

    def do_pty():
        nonlocal saida_bytes, saida_pausada
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
        saida_bytes += len(dados)
        tem_saida.set()
        if saida_bytes >= _SAIDA_MAX:
            # Quem PAUSA marca o flag, nao quem drena: `do_pty` pode disparar DE NOVO no MEIO do
            # `await ws.send_bytes` do escritor — e exatamente onde o laco de eventos roda
            # callbacks de reader. Se o escritor recalculasse "pausado" so no TOPO do proprio laco
            # (antes de entrar no `while saida`), uma pausa cruzada durante o dreno nunca seria
            # vista: o reader some de vez, e como o EOF do pty so o reader percebe, `fim` tambem
            # nunca resolve — terminal mudo pra sempre. Medido com `sim_i8.py` (achado da
            # revisao): reader nunca mais registrado, bytes enviados param em definitivo.
            saida_pausada = True
            loop.remove_reader(s.master)

    async def escritor():
        # UMA task escritora, nao uma por leitura: `ensure_future(ws.send_bytes(...))` por leitura
        # embaralha os bytes sob carga (cada send tem await no meio, e a task N pode terminar depois
        # da N+1). Achado do pass.
        nonlocal saida_bytes, saida_pausada
        while True:
            await tem_saida.wait()
            tem_saida.clear()
            while saida:
                b = saida.popleft()
                saida_bytes -= len(b)
                await ws.send_bytes(b)
            if saida_pausada and not fim.done():
                saida_pausada = False
                loop.add_reader(s.master, do_pty)   # reata a leitura: a fila esvaziou
            if fim.done() and not saida:
                return

    def _drenar_entrada():
        while entrada:
            b = entrada[0]
            try:
                n = os.write(s.master, b)
            except BlockingIOError:
                loop.add_writer(s.master, _drenar_entrada)
                return
            except OSError:
                entrada.clear()
                break
            if n < len(b):
                entrada[0] = b[n:]
                loop.add_writer(s.master, _drenar_entrada)
                return
            entrada.popleft()
        try:
            loop.remove_writer(s.master)
        except (ValueError, OSError):
            pass

    def escrever_no_pty(b: bytes) -> None:
        _diag_entrada(name, b)
        # `add_writer`, nao `time.sleep` bloqueante: isto roda DENTRO do laco de eventos (chamado
        # direto de leitor_do_socket) — um `time.sleep` ali travava o backend inteiro (SSE de
        # todas as sessoes, listagem, tudo) enquanto o buffer do pty estivesse cheio (paste
        # grande, tmux nativo parado). Achado da revisao — era a restricao explicita do projeto
        # sendo quebrada. Tenta escrever na hora (fila vazia = sem overhead pro caso comum); so
        # registra o writer quando o fd realmente emperra.
        vazia = not entrada
        entrada.append(b)
        if vazia:
            _drenar_entrada()

    loop.add_reader(s.master, do_pty)
    tarefa_escritor = asyncio.ensure_future(escritor())
    s.tarefa_escritor = tarefa_escritor          # visivel pra quem evict essa Sessao depois

    # Aviso-uma-vez por CONEXAO do quadro de controle invalido: um cliente com bug manda o
    # frame ruim em rajada, e uma linha por frame seria a enxurrada que o log nao aguenta.
    avisou_frame_torto = False

    async def leitor_do_socket():
        nonlocal avisou_frame_torto
        while True:
            try:
                msg = await ws.receive()
                if msg["type"] == "websocket.disconnect":
                    return
                if (b := msg.get("bytes")) is not None:
                    escrever_no_pty(b)
                elif (t := msg.get("text")) is not None:
                    # Quadro de controle malformado se IGNORA, nao derruba o terminal (achado da
                    # revisao): JSON valido que nao e OBJETO (`"5"`, `null`, `[1,2]`) levantava
                    # AttributeError no `.get`, e `cols` com lista/dict levantava TypeError no
                    # `int()` — nenhum dos dois estava no `except` la de baixo, entao um unico frame
                    # torto matava a task do leitor e fechava a CONEXAO inteira. `isinstance` +
                    # suppress local: o laco continua, so o frame ruim e descartado.
                    try:
                        ctl = json.loads(t)
                        if isinstance(ctl, dict) and ctl.get("t") == "resize":
                            # Mesmo clamp do connect (:cols/:rows acima) — sem ele, {"cols":99999}
                            # ou negativo estoura o struct.pack de _winsize.
                            c = max(20, min(500, int(ctl["cols"])))
                            r = max(5, min(200, int(ctl["rows"])))
                            _winsize(s.master, c, r)
                    except (ValueError, TypeError, KeyError) as e:
                        # Descarta o frame MAS deixa rastro: descartar calado trocaria um erro
                        # barulhento (antes, a task morria e o asyncio logava o traceback) por
                        # silencio total — um bug de verdade no front mandando cols/rows fora de
                        # forma sumiria sem nenhuma linha no log. Avisa UMA vez por conexao, mesma
                        # politica do agentpane: um cliente com bug manda o frame ruim em rajada.
                        if not avisou_frame_torto:
                            avisou_frame_torto = True
                            _log.warning("termsock: %r mandou quadro de controle invalido (%s): %.80r "
                                         "— descartado, aviso unico nesta conexao", name, e, t)
            except (WebSocketDisconnect, RuntimeError, ValueError, KeyError):
                # O try vivia em volta do `asyncio.wait` la embaixo, onde nunca pegava nada:
                # `asyncio.wait` nao propaga excecao de dentro das tasks que espera — o socket
                # caindo no meio do `receive()` matava esta task em silencio (so "Task exception was
                # never retrieved" no log) sem fechar o terminal. Achado da revisao. Aqui sobrou o
                # que e da CONEXAO (receive num ws morto, `msg` sem "type"); frame de controle torto
                # morre no `suppress` de cima, sem derrubar o terminal.
                return

    tarefa_leitor = asyncio.ensure_future(leitor_do_socket())
    try:
        # Esperar os TRES: so o receive() deixaria o painel congelado pra sempre quando o PTY morre
        # (usuario digita `exit`, sessao acaba). Achado do pass — o `fim` era codigo morto.
        #
        # O ESCRITOR entrou no conjunto na revisao final (I5). Ele morrendo por excecao que nao
        # seja desconexao — com `saida_pausada` True, quando so ele reata o reader — deixava o
        # reader fora do laco, `fim` sem nunca resolver e o painel MUDO com o socket ABERTO: sem
        # `onclose` no navegador, nem o botao "desconectado · reconectar" aparecia. Terminar aqui
        # fecha o socket e devolve ao usuario a unica coisa acionavel, que e reconectar. No caminho
        # saudavel isto nao muda nada: o escritor so retorna sozinho depois de `fim` resolver e a
        # fila esvaziar, quando o `wait` ja teria voltado pelo `fim`.
        await asyncio.wait({fim, tarefa_leitor, tarefa_escritor},
                           return_when=asyncio.FIRST_COMPLETED)
    finally:
        tarefa_leitor.cancel()
        if fim.done():
            # PTY morreu por conta propria (ex: `exit`): deixa o escritor escoar o que sobrou antes
            # de cancelar — cancelar direto perde a ultima tela (achado da revisao). So faz sentido
            # quando `fim` ja resolveu; se foi o CLIENTE que desconectou, ninguem esta olhando
            # mesmo, e drenar so atrasaria o fechamento a toa.
            # `suppress(BaseException)`, nao so Timeout/Cancelled: `wait_for` RE-LEVANTA o que a
            # task escritora levantar, e um `ws.send_bytes` num socket que ja caiu pode levantar
            # RuntimeError ou WebSocketDisconnect — nenhum dos dois estava na tupla. Isto e um
            # `finally`: uma excecao aqui pulava TODO o resto (cancel do escritor, remove_reader/
            # writer, del em _ativos, _desmontar, ws.close) e deixava a Sessao fantasma em _ativos
            # pra sempre (achado da revisao — mesma inconsistencia do `contextlib.suppress` que ja
            # existe no caminho de derrubada, mesmo remedio).
            with contextlib.suppress(BaseException):
                await asyncio.wait_for(tarefa_escritor, timeout=1.0)
        tarefa_escritor.cancel()
        # Recupera a excecao das DUAS tasks SEMPRE, fora do `if` (achado da revisao): o dreno acima
        # so roda quando `fim` resolveu, e `cancel()` numa task que JA terminou com excecao e no-op
        # — nao recupera nada, e a excecao vira "Task exception was never retrieved" com traceback
        # no log. Vale igual pro leitor: ele tambem pode ter terminado com excecao antes do cancel.
        # `await` numa task cancelada volta na hora (o CancelledError e entregue no ponto de await),
        # e o suppress cobre os dois casos.
        # E LOGA: recuperar calado trocaria um erro barulhento (o handler default do asyncio ainda
        # imprimia o traceback, tarde e feio, mas imprimia) por silencio total — um bug de verdade
        # no escritor ou no leitor sumiria, e o usuario so veria o terminal cair sem nenhuma linha
        # no log dizendo por que. CancelledError fica de fora: e o encerramento normal, nao falha.
        await _colher_tarefas(name, tarefa_leitor, tarefa_escritor)
        # So remove reader/writer se ESTA Sessao ainda nao foi desmontada por outro caminho: o
        # `pty.fork()` reusa o MESMO numero de fd, e o caminho de derrubada (acima) ja fez essa
        # remocao e fechou o fd antes de devolver o numero pro SO. Se o handler velho acorda
        # TARDE (socket morto de celular, que e justamente o caso comum de acordar tarde), essas
        # duas chamadas desregistrariam os callbacks da conexao NOVA que ja tomou aquele fd —
        # exatamente o sintoma que o C3 existe pra matar. `desmontada` sem lock aqui: o valor so
        # vira True bem antes desta leitura (setado no INICIO de `_desmontar`, que ja rodou por
        # completo do outro lado antes do fd ser reusado), entao nao ha corrida real pra fechar
        # (achado da revisao). Se ainda nao desmontou, o fd esta aberto e a remocao e segura.
        if not s.desmontada:
            try:
                loop.remove_reader(s.master)
            except (ValueError, OSError):
                pass
            try:
                loop.remove_writer(s.master)
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


# ===========================================================================================
# Motor do Windows: ConPTY + transportes do Proactor. Mesma FORMA do motor POSIX de proposito —
# fila de saida unica, uma task escritora, contrapressao por teto de bytes — porque o
# `pause_reading()`/`resume_reading()` do `_ProactorReadPipeTransport` e substituto um-para-um do
# `remove_reader`/`add_reader` (parar de ler enche o buffer do pipe e bloqueia o filho). Nao ha
# thread nem fila entre os dois lados: o Proactor le por I/O sobreposto e entrega por callback.
# ===========================================================================================


class SessaoWindows:
    """O que o motor do Windows guarda por conexao viva. Espelha a `Sessao` do POSIX.

    Nao ha `master`/`tty` aqui, e a ausencia e a diferenca de fundo entre os dois motores: no
    psmux nao existe identidade de cliente (`list-clients -F` e ignorado e todo cliente aparece
    com o mesmo tty ficticio), entao nada do desmonte pode ser "espere o NOSSO tty sumir da lista".
    O que faz as vezes disso e o pid do nosso proprio processo de attach, que e conhecido porque e
    nosso.
    """

    def __init__(self, name: str, pty, ws: WebSocket):
        self.name, self.pty, self.ws = name, pty, ws
        self.desmontada = False
        self.tarefa_escritor: Optional[asyncio.Task] = None
        self.transporte_leitura = None
        self.transporte_escrita = None

    def pausar_leitura(self) -> None:
        """Equivalente do `remove_reader` do POSIX no caminho de derrubada."""
        try:
            if self.transporte_leitura is not None:
                self.transporte_leitura.pause_reading()
        except (RuntimeError, OSError):
            pass

    def fechar_transportes(self) -> None:
        """Fecha os dois transportes — e, com eles, os handles do nosso lado do pipe.

        Roda NO LACO DE EVENTOS, nunca em `to_thread`: transporte e objeto do loop. E por isto que
        o `ConPty.encerrar()` (que roda em thread, porque espera o filho morrer) NAO fecha esses
        dois handles — a posse deles passou pro `PipeHandle` de cada transporte, e fechar dos dois
        lados seria double-close num numero que o processo ja pode ter reusado.
        """
        for t in (self.transporte_leitura, self.transporte_escrita):
            try:
                if t is not None:
                    t.close()
            except (RuntimeError, OSError):
                pass


def _abrir_conpty(name: str, cols: int, rows: int):
    """`tmux attach` na SESSAO dentro de um ConPTY novo. Bloqueante — chame em `to_thread`."""
    import subprocess

    from app import conpty
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    env["COLORTERM"] = "truecolor"
    env["CLAUDE_CODE_TMUX_TRUECOLOR"] = "1"
    # O psmux RECUSA `tmux attach` de dentro de um pane, e ele descobre isso por esta variavel. O
    # backend normalmente roda fora de pane e nao esbarra — mas quem o subir de dentro de uma
    # sessao veria TODO painel falhar por um motivo que nao aparece em lugar nenhum.
    env.pop("PSMUX_SESSION", None)
    # Mira a SESSAO (`={name}:`), NAO o pane — mesma razao do motor POSIX (invariante 2): `attach`
    # troca a janela/pane ativo pra TODOS os clientes anexados.
    # `list2cmdline` porque no Windows a linha de comando e UMA string: montar na mao quebraria em
    # nome de sessao com espaco.
    linha = subprocess.list2cmdline(["tmux", "attach", "-t", f"={name}:"])
    return conpty.abrir(linha, cols, rows, env)


def _desmontar_windows(s: "SessaoWindows") -> None:
    """Idempotente (via lock, igual ao POSIX). Bloqueante: espera o filho morrer.

    NAO ha o passo de repor o tamanho da janela que o `_desmontar` do POSIX tem no fim, e isso foi
    MEDIDO, nao esquecido (22/08/2026): no psmux o tamanho da janela acompanha o cliente que esta
    anexado no momento — o painel sai deixando 120x29 gravado, o proximo cliente anexa a 80x24 e a
    janela vira 80x23 sozinha. Nao ha o que repor. E nem daria: `resize-window` e
    `setw window-size latest` voltam rc=0 no psmux e nao fazem NADA (tambem medido), entao copiar o
    passo do POSIX seria um par de comandos que mente que funcionou.
    """
    with _lock_desmontagem:
        if s.desmontada:
            return
        s.desmontada = True
    s.pty.encerrar()


async def _motor_windows(ws: WebSocket, name: str, cols: int, rows: int) -> None:
    loop = asyncio.get_running_loop()

    if anterior := _ativos.pop(name, None):
        # Mesma ordem do POSIX e pelos mesmos motivos: para de ler ANTES de mexer no resto, para a
        # task escritora e ESPERA ela (senao ela pode estar no meio de um `send_bytes` quando o
        # `close()` abaixo roda, e o Starlette levanta RuntimeError numa task que ninguem espera),
        # e so entao fecha o ws antigo — sem isso quem foi derrubado fica com terminal congelado
        # pra sempre, sem aviso de desconexao.
        anterior.pausar_leitura()
        if anterior.tarefa_escritor is not None:
            anterior.tarefa_escritor.cancel()
            with contextlib.suppress(BaseException):
                await anterior.tarefa_escritor
        try:
            await anterior.ws.close(code=1000, reason="outra conexao assumiu")
        except RuntimeError:
            pass
        # Os transportes fecham DEPOIS do `ws.close()`, e a ordem foi corrigida por medicao: no
        # POSIX o `remove_reader` nao produz EOF, entao o handler derrubado fica parado no
        # `asyncio.wait` ate o close chegar — quem o encerra e o close, sempre. Fechando o
        # transporte ANTES, o `connection_lost` resolvia o `fim` e o handler velho corria pra
        # fechar o proprio ws ao mesmo tempo que este aqui: o cliente derrubado as vezes perdia o
        # quadro de close e via a conexao sumir sem aviso — que e exatamente o sintoma que a
        # invariante I1 existe pra impedir (terminal congelado, sem "desconectado · reconectar").
        anterior.fechar_transportes()
        await asyncio.to_thread(_desmontar_windows, anterior)

    await ws.accept()
    try:
        pty = await asyncio.to_thread(_abrir_conpty, name, cols, rows)
    except OSError as e:
        # Falhar aqui e visivel pro usuario, nao silencioso: sem o `close` com motivo, o painel
        # ficaria em branco esperando bytes que nunca vem.
        _log.warning("termsock: %r nao consegui abrir o ConPTY: %r", name, e)
        with contextlib.suppress(RuntimeError):
            await ws.close(code=1011, reason="nao consegui abrir o terminal")
        return

    s = SessaoWindows(name, pty, ws)
    _ativos[name] = s
    _log.info("termsock: %r anexado (%dx%d, conpty pid %d)", name, cols, rows, pty.pid)

    fim: asyncio.Future = loop.create_future()
    saida: deque[bytes] = deque()
    saida_bytes = 0
    saida_pausada = False
    tem_saida = asyncio.Event()

    class Leitor(asyncio.Protocol):
        def connection_made(self, transport) -> None:
            # Guardado AQUI, nao no retorno do `connect_read_pipe`: o `connect_read_pipe` tem um
            # `await` dentro, e o laco pode entregar `data_received` antes de devolver o
            # transporte pra gente — o `connection_made` e o unico ponto garantidamente anterior.
            s.transporte_leitura = transport

        def data_received(self, dados: bytes) -> None:
            nonlocal saida_bytes, saida_pausada
            saida.append(dados)
            saida_bytes += len(dados)
            tem_saida.set()
            if saida_bytes >= _SAIDA_MAX and not saida_pausada:
                # Quem PAUSA marca o flag, nao quem drena — mesma corrida do motor POSIX: isto
                # pode disparar no MEIO do `await ws.send_bytes` do escritor, e se ele so
                # recalculasse "pausado" no topo do proprio laco, uma pausa cruzada durante o
                # dreno nunca seria vista e a leitura pararia pra sempre.
                saida_pausada = True
                s.transporte_leitura.pause_reading()

        def connection_lost(self, exc: Optional[BaseException]) -> None:
            # EOF do ConPTY = o filho (`tmux attach`) saiu. Equivale ao `os.read` devolvendo b"".
            if not fim.done():
                fim.set_result(None)
            tem_saida.set()

    try:
        await loop.connect_read_pipe(Leitor, _pipe_handle(pty.saida))
        transporte_escrita, _ = await loop.connect_write_pipe(
            asyncio.BaseProtocol, _pipe_handle(pty.entrada))
        s.transporte_escrita = transporte_escrita
    except OSError as e:
        _log.warning("termsock: %r nao consegui ligar os pipes do ConPTY: %r", name, e)
        s.fechar_transportes()
        if _ativos.get(name) is s:
            del _ativos[name]
        await asyncio.to_thread(_desmontar_windows, s)
        with contextlib.suppress(RuntimeError):
            await ws.close(code=1011, reason="nao consegui abrir o terminal")
        return

    async def escritor():
        # UMA task escritora, mesmo motivo do POSIX: um `ensure_future(send_bytes)` por leitura
        # embaralha os bytes sob carga.
        nonlocal saida_bytes, saida_pausada
        while True:
            await tem_saida.wait()
            tem_saida.clear()
            while saida:
                b = saida.popleft()
                saida_bytes -= len(b)
                await ws.send_bytes(b)
            if saida_pausada and not fim.done():
                saida_pausada = False
                with contextlib.suppress(RuntimeError, OSError):
                    s.transporte_leitura.resume_reading()
            if fim.done() and not saida:
                return

    tarefa_escritor = asyncio.ensure_future(escritor())
    s.tarefa_escritor = tarefa_escritor

    avisou_frame_torto = False

    async def leitor_do_socket():
        nonlocal avisou_frame_torto
        while True:
            try:
                msg = await ws.receive()
                if msg["type"] == "websocket.disconnect":
                    return
                if (b := msg.get("bytes")) is not None:
                    # Sem `_drenar_entrada`/`add_writer` como no POSIX: o
                    # `_ProactorWritePipeTransport` JA enfileira internamente e escreve por I/O
                    # sobreposto, entao `write()` nunca bloqueia o laco de eventos — que era a
                    # restricao que obrigou aquele desenho la (um `time.sleep` no laco travava o
                    # backend inteiro). Reimplementar a fila aqui seria duas filas.
                    _diag_entrada(name, b)
                    transporte_escrita.write(b)
                elif (t := msg.get("text")) is not None:
                    try:
                        ctl = json.loads(t)
                        if isinstance(ctl, dict) and ctl.get("t") == "resize":
                            c = max(20, min(500, int(ctl["cols"])))
                            r = max(5, min(200, int(ctl["rows"])))
                            s.pty.redimensionar(c, r)
                    except (ValueError, TypeError, KeyError, OSError) as e:
                        if not avisou_frame_torto:
                            avisou_frame_torto = True
                            _log.warning("termsock: %r mandou quadro de controle invalido (%s): "
                                         "%.80r — descartado, aviso unico nesta conexao",
                                         name, e, t)
            except (WebSocketDisconnect, RuntimeError, ValueError, KeyError):
                return

    tarefa_leitor = asyncio.ensure_future(leitor_do_socket())
    try:
        await asyncio.wait({fim, tarefa_leitor, tarefa_escritor},
                           return_when=asyncio.FIRST_COMPLETED)
    finally:
        tarefa_leitor.cancel()
        if fim.done():
            # ConPTY morreu sozinho (o usuario deu `exit`): deixa o escritor escoar o que sobrou
            # antes de cancelar, senao a ultima tela se perde.
            with contextlib.suppress(BaseException):
                await asyncio.wait_for(tarefa_escritor, timeout=1.0)
        tarefa_escritor.cancel()
        await _colher_tarefas(name, tarefa_leitor, tarefa_escritor)
        if not s.desmontada:
            s.fechar_transportes()
        # Identidade, nao nome: um `pop(name)` cru removeria a Sessao NOVA de uma reconexao que ja
        # tomou o lugar desta.
        if _ativos.get(name) is s:
            del _ativos[name]
        await asyncio.to_thread(_desmontar_windows, s)
        try:
            await ws.close()
        except RuntimeError:
            pass
        _log.info("termsock: %r desanexado", name)


async def _colher_tarefas(name: str, *tarefas: asyncio.Future) -> None:
    """Recupera o resultado das tasks da conexao, logando so o que e falha de verdade.

    Cliente que fecha no meio de um `send_bytes` termina a task com `ClientDisconnected`: e o
    painel fechando, nao defeito, e virava ERROR com traceback a cada fechamento.
    """
    for tarefa in tarefas:
        try:
            await tarefa
        except (asyncio.CancelledError, WebSocketDisconnect, ClientDisconnected):
            pass
        except BaseException:
            _log.exception("termsock: %r — task terminou com excecao", name)


def _pipe_handle(h: int):
    """Embrulha um handle cru no `PipeHandle` que os transportes do Proactor esperam.

    Import tardio pelo mesmo motivo do `pty`/`fcntl` no motor POSIX (C1): `asyncio.windows_utils`
    so importa no Windows (puxa `_winapi` e `msvcrt`), e este modulo e importado tambem pelo
    caminho da guarda de 409, que roda nos dois sistemas.
    """
    from asyncio.windows_utils import PipeHandle
    return PipeHandle(h)
