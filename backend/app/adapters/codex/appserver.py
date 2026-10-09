"""Cliente JSON-RPC 2.0 pro ``codex app-server``.

O transporte default continua sendo NDJSON/stdio (util nos testes e como fallback). Para sessoes
visiveis no tmux, ``start_shared()`` abre um listener WebSocket somente em 127.0.0.1: o backend e a
TUI ``codex --remote`` conectam ao MESMO app-server, portanto a TUI fica anexavel sem trocar os
eventos estruturados por scraping de terminal."""
import asyncio
import collections
import contextlib
import json
import logging
import os
import socket
import uuid
from pathlib import Path
from typing import AsyncIterator

import websockets

from app import codex_contas
from app.adapters.codex.lancador import tool_output_override

logger = logging.getLogger(__name__)

# Limite de linha do StreamReader (default da stdlib e 64 KiB). Notifications do Codex podem
# carregar diffs grandes (item/fileChange/patchUpdated, item/commandExecution/outputDelta) que
# estouram 64 KiB -> LimitOverrunError. Dimensionado pra alguns MiB.
_READ_LIMIT = 8 * 1024 * 1024

# Teto de falhas de leitura SEGUIDAS antes de dar o cano por morto. Erro de leitura recuperavel
# consome a linha, entao alguns poucos ja cobrem o caso real; o teto e a rede contra o laco quente.
_MAX_FALHAS_LEITURA = 20


class RequestNotSent(ConnectionError):
    """A conexão estava indisponível antes de iniciar a escrita."""


class RequestRejected(RuntimeError):
    """O servidor respondeu ao pedido com uma recusa JSON-RPC."""


class RequestOutcomeUnknown(ConnectionError):
    """A escrita começou, mas a resposta do servidor não pôde ser confirmada."""


