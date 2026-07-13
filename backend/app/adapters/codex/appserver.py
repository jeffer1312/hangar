"""Cliente JSON-RPC 2.0 pro `codex app-server --stdio`: transporte NDJSON (uma linha JSON
por mensagem, SEM framing Content-Length do LSP), correlacao request<->response por `id` e
fila de notifications (mensagens sem `id`). Contrato confirmado contra codex-cli 0.141.0 em
docs/codex-app-server-contract.md.

Achado critico do spike: o processo so responde se o stdin ficar aberto durante toda a vida
da sessao - `cat arquivo | codex app-server --stdio` sai sem responder nada. Por isso o pipe
de stdin do subprocess nunca e fechado ate `close()`."""
import asyncio
import contextlib
import json
import logging
from typing import AsyncIterator

logger = logging.getLogger(__name__)

# Limite de linha do StreamReader (default da stdlib e 64 KiB). Notifications do Codex podem
# carregar diffs grandes (item/fileChange/patchUpdated, item/commandExecution/outputDelta) que
# estouram 64 KiB -> LimitOverrunError. Dimensionado pra alguns MiB.
_READ_LIMIT = 8 * 1024 * 1024


class AppServerClient:
    def __init__(self, codex_bin: str = "codex") -> None:
        self._codex_bin = codex_bin
        self._proc: asyncio.subprocess.Process | None = None
        self._reader: asyncio.StreamReader | None = None
        self._writer = None  # asyncio.StreamWriter (real) ou stub de teste com write/drain/close
        self._reader_task: asyncio.Task | None = None
        self._next_id = 0
        self._pending: dict[int, asyncio.Future] = {}
        self._notifications: asyncio.Queue = asyncio.Queue()
        # True quando o read loop encerrou (EOF do processo / close()). Deixa o adapter distinguir
        # "app-server morreu" de "sem mais notifications no momento" -> emite estado dead (Task 5).
        self._closed = False

    @property
    def closed(self) -> bool:
        return self._closed

    async def start(self) -> None:
        """Spawna `codex app-server --stdio` com stdin/stdout em PIPE e mantem o stdin aberto
        (nunca fechado ate close()) - fechar cedo faz o processo sair sem responder."""
        self._proc = await asyncio.create_subprocess_exec(
            self._codex_bin, "app-server", "--stdio",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            limit=_READ_LIMIT,
        )
        self._attach(self._proc.stdout, self._proc.stdin)

    def _attach(self, reader: asyncio.StreamReader, writer) -> None:
        # seam de teste: quem chama start() usa proc.stdout/stdin reais; os testes injetam
        # um StreamReader alimentado manualmente + um writer fake em memoria.
        self._reader = reader
        self._writer = writer
        self._reader_task = asyncio.create_task(self._read_loop())

    async def _read_loop(self) -> None:
        assert self._reader is not None
        try:
            while True:
                # Corpo inteiro do loop protegido: readline() pode levantar LimitOverrunError
                # (linha > _READ_LIMIT), json.loads erros, e o dispatch pode ver JSON valido
                # nao-objeto. Nenhum desses pode matar a reader task (senao requests futuras so
                # destravam por timeout e close() fica com subprocess orfao). CancelledError
                # (cancel de close()) DEVE continuar propagando -> por isso except Exception, nao
                # BaseException.
                try:
                    raw = await self._reader.readline()
                    if not raw:
                        break  # EOF - processo encerrou ou stream fechado
                    raw = raw.strip()
                    if not raw:
                        continue
                    msg = json.loads(raw)
                    if not isinstance(msg, dict):
                        continue  # JSON valido mas nao-objeto (ex: "42", "[]") - ignora
                    msg_id = msg.get("id")
                    if msg_id is not None:
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
                    elif "method" in msg:
                        await self._notifications.put(msg)  # notification legitima
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
            self._notifications.put_nowait(None)

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        if self._writer is None:
            raise RuntimeError("AppServerClient.start() precisa rodar antes de request()")
        self._next_id += 1
        req_id = self._next_id
        fut = asyncio.get_running_loop().create_future()
        self._pending[req_id] = fut
        line = json.dumps({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params})
        self._writer.write((line + "\n").encode())
        await self._writer.drain()
        try:
            msg = await asyncio.wait_for(fut, timeout=timeout)
        finally:
            self._pending.pop(req_id, None)
        if "error" in msg:
            raise RuntimeError(f"codex app-server error em '{method}': {msg['error']}")
        return msg.get("result", {})

    async def notifications(self) -> AsyncIterator[dict]:
        while True:
            item = await self._notifications.get()
            if item is None:
                return  # sentinela de EOF (processo morreu / close()): encerra o stream
            yield item

    def terminate(self) -> None:
        """Best-effort SIGTERM SINCRONO no subprocess -- seguro de chamar de outra thread (so manda
        o sinal, nao toca o event loop). Usado pelo registry.kill() (sync) sem precisar de bridge
        async: o read loop no loop principal vai ver o EOF e rodar seu finally (dead-detection)."""
        proc = self._proc
        if proc is not None:
            with contextlib.suppress(ProcessLookupError, Exception):
                proc.terminate()

    async def close(self) -> None:
        if self._reader_task is not None:
            self._reader_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._reader_task
            self._reader_task = None
        if self._writer is not None:
            self._writer.close()
            with contextlib.suppress(Exception):
                await self._writer.wait_closed()
            self._writer = None
        if self._proc is not None:
            with contextlib.suppress(ProcessLookupError):
                self._proc.terminate()
            await self._proc.wait()
            self._proc = None
