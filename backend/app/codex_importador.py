"""Importação pelo protocolo oficial do Codex, sem abrir uma sessão de agente."""

import asyncio
import contextlib
import json
import os
from pathlib import Path
import shutil


_READ_LIMIT = 8 * 1024 * 1024
_COMPLETED = "externalAgentConfig/import/completed"


class CodexNativoErro(RuntimeError):
    """Falha do processo ou do protocolo nativo, sem expor configurações sensíveis."""

    def __init__(self, message: str, *, code: int | None = None, data: dict | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.data = data


class CodexNativo:
    def __init__(
        self, home: Path, codex_home: Path, binario: str = "codex", *,
        timeout: float = 120.0, close_timeout: float = 3.0,
    ) -> None:
        self.home = home.absolute()
        self.codex_home = codex_home.absolute()
        self.binario = binario
        self.timeout = timeout
        self.close_timeout = close_timeout
        self._proc: asyncio.subprocess.Process | None = None
        self._reader_task: asyncio.Task | None = None
        self._pending: dict[int, asyncio.Future] = {}
        self._next_id = 0
        self._closed = True
        self._import_lock = asyncio.Lock()
        self._completions: asyncio.Queue | None = None

    def _env(self) -> dict[str, str]:
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

    def _comando(self) -> list[str]:
        caminho = shutil.which(self.binario)
        if not caminho:
            raise CodexNativoErro("Codex CLI não encontrado; instale ou configure o executável.")
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

    async def __aenter__(self) -> "CodexNativo":
        if self._proc is not None:
            raise CodexNativoErro("O cliente nativo do Codex já está aberto.")
        try:
            self._proc = await asyncio.create_subprocess_exec(
                *self._comando(), "app-server", "--stdio", cwd=self.home, env=self._env(),
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

    async def _read_loop(self) -> None:
        assert self._proc is not None and self._proc.stdout is not None
        try:
            while raw := await self._proc.stdout.readline():
                if not raw.strip():
                    continue
                message = json.loads(raw)
                if not isinstance(message, dict):
                    continue
                if "id" in message:
                    req_id = message["id"]
                    future = self._pending.get(req_id) if isinstance(req_id, int) else None
                    if future is not None and not future.done():
                        future.set_result(message)
                elif message.get("method") == _COMPLETED and self._completions is not None:
                    params = message.get("params")
                    if isinstance(params, dict):
                        self._completions.put_nowait(params)
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

    async def close(self) -> None:
        self._disconnected()
        try:
            if self._proc is not None:
                await self._stop(self._proc)
        finally:
            if self._reader_task is not None:
                self._reader_task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await self._reader_task
                self._reader_task = None
            self._proc = None

    async def cli(self, args: list[str]) -> dict:
        proc = await asyncio.create_subprocess_exec(
            *self._comando(), *args, cwd=self.home, env=self._env(),
            stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        try:
            try:
                stdout, _ = await asyncio.wait_for(proc.communicate(), self.timeout)
            except TimeoutError:
                raise CodexNativoErro("O comando do Codex excedeu o tempo limite.") from None
            if proc.returncode:
                raise CodexNativoErro(f"O comando do Codex falhou (código {proc.returncode}).")
            try:
                result = json.loads(stdout)
            except (ValueError, UnicodeError):
                raise CodexNativoErro("O comando do Codex não retornou JSON válido.") from None
            if not isinstance(result, dict):
                raise CodexNativoErro("O comando do Codex retornou um resultado inválido.")
            return result
        finally:
            await self._stop(proc)

    async def instalar_plugin(self, plugin_id: str) -> dict:
        return await self.cli(["plugin", "add", plugin_id, "--json"])

    async def plugins_instalados(self) -> list[dict]:
        result = await self.cli(["plugin", "list", "--json"])
        plugins = result.get("installed")
        if not isinstance(plugins, list) or any(not isinstance(item, dict) for item in plugins):
            raise CodexNativoErro("O Codex retornou um inventário de plugins inválido.")
        return plugins

    async def atualizar_marketplace(self, nome: str) -> dict:
        return await self.cli(["plugin", "marketplace", "upgrade", nome, "--json"])
