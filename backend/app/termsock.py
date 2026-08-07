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
import json
import logging
import os
import secrets
import signal
import struct
import threading
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


async def term_ws(ws: WebSocket, name: str) -> None:
    loop = asyncio.get_running_loop()
    host = ws.client.host if ws.client else ""
    agora = time.time()
    tok = ws.query_params.get("token", "")
    if _blocked(host, agora):
        await ws.close(code=1008)
        return                                   # NAO registra: registrar aqui estende o bloqueio
    # compare_digest, nao `!=` de string (mesmo cuidado do require_auth, auth.py:104): `!=` sai fora
    # na primeira letra diferente e vira canal lateral de tempo. Este e o endpoint que abre um shell
    # completo. `.encode()` tambem evita o TypeError do compare_digest com string nao-ASCII.
    if not settings.auth_token or not secrets.compare_digest(tok.encode(),
                                                             settings.auth_token.encode()):
        if host not in _LOOPBACK:                # mesma isencao do require_auth (auth.py:46)
            _record_fail(host, agora)
        await ws.close(code=1008)                # fecha SEM accept: o PTY nunca chega a nascer
        return
    origem = ws.headers.get("origin")
    if origem and settings.public_url and origem.rstrip("/") != settings.public_url.rstrip("/"):
        # WebSocket nao e coberto por CORS: sem isto, uma pagina qualquer aberta no navegador do
        # dono poderia abrir um shell usando o cookie/token dele. Igualdade normalizada, nao
        # prefixo: Origin nunca tem path, e um `startswith` deixava passar
        # "https://<public_url>.evil.com" (achado da revisao).
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

    async def leitor_do_socket():
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
                    with contextlib.suppress(ValueError, TypeError, KeyError):
                        ctl = json.loads(t)
                        if isinstance(ctl, dict) and ctl.get("t") == "resize":
                            # Mesmo clamp do connect (:cols/:rows acima) — sem ele, {"cols":99999}
                            # ou negativo estoura o struct.pack de _winsize.
                            c = max(20, min(500, int(ctl["cols"])))
                            r = max(5, min(200, int(ctl["rows"])))
                            _winsize(s.master, c, r)
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
        for tarefa in (tarefa_leitor, tarefa_escritor):
            with contextlib.suppress(BaseException):
                await tarefa
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
