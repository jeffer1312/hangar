"""Importação pelo protocolo oficial do Codex, sem abrir uma sessão de agente."""

import asyncio
import contextlib
import functools
import hashlib
import json
import logging
import os
from pathlib import Path
import shutil
import signal
import sys
import tempfile
import time
from typing import TYPE_CHECKING
import uuid

from app.atomico import substituir
from app import account_lifecycle, diag, log_paths

if TYPE_CHECKING:
    from app.codex_contas import Account


_log = logging.getLogger("hangar.codex.importador")
_READ_LIMIT = 8 * 1024 * 1024
_COMPLETED = "externalAgentConfig/import/completed"
_AUTO_UPGRADE_EM_CURSO = "auto-upgrade was in flight"
_ADMIN_CONFIG = ("-c", "project_root_markers=[]")
_CREATE_SUSPENDED = 0x00000004
# Depois de encerrar o Job, prazo para o Windows desmontar os processos e soltar os handles.
_TREE_KILL_TIMEOUT = 10.0


class CodexNativoErro(RuntimeError):
    """Falha do processo ou do protocolo nativo, sem expor configurações sensíveis.

    `data` pode carregar a cauda do stderr (URL com token, por exemplo): é pra decisão interna,
    nunca pra resposta HTTP ou tela."""

    def __init__(self, message: str, *, code: int | None = None, data: dict | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.data = data


class CodexAusente(CodexNativoErro):
    """O executável do Codex não existe nesta máquina — estado, não falha do processo."""


class _ProcessHandle:
    """Handle próprio no formato que `WindowsJob.assign` lê."""

    def __init__(self, handle: int) -> None:
        self._handle = handle


class _OwnTree:
    """Job próprio do Windows: o Codex nasce suspenso dentro dele, e todo descendente herda.

    Um filho do Codex (git, por exemplo) segura o cwd depois de o líder sair; sem esperar a
    árvore inteira, a pasta não sai e o processo segue vivo."""

    _SYNCHRONIZE = 0x00100000
    _QUERY_LIMITED = 0x1000
    _SET_QUOTA_TERMINATE = 0x0100 | 0x0001
    _WAIT_TIMEOUT = 0x00000102
    _WAIT_FAILED = 0xFFFFFFFF
    _GONE = 87

    def __init__(self) -> None:
        import ctypes
        from ctypes import wintypes

        from app.runtime_process import WindowsJob

        self._ctypes = ctypes
        api = ctypes.WinDLL("kernel32", use_last_error=True)
        for name, args, result in (
            ("OpenProcess", [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE),
            ("IsProcessInJob", [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)], wintypes.BOOL),
            ("WaitForSingleObject", [wintypes.HANDLE, wintypes.DWORD], wintypes.DWORD),
            ("CloseHandle", [wintypes.HANDLE], wintypes.BOOL),
        ):
            function = getattr(api, name)
            function.argtypes, function.restype = args, result
        self._api = api
        self._bool = wintypes.BOOL
        self.job = WindowsJob(f"Local\\Hangar-codex-{uuid.uuid4().hex}")
        # O Job tira o processo da lista e da contagem antes de ele soltar os handles (o cwd,
        # inclusive): só o handle do próprio processo, aberto enquanto ele era membro, diz quando
        # ele terminou de sair.
        self._handles: dict[int, int] = {}

    def _error(self) -> OSError:
        return self._ctypes.WinError(self._ctypes.get_last_error())

    def admit(self, pid: int) -> None:
        """Atribui o processo suspenso ao Job e só então o deixa rodar."""
        import psutil

        handle = self._api.OpenProcess(self._SET_QUOTA_TERMINATE, False, pid)
        if not handle:
            raise self._error()
        try:
            self.job.assign(_ProcessHandle(handle))
        finally:
            self._api.CloseHandle(handle)
        psutil.Process(pid).resume()

    def _track(self) -> list[int]:
        """Membros atuais, com o handle de cada um guardado até o fim da contenção."""
        pids = self.job.members()
        for pid in pids:
            if pid in self._handles:
                continue
            handle = self._api.OpenProcess(self._SYNCHRONIZE | self._QUERY_LIMITED, False, pid)
            if not handle:
                if self._ctypes.get_last_error() == self._GONE:
                    continue
                raise self._error()
            inside = self._bool()
            if not self._api.IsProcessInJob(handle, self.job.handle, self._ctypes.byref(inside)):
                error = self._error()
                self._api.CloseHandle(handle)
                raise error
            if not inside:
                # O PID saiu do Job e foi reaproveitado por outro processo.
                self._api.CloseHandle(handle)
                continue
            self._handles[pid] = handle
        return pids

    def _wait_empty(self, timeout: float) -> bool:
        """Espera pelos handles até o Job ficar sem membros e cada processo visto terminar de sair."""
        deadline = time.monotonic() + timeout
        while True:
            pids = self._track()
            for handle in tuple(self._handles.values()):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                result = self._api.WaitForSingleObject(handle, int(remaining * 1000))
                if result == self._WAIT_TIMEOUT:
                    return False
                if result == self._WAIT_FAILED:
                    raise self._error()
            if not pids and not self.job.active():
                return True
            if time.monotonic() >= deadline:
                return False

    def close(self) -> None:
        while self._handles:
            self._api.CloseHandle(self._handles.popitem()[1])
        self.job.close()

    async def release(self, graceful: float) -> None:
        """Dá o prazo de saída normal, encerra o Job se ainda houver membros e confirma o fim.

        Só fecha o Job com zero membros: falha mantém a contenção para uma nova tentativa."""
        try:
            if not await asyncio.to_thread(self._wait_empty, graceful):
                self._track()
                self.job.terminate()
                if not await asyncio.to_thread(self._wait_empty, _TREE_KILL_TIMEOUT):
                    raise CodexNativoErro("Processos do Codex continuam ativos após encerrar a contenção.")
            self.close()
        except OSError as exc:
            raise CodexNativoErro("Não foi possível consultar ou encerrar a contenção do Codex.") from exc


class _OwnGroup:
    """No POSIX, o grupo de processos próprio faz o papel do Job: o Codex deixa filhos em segundo plano
    (o clone dos plugins curados) que seguram e escrevem na pasta temporária depois de ele sair."""

    def __init__(self, pgid: int) -> None:
        self.pgid = pgid

    def _empty(self) -> bool:
        # Zumbi ainda não recolhido conta para o killpg(pgid, 0), mas já parou: no Linux só os vivos contam.
        if sys.platform.startswith("linux"):
            return not any(self._alive_in_group(entry.name) for entry in os.scandir("/proc") if entry.name.isdigit())
        try:
            os.killpg(self.pgid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        return False

    def _alive_in_group(self, pid: str) -> bool:
        try:
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            return False
        return fields[2] == str(self.pgid) and fields[0] != "Z"

    async def _wait_empty(self, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while not self._empty():
            if time.monotonic() >= deadline:
                return False
            await asyncio.sleep(0.02)
        return True

    def close(self) -> None:
        return None

    async def release(self, graceful: float) -> None:
        """O mesmo contrato do Job: prazo de saída normal, depois o grupo inteiro encerrado e o fim confirmado."""
        if await self._wait_empty(graceful):
            return
        with contextlib.suppress(ProcessLookupError):
            os.killpg(self.pgid, signal.SIGKILL)
        if not await self._wait_empty(_TREE_KILL_TIMEOUT):
            raise CodexNativoErro("Processos do Codex continuam ativos após encerrar o grupo.")


class _CliOperation:
    """Um comando CLI admitido: recursos registrados antes do nascimento e retidos até o fim confirmado."""

    def __init__(self, work_dir: str) -> None:
        self.work_dir = work_dir
        self.proc: asyncio.subprocess.Process | None = None
        self.tree: "_OwnTree | _OwnGroup | None" = None
        self.spawned = asyncio.Event()
        self.cleanup: asyncio.Future | None = None


class CodexNativo:
    def __init__(
        self, home: Path, codex_home: Path, binario: str = "codex", *,
        timeout: float = 120.0, close_timeout: float = 3.0,
        account: "Account | None" = None, memoria: bool = False,
    ) -> None:
        self.home = home.absolute()
        self.codex_home = codex_home.absolute()
        # Por linha de comando, não no config.toml do stage: o arquivo é lido de volta como
        # resultado da importação nativa, e uma chave nossa ali viraria diferença a conciliar.
        self.memoria = memoria
        self.account = account
        self.binario = binario
        self.timeout = timeout
        self.close_timeout = close_timeout
        self._proc: asyncio.subprocess.Process | None = None
        self._tree: "_OwnTree | _OwnGroup | None" = None
        self._reader_task: asyncio.Task | None = None
        self._work_dir: str | None = None
        self._closing: asyncio.Future | None = None
        self._operations: set[_CliOperation] = set()
        self._accepting = True
        self._pending: dict[int, asyncio.Future] = {}
        self._next_id = 0
        self._closed = True
        self._import_lock = asyncio.Lock()
        self._completions: asyncio.Queue | None = None
        self._listeners: dict[str, set[asyncio.Queue]] = {}

    def _env(self) -> dict[str, str]:
        if self.account is not None:
            from app.codex_contas import environment
            return environment(self.account, home=self.home)
        env = {**os.environ, "HOME": str(self.home), "USERPROFILE": str(self.home),
               "CODEX_HOME": str(self.codex_home)}
        if self.home != Path.home().absolute():
            # Diretórios XDG herdados também podem apontar para os dados reais do usuário.
            env.update({
                "XDG_CONFIG_HOME": str(self.home / ".config"),
                "XDG_DATA_HOME": str(self.home / ".local" / "share"),
                "XDG_STATE_HOME": str(self.home / ".local" / "state"),
                "XDG_CACHE_HOME": str(self.home / ".cache"),
            })
        return env

    def _config_memoria(self) -> tuple[str, ...]:
        if not self.memoria:
            return ()
        return ("-c", "features.external_agent_memory_import=true")

    def _comando(self) -> list[str]:
        caminho = shutil.which(self.binario)
        if not caminho:
            raise CodexAusente("Codex CLI não encontrado; instale ou configure o executável.")
        if os.name != "nt" or Path(caminho).suffix.lower() not in {".cmd", ".bat"}:
            return [caminho]
        # O shim npm exige cmd.exe; chamar seu JS por Node evita interpretação de argumentos.
        pasta = Path(caminho).parent
        executavel = pasta / "codex.exe"
        if executavel.is_file():
            return [str(executavel)]
        script = pasta / "node_modules" / "@openai" / "codex" / "bin" / "codex.js"
        node = str(pasta / "node.exe") if (pasta / "node.exe").is_file() else shutil.which("node")
        if script.is_file() and node:
            return [node, str(script)]
        raise CodexNativoErro("Não foi possível resolver o executável nativo do Codex no Windows.")

    async def _spawn(self, args: list[str], cwd: str, **pipes) -> tuple[asyncio.subprocess.Process, "_OwnTree | _OwnGroup"]:
        """No Windows, o Job existe antes do processo, e o processo só roda depois de entrar nele."""
        command = self._comando()
        if os.name != "nt":
            proc = await asyncio.create_subprocess_exec(
                *command, *args, cwd=cwd, env=self._env(), start_new_session=True, **pipes)
            return proc, _OwnGroup(proc.pid)
        try:
            tree = _OwnTree()
        except OSError as exc:
            raise CodexNativoErro("Não foi possível criar a contenção do Codex.") from exc
        try:
            proc = await asyncio.create_subprocess_exec(
                *command, *args, cwd=cwd, env=self._env(), creationflags=_CREATE_SUSPENDED, **pipes)
        except BaseException:
            tree.close()
            raise
        try:
            tree.admit(proc.pid)
        except BaseException as exc:
            # Ainda suspenso, sem filhos: encerra e aguarda o próprio processo antes de falhar.
            try:
                with contextlib.suppress(ProcessLookupError):
                    proc.kill()
                await account_lifecycle.complete_on_cancel(asyncio.wait_for(proc.wait(), _TREE_KILL_TIMEOUT))
            finally:
                tree.close()
            if isinstance(exc, Exception):
                raise CodexNativoErro("Não foi possível colocar o Codex na contenção própria.") from exc
            raise
        return proc, tree

    async def __aenter__(self) -> "CodexNativo":
        if self._proc is not None or self._work_dir is not None:
            raise CodexNativoErro("O cliente nativo do Codex já está aberto.")
        self._closing = None
        self._accepting = True
        try:
            # Administração da conta não deve carregar configuração de nenhum projeto.
            self._work_dir = tempfile.mkdtemp(prefix="hangar-codex-admin-")
            self._proc, self._tree = await self._spawn(
                [*_ADMIN_CONFIG, *self._config_memoria(), "app-server", "--stdio"], self._work_dir,
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL, limit=_READ_LIMIT,
            )
            self._closed = False
            self._reader_task = asyncio.create_task(self._read_loop())
            await self.request("initialize", {
                "clientInfo": {"name": "hangar", "version": "0.1.0"},
                "capabilities": {"experimentalApi": True},
            })
            await self._send({"jsonrpc": "2.0", "method": "initialized"})
            return self
        except BaseException:
            await self.close()
            raise

    async def __aexit__(self, exc_type, exc_value, traceback) -> None:
        await self.close()

    async def _send(self, message: dict) -> None:
        if self._closed or self._proc is None or self._proc.stdin is None:
            raise CodexNativoErro("A conexão com o importador nativo do Codex está encerrada.")
        self._proc.stdin.write((json.dumps(message) + "\n").encode())
        await self._proc.stdin.drain()

    def _disconnected(self) -> None:
        self._closed = True
        for future in self._pending.values():
            if not future.done():
                future.set_exception(CodexNativoErro("O importador nativo do Codex encerrou a conexão."))
        if self._completions is not None:
            self._completions.put_nowait(None)
        for queues in self._listeners.values():
            for queue in queues:
                queue.put_nowait(None)
        self._listeners.clear()

    def subscribe(self, method: str) -> asyncio.Queue:
        queue: asyncio.Queue = asyncio.Queue()
        self._listeners.setdefault(method, set()).add(queue)
        return queue

    def unsubscribe(self, method: str, queue: asyncio.Queue) -> None:
        listeners = self._listeners.get(method)
        if listeners is None:
            return
        listeners.discard(queue)
        if not listeners:
            self._listeners.pop(method, None)

    def _dispatch(self, message: dict) -> None:
        # JSON-RPC server messages are identified by `method`; a server request may also carry
        # `id`, so checking id first would resolve a client Future with an unrelated notification.
        method = message.get("method")
        if "method" in message:
            if not isinstance(method, str):
                return
            if method == _COMPLETED and self._completions is not None:
                params = message.get("params")
                if isinstance(params, dict):
                    self._completions.put_nowait(params)
            for queue in tuple(self._listeners.get(method, ())):
                queue.put_nowait(message)
            return
        if "id" in message:
            req_id = message["id"]
            future = self._pending.get(req_id) if isinstance(req_id, int) else None
            if future is not None and not future.done():
                future.set_result(message)

    async def _read_loop(self) -> None:
        assert self._proc is not None and self._proc.stdout is not None
        try:
            while raw := await self._proc.stdout.readline():
                if not raw.strip():
                    continue
                message = json.loads(raw)
                if not isinstance(message, dict):
                    continue
                self._dispatch(message)
        except (OSError, ValueError):
            # Conteúdo de stdout pode incluir segredos; a falha publicada é somente de conexão.
            pass
        finally:
            self._disconnected()

    async def request(self, method: str, params: dict | None, timeout: float | None = None) -> dict:
        if self._closed:
            raise CodexNativoErro("Abra o cliente nativo do Codex antes de fazer uma requisição.")
        self._next_id += 1
        req_id = self._next_id
        future = asyncio.get_running_loop().create_future()
        self._pending[req_id] = future
        try:
            async with asyncio.timeout(self.timeout if timeout is None else timeout):
                await self._send({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params})
                message = await future
        except TimeoutError:
            raise CodexNativoErro("O Codex excedeu o tempo limite da requisição nativa.") from None
        finally:
            self._pending.pop(req_id, None)
            if not future.done():
                future.cancel()
        if "error" in message:
            error = message["error"]
            code = error.get("code") if isinstance(error, dict) else None
            code = code if isinstance(code, int) else None
            data = error.get("data") if isinstance(error, dict) else None
            # Só o discriminador conhecido é público; o resto pode conter o config.toml.
            safe_data = None
            if isinstance(data, dict) and data.get("config_write_error_code") == "configLayerReadonly":
                safe_data = {"config_write_error_code": "configLayerReadonly"}
            # Login revogado do lado da OpenAI só aparece no texto: "... unauthorized (401)".
            text = error.get("message") if isinstance(error, dict) else None
            if isinstance(text, str) and "unauthorized (401)" in text.lower():
                safe_data = {**(safe_data or {}), "auth_error": "unauthorized"}
            if code == -32601:
                raise CodexNativoErro(
                    "Esta versão do Codex não oferece a API necessária; atualize o Codex CLI.",
                    code=code, data=safe_data,
                )
            raise CodexNativoErro("O Codex recusou a requisição nativa.", code=code, data=safe_data)
        result = message.get("result")
        if not isinstance(result, dict):
            raise CodexNativoErro("O Codex retornou uma resposta nativa inválida.")
        return result

    async def detectar(self) -> list[dict]:
        result = await self.request("externalAgentConfig/detect", {
            "includeHome": True, "cwds": [], "maxSessions": 0,
        })
        if any(result.get(key) for key in ("sourceErrors", "errors", "warnings")):
            raise CodexNativoErro("O Codex informou falhas ou avisos na detecção; fontes existentes foram preservadas.")
        items = result.get("items")
        if not isinstance(items, list) or any(not isinstance(item, dict) for item in items):
            raise CodexNativoErro("O Codex retornou uma detecção inválida.")
        return items

    async def historicos_importacao(self) -> list[dict]:
        result = await self.request("externalAgentConfig/import/readHistories", None)
        historicos = result.get("data")
        if not isinstance(historicos, list) or any(not isinstance(item, dict) for item in historicos):
            raise CodexNativoErro("O Codex retornou um histórico de importação inválido.")
        return historicos

    async def importar(self, items: list[dict]) -> dict:
        async with self._import_lock:
            # A conclusão pode chegar antes da resposta que informa o importId.
            self._completions = asyncio.Queue()
            try:
                async with asyncio.timeout(self.timeout):
                    response = await self.request("externalAgentConfig/import", {
                        "migrationItems": items, "source": "hangar",
                    })
                    import_id = response.get("importId")
                    if not isinstance(import_id, str) or not import_id:
                        raise CodexNativoErro("O Codex não retornou o identificador da importação.")
                    while True:
                        completed = await self._completions.get()
                        if completed is None:
                            raise CodexNativoErro("O Codex encerrou antes de concluir a importação.")
                        if completed.get("importId") == import_id:
                            if not isinstance(completed.get("itemTypeResults"), list):
                                raise CodexNativoErro("O Codex retornou uma conclusão inválida.")
                            return completed
            except TimeoutError:
                raise CodexNativoErro("O Codex excedeu o tempo limite para concluir a importação.") from None
            finally:
                self._completions = None

    async def _stop(self, proc: asyncio.subprocess.Process) -> None:
        if proc.returncode is not None:
            return
        if proc.stdin is not None:
            proc.stdin.close()
        try:
            await asyncio.wait_for(proc.wait(), self.close_timeout)
            return
        except TimeoutError:
            pass
        with contextlib.suppress(ProcessLookupError):
            proc.terminate()
        try:
            await asyncio.wait_for(proc.wait(), self.close_timeout)
        except TimeoutError:
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
            await asyncio.wait_for(proc.wait(), self.close_timeout)

    async def _finish(self, proc: asyncio.subprocess.Process | None, tree: "_OwnTree | _OwnGroup | None") -> None:
        if proc is not None:
            await self._stop(proc)
        if tree is not None:
            await tree.release(self.close_timeout)

    @staticmethod
    def _remove_work_dir(path: str) -> None:
        try:
            shutil.rmtree(path)
        except OSError as exc:
            raise CodexNativoErro("Não foi possível remover a pasta temporária do Codex.") from exc

    async def close(self) -> None:
        """Idempotente e concorrente: todos esperam o mesmo encerramento até o fim, mesmo sob
        cancelamentos repetidos; depois de uma falha, a próxima chamada tenta de novo."""
        self._accepting = False
        closing = self._closing
        if closing is None or (closing.done() and (closing.cancelled() or closing.exception() is not None)):
            closing = self._closing = asyncio.ensure_future(self._close_once())
        await account_lifecycle.complete_on_cancel(closing)

    async def _close_once(self) -> None:
        self._accepting = False
        self._disconnected()
        try:
            await self._finish(self._proc, self._tree)
            self._tree = None
        finally:
            if self._reader_task is not None:
                self._reader_task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await self._reader_task
                self._reader_task = None
            # Comandos admitidos terminam pela mesma limpeza registrada que o próprio cli espera.
            results = await asyncio.gather(
                *(self._operation_cleanup(operation) for operation in tuple(self._operations)),
                return_exceptions=True)
        self._proc = None
        failure = next((result for result in results if isinstance(result, BaseException)), None)
        if failure is not None:
            raise failure
        if self._work_dir is not None:
            self._remove_work_dir(self._work_dir)
            self._work_dir = None

    def _operation_cleanup(self, operation: _CliOperation) -> asyncio.Future:
        """Uma tarefa de limpeza por operação; só uma falha concluída abre nova tentativa."""
        cleanup = operation.cleanup
        if cleanup is None or (cleanup.done() and (cleanup.cancelled() or cleanup.exception() is not None)):
            cleanup = operation.cleanup = asyncio.ensure_future(self._finish_operation(operation))
        return cleanup

    async def _finish_operation(self, operation: _CliOperation) -> None:
        # Sem esperar o nascimento, a pasta sairia antes de o processo nascer dentro dela.
        await operation.spawned.wait()
        await self._finish(operation.proc, operation.tree)
        operation.tree = None
        self._remove_work_dir(operation.work_dir)
        self._operations.discard(operation)

    def _diagnostico_cli(self, args: list[str], codigo: int | None,
                        stdout: bytes, stderr: bytes) -> None:
        """Guarda a última falha por comando; saída bruta pode conter credenciais."""
        temporario = None
        try:
            pasta = log_paths.base() / "privado" / "codex" / diag.conta_id(self.codex_home)
            pasta.mkdir(mode=0o700, parents=True, exist_ok=True)
            chave = hashlib.sha256(json.dumps(args).encode()).hexdigest()[:16]
            destino = pasta / f"cli-{chave}.log"
            with tempfile.NamedTemporaryFile(dir=pasta, prefix=".cli-", delete=False) as arquivo:
                temporario = Path(arquivo.name)
                arquivo.write(json.dumps({"comando": args, "codigo": codigo,
                                          "stdout": stdout[-8192:].decode(errors="replace"),
                                          "stderr": stderr[-8192:].decode(errors="replace")},
                                         ensure_ascii=False).encode("utf-8"))
            substituir(temporario, destino)
            _log.warning("CLI Codex falhou ou devolveu erros (código %s); diagnóstico local: %s",
                         codigo, destino)
        except OSError:
            _log.warning("CLI Codex falhou ou devolveu erros (código %s); não foi possível gravar "
                         "o diagnóstico local", codigo)
        finally:
            if temporario is not None:
                with contextlib.suppress(OSError):
                    temporario.unlink(missing_ok=True)

    async def cli(self, args: list[str], *, esperado: str = "") -> dict:
        """`esperado`: trecho do stderr que o chamador trata como benigno — sem diagnóstico nem aviso."""
        if not self._accepting:
            raise CodexNativoErro("O cliente nativo do Codex está encerrando.")
        operation = _CliOperation(tempfile.mkdtemp(prefix="hangar-codex-admin-"))
        self._operations.add(operation)
        try:
            try:
                proc, operation.tree = await self._spawn(
                    [*_ADMIN_CONFIG, *args], operation.work_dir, stdin=asyncio.subprocess.DEVNULL,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                )
                operation.proc = proc
            finally:
                operation.spawned.set()
            try:
                stdout, stderr = await asyncio.wait_for(proc.communicate(), self.timeout)
            except TimeoutError:
                raise CodexNativoErro("O comando do Codex excedeu o tempo limite.") from None
            # O arquivo vai pro thread: `substituir` pode dormir no Windows, e isto roda no loop do SSE.
            diagnostico = functools.partial(asyncio.to_thread, self._diagnostico_cli,
                                            args, proc.returncode, stdout, stderr)
            if proc.returncode:
                # A cauda vai só em `data`, pra decisão interna (auto-upgrade em curso); nunca no log.
                cauda = stderr.decode(errors="replace")[-500:].strip()
                if not (esperado and esperado in cauda):
                    await diagnostico()
                raise CodexNativoErro(f"O comando do Codex falhou (código {proc.returncode}).",
                                      data={"stderr": cauda})
            try:
                result = json.loads(stdout)
            except (ValueError, UnicodeError):
                await diagnostico()
                raise CodexNativoErro("O comando do Codex não retornou JSON válido.") from None
            if not isinstance(result, dict):
                await diagnostico()
                raise CodexNativoErro("O comando do Codex retornou um resultado inválido.")
            if result.get("errors"):
                await diagnostico()
            return result
        finally:
            # A mesma limpeza registrada que o close espera; um novo cancelamento não a abandona.
            primary = sys.exception()
            try:
                await account_lifecycle.complete_on_cancel(self._operation_cleanup(operation))
            except CodexNativoErro:
                # A limpeza que falhou fica registrada para o close; o erro do comando é o que vale.
                if primary is None:
                    raise
                _log.warning("limpeza da pasta temporária do Codex adiada após falha do comando")

    async def instalar_plugin(self, plugin_id: str) -> dict:
        return await self.cli(["plugin", "add", plugin_id, "--json"])

    async def plugins_instalados(self) -> list[dict]:
        result = await self.cli(["plugin", "list", "--json"])
        plugins = result.get("installed")
        if not isinstance(plugins, list) or any(not isinstance(item, dict) for item in plugins):
            raise CodexNativoErro("O Codex retornou um inventário de plugins inválido.")
        return plugins

    async def marketplaces_instalados(self) -> list[dict]:
        result = await self.cli(["plugin", "marketplace", "list", "--json"])
        marketplaces = result.get("marketplaces")
        if not isinstance(marketplaces, list) or any(not isinstance(item, dict) for item in marketplaces):
            raise CodexNativoErro("O Codex retornou um inventário de marketplaces inválido.")
        return marketplaces

    async def atualizar_marketplace(self, nome: str) -> dict:
        try:
            return await self.cli(["plugin", "marketplace", "upgrade", nome, "--json"],
                                  esperado=_AUTO_UPGRADE_EM_CURSO)
        except CodexNativoErro as exc:
            # O próprio Codex já está atualizando esse marketplace (auto-upgrade ao abrir sessão):
            # o trabalho vai ser feito por ele, não é falha a retentar nem a pintar de vermelho.
            if _AUTO_UPGRADE_EM_CURSO in (exc.data or {}).get("stderr", ""):
                return {"selectedMarketplaces": [nome], "upgradedRoots": [], "errors": []}
            raise