class AppServerClient:
    def __init__(self, codex_bin: str = "codex") -> None:
        self._codex_bin = codex_bin
        self._proc: asyncio.subprocess.Process | None = None
        self._reader: asyncio.StreamReader | None = None
        self._writer = None  # asyncio.StreamWriter (real) ou stub de teste com write/drain/close
        self._ws = None
        self._endpoint: str | None = None
        self._reader_task: asyncio.Task | None = None
        self._next_id = 0
        self._pending: dict[int | str, asyncio.Future] = {}
        self._notifications: asyncio.Queue = asyncio.Queue()
        self.server_requests: dict[int | str, dict] = {}
        self._respondendo: set[int | str] = set()
        # True quando o read loop encerrou (EOF do processo / close()). Deixa o adapter distinguir
        # "app-server morreu" de "sem mais notifications no momento" -> emite estado dead (Task 5).
        self._closed = False
        # Só quando o app-server roda atrás de um cano (sessão sem terminal): cauda do stderr e o
        # código de saída dele, pra dizer na tela por que a sessão morreu.
        self.stderr_tail: collections.deque[str] = collections.deque(maxlen=20)
        self.rc_cano: int | None = None
        self.runtime_acks = {}
        self.runtime_tickets = {}
        self.runtime_owner = None
        self.runtime_nonce = uuid.uuid4().hex

    @property
    def closed(self) -> bool:
        return self._closed

    @property
    def endpoint(self) -> str | None:
        return self._endpoint

    async def start(self, *, codex_home: str | Path | None = None,
                    tool_output_token_limit: int | None = None, cwd: str | None = None,
                    session_name: str | None = None, session_key: str | None = None) -> None:
        """Spawna `codex app-server --stdio` com stdin/stdout em PIPE e mantem o stdin aberto
        (nunca fechado ate close()) - fechar cedo faz o processo sair sem responder."""
        env = self._environment(codex_home, session_name=session_name, session_key=session_key)
        self._proc = await asyncio.create_subprocess_exec(
            self._codex_bin, "app-server", "--stdio",
            *tool_output_override(tool_output_token_limit),
            env=env, cwd=cwd,
            stderr=asyncio.subprocess.DEVNULL if tool_output_token_limit is not None else None,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            limit=_READ_LIMIT,
        )
        self._attach(self._proc.stdout, self._proc.stdin)

    @staticmethod
    def _free_loopback_endpoint() -> str:
        # Reserva e solta uma porta loopback. Existe uma janela minima ate o app-server dar bind,
        # fechada pelo retry abaixo; se outro processo vencer a corrida, o app-server sai e falhamos
        # sem deixar uma TUI apontando pro servidor errado.
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        return f"ws://127.0.0.1:{port}"

    @staticmethod
    def _environment(codex_home: str | Path | None, *, session_name: str | None = None,
                     session_key: str | None = None) -> dict:
        env = dict(os.environ)
        if codex_home is not None:
            path = Path(codex_home).expanduser().absolute()
            default = codex_contas.default_home().expanduser().absolute()
            account = codex_contas.Account("default", path, True) if path == default else \
                codex_contas.Account("selected", path, False)
            env = codex_contas.environment(account, base=env)
        if session_name is not None or session_key is not None:
            for key in ("CP_SESSION_NAME", "CP_SESSION_KEY", "TMUX", "TMUX_PANE", "HANGAR_CANO_KEY"):
                env.pop(key, None)
            if session_name is not None:
                env["CP_SESSION_NAME"] = session_name
            if session_key is not None:
                env["CP_SESSION_KEY"] = session_key
        return env

    async def start_shared(self, endpoint: str | None = None, *,
                           codex_home: str | Path | None = None,
                           tool_output_token_limit: int | None = None,
                           session_name: str | None = None, session_key: str | None = None) -> str:
        """Spawna app-server WebSocket local e conecta este cliente.

        O endpoint retornado pode ser passado a ``codex --remote`` dentro do tmux. stdout/stderr
        vao para DEVNULL: no modo WebSocket o protocolo nao passa por eles e pipes sem consumidor
        poderiam encher durante uma sessao longa.
        """
        self._endpoint = endpoint or self._free_loopback_endpoint()
        self._proc = await asyncio.create_subprocess_exec(
            self._codex_bin, "app-server", "--listen", self._endpoint,
            *tool_output_override(tool_output_token_limit),
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
            env=self._environment(codex_home, session_name=session_name, session_key=session_key),
        )
        last_error: Exception | None = None
        for _ in range(50):
            if self._proc.returncode is not None:
                raise RuntimeError(
                    f"codex app-server encerrou antes de abrir {self._endpoint}"
                )
            try:
                self._ws = await websockets.connect(
                    self._endpoint, max_size=None, open_timeout=1
                )
                self._reader_task = asyncio.create_task(self._read_loop())
                return self._endpoint
            except (OSError, TimeoutError) as exc:
                last_error = exc
                await asyncio.sleep(0.1)
        await self.close()
        raise RuntimeError(
            f"codex app-server nao abriu {self._endpoint}: {last_error}"
        )

    async def connect(self, endpoint: str, timeout: float = 5.0) -> str:
        """Conecta a um app-server que JA existe, sem spawnar nada.

        E o caminho normal desde que o lancador (scripts/hangar-codex-tui) passou a ser o dono do
        servidor: ele nasce no pane e morre com o pane, entao o backend so se liga nele. Falha ALTO
        quando o handshake nao responde — porta de loopback e reciclada, e um servidor que nao
        responde e sessao morta, nunca sessao viva a espera de outra tentativa.
        """
        try:
            self._ws = await websockets.connect(
                endpoint, max_size=None, open_timeout=timeout
            )
        except (OSError, TimeoutError, websockets.WebSocketException) as exc:
            raise ConnectionError(f"codex app-server nao respondeu em {endpoint}: {exc}") from exc
        self._endpoint = endpoint
        self._reader_task = asyncio.create_task(self._read_loop())
        return endpoint

    @property
    def tem_processo_proprio(self) -> bool:
        """False quando este cliente apenas se CONECTOU a um servidor de outro dono (o lancador).

        Quem encerra a sessao precisa saber disto: `close()`/`terminate()` daqui nao tem processo
        pra matar, e parar neles deixaria o app-server vivo. Quem mata e o registry, pelo pid
        gravado no sidecar.
        """
        return self._proc is not None

    def _attach(self, reader: asyncio.StreamReader, writer) -> None:
        # seam de teste: quem chama start() usa proc.stdout/stdin reais; os testes injetam
        # um StreamReader alimentado manualmente + um writer fake em memoria.
        self._reader = reader
        self._writer = writer
        self._reader_task = asyncio.create_task(self._read_loop())

    async def _read_loop(self) -> None:
        try:
            falhas_seguidas = 0
            while True:
                # LEITURA e PROCESSAMENTO em try separados, porque só o segundo pode seguir adiante:
                # transporte que caiu nao se recupera, e insistir nele nao le a proxima linha — re-le
                # a MESMA falha, sem suspender.
                try:
                    if self._ws is not None:
                        raw = await self._ws.recv()
                        if isinstance(raw, str):
                            raw = raw.encode()
                    else:
                        assert self._reader is not None
                        raw = await self._reader.readline()
                except asyncio.CancelledError:
                    raise  # cancel de close() - propaga, nao engole
                except websockets.ConnectionClosed as exc:
                    close = exc.sent or exc.rcvd
                    code = close.code if close is not None else 1006
                    logger.warning("codex app-server: WebSocket encerrado código=%s", code)
                    from app import diag
                    diag.registrar("codex.connection_closed", "aviso", codigo=str(code))
                    break
                except OSError as exc:
                    # Cano fechado no Windows chega como ConnectionResetError (WinError 64), nao como
                    # EOF. O StreamReader GUARDA a excecao e a relevanta em toda leitura seguinte sem
                    # jamais suspender: tratar isso como erro de linha e continuar vira laco quente
                    # que nunca cede o event loop (backend inteiro sem resposta) e nem aceita cancel,
                    # enquanto cada relevantada empilha frames no MESMO traceback e faz o
                    # logger.exception custar quadratico. Conexao caida e fim de loop, igual ao EOF.
                    logger.info("codex app-server: conexao encerrada na leitura (%s)", exc)
                    break
                except Exception:
                    # Recuperavel de verdade: readline() converte linha > _READ_LIMIT em ValueError
                    # DEPOIS de drenar o buffer, entao a proxima leitura anda. O teto existe porque
                    # a lista de erros que o StreamReader guarda pra sempre nao e fechada: se a
                    # proxima leitura nao andar, isto encerra em vez de girar a vazio.
                    falhas_seguidas += 1
                    logger.exception("codex app-server: erro lendo linha (%d seguidas)", falhas_seguidas)
                    if falhas_seguidas >= _MAX_FALHAS_LEITURA:
                        logger.error("codex app-server: leitura falhou %d vezes seguidas, encerrando",
                                     falhas_seguidas)
                        break
                    continue
                falhas_seguidas = 0
                if not raw:
                    break  # EOF - processo encerrou ou stream fechado
                # Erro daqui pra baixo e da MENSAGEM, nao do cano: json.loads pode falhar e o dispatch
                # pode ver JSON valido nao-objeto. Nenhum desses pode matar a reader task (senao
                # requests futuras so destravam por timeout e close() fica com subprocess orfao).
                try:
                    raw = raw.strip()
                    if not raw:
                        continue
                    msg = json.loads(raw)
                    if not isinstance(msg, dict):
                        continue  # JSON valido mas nao-objeto (ex: "42", "[]") - ignora
                    if msg.get("type") == "cano_output":
                        msg = json.loads(msg["frame"])
                        if not isinstance(msg, dict):
                            continue
                    tipo_cano = msg.get("type")
                    if tipo_cano == "cano_input_ack":
                        from app.runtime_adapter import accept_ack
                        accept_ack(self, msg)
                        continue
                    if tipo_cano == "cano_stderr":
                        self.stderr_tail.append(str(msg.get("linha", "")))
                        continue
                    if tipo_cano == "cano_saiu":
                        self.rc_cano = msg.get("rc")
                        break  # o app-server atrás do cano saiu: mesmo fim que o EOF
                    msg_id = msg.get("id")
                    if "method" in msg:
                        # Os dois lados numeram pedidos independentemente: o método distingue
                        # pedido do servidor de resposta ao cliente, mesmo com IDs iguais.
                        if msg_id is not None:
                            self.server_requests[msg_id] = msg
                        elif msg["method"] == "serverRequest/resolved":
                            resolved = (msg.get("params") or {}).get("requestId")
                            self.server_requests.pop(resolved, None)
                            self._respondendo.discard(resolved)
                        elif msg["method"] == "turn/completed":
                            params = msg.get("params") or {}
                            for rid, pedido in list(self.server_requests.items()):
                                if (pedido.get("params") or {}).get("threadId") == params.get("threadId"):
                                    self.server_requests.pop(rid, None)
                                    self._respondendo.discard(rid)
                        await self._notifications.put(msg)
                    elif msg_id is not None:
                        if type(msg_id) not in (int, str):
                            raise ValueError("ID RPC inválido")
                        if self.runtime_owner is not None:
                            from app import runtime_coordinator
                            from app.runtime_adapter import LegacyIO
                            coordinator = runtime_coordinator.current()
                            await LegacyIO(coordinator).reply(self.runtime_owner[0], self, msg)
                        # Resposta de request: casa o Future pendente. Se o id nao tem Future
                        # (resposta tardia de request que ja deu timeout), dropa com warning -
                        # NAO enfileira em notifications (resposta nao tem `method`, poluiria a
                        # fila e quebraria consumidor que faz msg["method"]).
                        fut = self._pending.pop(msg_id, None)
                        if fut is not None:
                            if not fut.done():
                                fut.set_result(msg)
                        else:
                            logger.warning("codex app-server: resposta orfa id=%r (request ja expirou?)", msg_id)
                    else:
                        logger.warning("codex app-server: mensagem sem id e sem method, ignorada: %.200r", raw)
                except asyncio.CancelledError:
                    raise  # cancel de close() - propaga, nao engole
                except Exception:
                    logger.exception("codex app-server: erro processando linha, seguindo")
                    continue
        finally:
            # conexao encerrou (EOF ou cancel de close()) - nenhuma request pendente pode
            # ficar orfa esperando um Future que nunca vai resolver.
            for fut in self._pending.values():
                if not fut.done():
                    fut.set_exception(ConnectionError("codex app-server: conexao encerrada"))
            self._pending.clear()
            # Sinaliza morte: marca fechado e empurra um sentinela None pra fila -> notifications()
            # termina o async-for e o adapter emite dead (em vez de bloquear pra sempre num get()).
            self._closed = True
            self.server_requests.clear()
            self._respondendo.clear()
            self._notifications.put_nowait(None)

    async def respond(self, request_id: int | str, result: dict | None, *, erro: dict | None = None) -> None:
        """Responde uma vez ao pedido nativo; o servidor publica a resolução para todos.
        `erro` responde com erro JSON-RPC (método que este cliente não atende) em vez de result."""
        if type(request_id) not in (int, str) or self.closed or request_id not in self.server_requests:
            raise ValueError("A pergunta já foi respondida ou cancelada.")
        if request_id in self._respondendo:
            raise ValueError("A resposta desta pergunta já está sendo enviada.")
        self._respondendo.add(request_id)
        corpo = {"error": erro} if erro is not None else {"result": result}
        line = json.dumps({"jsonrpc": "2.0", "id": request_id, **corpo})
        try:
            if self._ws is not None:
                await self._ws.send(line)
            else:
                await self._send_frame(json.loads(line))
        except Exception:
            self._respondendo.discard(request_id)
            raise

    def _wire_id(self, n: int) -> int | str:
        # Cliente da reserva do runtime: o id carrega dono e geração, e a resposta volta por ele.
        if self.runtime_owner is None:
            return n
        _name, key, generation = self.runtime_owner
        return f"reserve:{key}:{generation}:{self.runtime_nonce}:{n}"

    def request_bytes(self, method: str, params: dict) -> bytes:
        """Mesmo envelope da próxima chamada, inclusive id e escapes do transporte."""
        return (json.dumps({"jsonrpc": "2.0", "id": self._wire_id(self._next_id + 1),
                            "method": method, "params": params}) + "\n").encode()

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        if self._closed or (self._writer is None and self._ws is None):
            raise RequestNotSent("O app-server está sem conexão; nenhum pedido foi escrito")
        wire = self.request_bytes(method, params)
        self._next_id += 1
        req_id = self._wire_id(self._next_id)
        fut = asyncio.get_running_loop().create_future()
        self._pending[req_id] = fut
        try:
            if self._ws is not None:
                await self._ws.send(wire.decode().rstrip("\n"))
            else:
                await self._send_frame(json.loads(wire))
            msg = await asyncio.wait_for(fut, timeout=timeout)
        except (ConnectionError, OSError, TimeoutError, websockets.ConnectionClosed) as exc:
            # A perda da resposta não prova que o servidor deixou de receber o pedido.
            raise RequestOutcomeUnknown(f"Resultado de {method} incerto; não reenviar automaticamente") from exc
        finally:
            self._pending.pop(req_id, None)
            if not fut.done():
                fut.cancel()
            elif not fut.cancelled():
                fut.exception()
        if "error" in msg:
            raise RequestRejected(f"codex app-server error em '{method}': {msg['error']}")
        return msg.get("result", {})

    async def _send_frame(self, frame: dict) -> None:
        if self.runtime_owner is not None:
            from app import runtime_coordinator
            from app.runtime_adapter import LegacyIO, assert_legacy
            name, key, generation = self.runtime_owner
            coordinator = runtime_coordinator.current()
            if coordinator is None:
                raise RuntimeError("cliente gerenciado sem responsável")
            binding = coordinator.slot(name).binding
            if binding.key != key or binding.generation != generation:
                raise RuntimeError("cliente de outra geração")
            assert_legacy(name)
            await LegacyIO(coordinator).write(name, self, self._writer, frame, self.cano_snapshot["versao"])
        else:
            self._writer.write((json.dumps(frame) + "\n").encode())
            await self._writer.drain()

    async def notifications(self) -> AsyncIterator[dict]:
        while True:
            item = await self._notifications.get()
            if item is None:
                return  # sentinela de EOF (processo morreu / close()): encerra o stream
            yield item

    def terminate(self) -> None:
        """Best-effort SIGTERM SINCRONO no subprocess -- seguro de chamar de outra thread (so manda
        o sinal, nao toca o event loop). Usado pelo registry.kill() (sync) sem precisar de bridge
        async: o read loop no loop principal vai ver o EOF e rodar seu finally (dead-detection).

        Quando o servidor e do LANCADOR, aqui nao ha o que matar — e isso e dito em voz alta, nao
        engolido: quem encerra a sessao precisa saber que o SIGTERM tem que ir pelo pid do sidecar
        (adapter.matar_app_server), senao o app-server fica escutando sem dono."""
        if not self.tem_processo_proprio:
            logger.debug("codex app-server: nada a terminar aqui — o processo e do lancador, "
                         "o SIGTERM vai pelo pid do sidecar")
            return
        with contextlib.suppress(ProcessLookupError, Exception):
            self._proc.terminate()

    async def close(self, *, strict: bool = False) -> None:
        if self._reader_task is not None:
            self._reader_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._reader_task
            self._reader_task = None
        if self._ws is not None:
            with contextlib.suppress(Exception):
                await self._ws.close()
            self._ws = None
        if self._writer is not None:
            self._writer.close()
            if strict:
                await self._writer.wait_closed()
            else:
                with contextlib.suppress(Exception):
                    await self._writer.wait_closed()
            self._writer = None
        if self._proc is not None:
            with contextlib.suppress(ProcessLookupError):
                self._proc.terminate()
            try:
                await asyncio.wait_for(self._proc.wait(), timeout=5.0)
            except TimeoutError:
                with contextlib.suppress(ProcessLookupError):
                    self._proc.kill()
                await asyncio.wait_for(self._proc.wait(), timeout=5.0)
            self._proc = None
        self._endpoint = None
