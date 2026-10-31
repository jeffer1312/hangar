"""Ponte de contas com o Rust: preparo e pedidos privados. Os fatos de uso
(processos, nascimentos, sidecars) são lidos no Rust, em `accounts/usage.rs`."""
from __future__ import annotations

from pathlib import Path
import json
import os
import re
import urllib.error
import urllib.request

from app.account_lifecycle import AccountKey
from app.mensagens import erro


def terminal_instances() -> dict[str, str]:
    from app import tmux
    result = tmux._run(["tmux", "list-sessions", "-F",
                       "#{session_name}\t#{pid}:#{session_id}:#{session_created}"])
    if result.returncode == 0 and "\ufffd" not in result.stdout:
        instances = {}
        for line in result.stdout.splitlines():
            name, identity = line.split("\t")
            parts = identity.split(":")
            if (not name or len(parts) != 3 or not parts[0].isdigit()
                    or not parts[1].startswith("$") or not parts[1][1:].isdigit()
                    or not parts[2].isdigit()):
                raise RuntimeError("account_mux_unknown")
            instances[name] = identity
        return instances
    error = result.stderr.strip()
    absent = error.startswith(("no server", "no sessions")) or (
        error.startswith("error connecting to ") and error.endswith("(No such file or directory)"))
    if result.returncode == 1 and absent:
        return {}
    raise RuntimeError("account_mux_unknown")


class PreparationJobs:
    """Trabalhos de configuração que continuam protegidos quando o chamador desaparece."""

    def __init__(self):
        self.jobs = {}
        self.stages = {}

    @staticmethod
    def validate(body):
        from app import runtime_coordinator, account_lifecycle, codex_contas
        coordinator = runtime_coordinator.current()
        if coordinator is None or coordinator.instance != body["instance"]:
            raise ValueError("instância de preparo encerrada")
        key = account_lifecycle.AccountKey.new(body["key"]["provider"], Path(body["key"]["canonical_home"]))
        name = body["account_id"]
        if not isinstance(name, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,31}", name):
            raise ValueError("conta inválida")
        expected = codex_contas.resolve_account(name).home if name == "default" else Path.home() / f".codex-{name}"
        if key != account_lifecycle.AccountKey.new(key.provider, expected) or expected.is_symlink() or not expected.is_dir():
            raise ValueError("destino inválido")
        record = account_lifecycle.default_lock_root() / (key.digest + ".prepare.json")
        if record.is_symlink() or json.loads(record.read_text(encoding="utf-8")) != body:
            raise ValueError("operação de preparo substituída")
        if codex_contas.resolve_account(name).home.resolve() != expected.resolve():
            raise ValueError("conta Codex inválida")
        cwd = body["cwd"]
        if cwd is not None and (not isinstance(cwd, str) or not Path(cwd).is_absolute() or not Path(cwd).is_dir()):
            raise ValueError("pasta de trabalho inválida")

    async def start(self, body):
        import asyncio
        if (not isinstance(body, dict) or set(body) != {"operation", "instance", "key", "account_id", "seed", "force", "cwd"}
                or not isinstance(body["operation"], str) or not re.fullmatch(r"[a-f0-9]{32}", body["operation"])
                or type(body["seed"]) is not bool or type(body["force"]) is not bool
                or not isinstance(body["key"], dict) or set(body["key"]) != {"provider", "canonical_home"}):
            raise ValueError("pedido de preparo inválido")
        self.validate(body)
        operation = body["operation"]
        if operation in self.jobs:
            previous, task = self.jobs[operation]
            if previous != body:
                raise ValueError("operação de preparo divergente")
            return task.result() if task.done() else self.running()
        if any(previous["key"] == body["key"] and not task.done() for previous, task in self.jobs.values()):
            raise ValueError("preparo anterior ainda está vivo")
        task = asyncio.create_task(self.run(body))
        self.jobs[operation] = (body, task)
        return self.running()

    @staticmethod
    def running(stage=None):
        return {"status": "running", "trust_pending": False, "issues": [], "etapa": stage}

    async def wait(self, operation):
        import asyncio
        job = self.jobs.get(operation)
        if job is None:
            return {"status": "unknown", "trust_pending": False, "issues": []}
        task = job[1]
        done, _ = await asyncio.wait({task}, timeout=1)
        if done:
            return task.result()
        stage = self.stages.get(operation)
        if stage is None and job[0]["key"]["provider"] == "codex":
            from app import codex_contas, codex_contas_sync
            stage = codex_contas_sync.preparation_status(codex_contas.resolve_account(job[0]["account_id"])).get("etapa")
        return self.running(stage)

    async def run(self, body):
        import asyncio
        from app import account_lifecycle, codex_contas, codex_contas_sync
        key = account_lifecycle.AccountKey.new(body["key"]["provider"], Path(body["key"]["canonical_home"]))

        async def owned():
            guard = await asyncio.to_thread(account_lifecycle.acquire, key, account_lifecycle.GuardMode.SHARED)
            with guard:
                self.validate(body)
                account = codex_contas.resolve_account(body["account_id"])
                if body["cwd"] is not None:
                    result = codex_contas_sync.preparation_status(account)
                    if result["status"] not in {"ready", "partial"}:
                        raise ValueError("conta ainda não está preparada")
                    async with codex_contas_sync.exclusivo(account.home / ".hangar-integracao.lock"):
                        self.validate(body)
                        def trust():
                            import os
                            import tomllib
                            from app.adapters.codex import sessions
                            sessions.pretrust_cwd(body["cwd"], codex_home=account.home)
                            config = tomllib.loads((account.home / "config.toml").read_text(encoding="utf-8"))
                            if config.get("projects", {}).get(os.path.abspath(body["cwd"]), {}).get("trust_level") != "trusted":
                                raise ValueError("confiança da pasta não foi confirmada")
                        await account_lifecycle.complete_on_cancel(asyncio.to_thread(trust))
                    return result
                from app import api
                service = getattr(api.app.state, "codex_contas_login", None)
                source_result = None
                if service is not None and service.atualizar_principal is not None:
                    self.stages[body["operation"]] = "principal"
                    try:
                        source_result = await service.atualizar_principal(body["force"])
                    finally:
                        self.stages.pop(body["operation"], None)
                result = await codex_contas_sync._prepare_account_guarded(
                    account, body["force"], validate=lambda: self.validate(body))
                source_optional = (isinstance(source_result,dict)
                                   and source_result.get("estado") == "indisponivel"
                                   and isinstance(source_result.get("etapa"),dict)
                                   and source_result["etapa"].get("codigo") == "etapa_sem_claude")
                if isinstance(source_result, dict) and not source_optional and source_result.get("estado") not in (None, "ok", "ocioso"):
                    if result["status"] == "ready":
                        result["status"] = "partial"
                    result.setdefault("issues", []).insert(0, {"code": "codex_account_source_sync_incomplete",
                                                               "params": {"status": str(source_result.get("estado"))}})
                if result["status"] == "ready":
                    from app.codex_preparo import reparar_conta
                    pending = await reparar_conta(account)
                    if pending is None:
                        raise RuntimeError("O Codex CLI não está instalado")
                    result["trust_pending"] = pending
                return result
        try:
            return await account_lifecycle.complete_on_cancel(owned())
        except asyncio.CancelledError:
            return {"status": "error", "trust_pending": False,
                    "issues": [{"code": "account_prepare_cancelled", "params": {}}]}
        except Exception as exc:
            return {"status": "error", "trust_pending": False,
                    "issues": [{"code": "codex_account_prepare_failed", "params": {"error": type(exc).__name__}}]}

_preparation_transport = None


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


# Sem proxy nem redirect: o segredo interno só vai ao loopback configurado.
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())


def owner_mode() -> str:
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    return "python" if coordinator is None else getattr(coordinator, "mode", "python")


def private_transport():
    """Endereço e segredo da porta privada do Rust, ou None antes da configuração."""
    return _preparation_transport


def _post(config, path, body, *, timeout, limit):
    request = urllib.request.Request(f"http://{config[0]}{path}", data=json.dumps(body).encode(),
                                     headers={"content-type": "application/json", "x-hangar-internal": config[1]},
                                     method="POST")
    with _opener.open(request, timeout=timeout) as response:
        return json.loads(response.read(limit))


def _error_detail(error, default):
    try:
        body = json.loads(error.read(16384))
    except (OSError, ValueError):
        return default
    return body.get("detail") if isinstance(body, dict) else default


def configure_preparation(address, secret):
    import ipaddress
    global _preparation_transport
    _preparation_transport = None
    if address is None or secret is None:
        return
    host, port = address.rsplit(":", 1)
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 1 <= int(port) <= 65535:
        raise ValueError("endereço privado de contas inválido")
    _preparation_transport = (address, secret)


def request_preparation(account, *, prepare=False, force=False, cwd=None):
    from app import codex_contas
    if owner_mode() == "python":
        return None
    config = _preparation_transport
    if config is None:
        raise codex_contas.AccountError(503, "account_prepare_bridge_unavailable", {})
    try:
        result = _post(config, "/__hangar_server/accounts",
                       {"account_id": account.id, "prepare": prepare, "force": force, "cwd": cwd},
                       timeout=15, limit=64 * 1024)
        if not isinstance(result, dict) or result.get("status") not in {"idle", "running", "ready", "partial", "error"}:
            raise ValueError("resposta de preparo inválida")
        return result
    except urllib.error.HTTPError as error:
        # Recusa do dono não é ponte indisponível: o código dele segue até quem chamou.
        detail = _error_detail(error, None)
        if isinstance(detail, dict) and isinstance(detail.get("code"), str):
            raise codex_contas.AccountError(error.code, detail["code"], detail.get("params") or {}) from None
        raise codex_contas.AccountError(503, "account_prepare_bridge_unavailable", {}) from None
    except (OSError, ValueError):
        raise codex_contas.AccountError(503, "account_prepare_bridge_unavailable", {}) from None


def request_process_uses(key: AccountKey, pid: int) -> bool | None:
    """Se o processo usa a conta, pela varredura do Rust; None quando não dá para saber."""
    config = _preparation_transport
    if owner_mode() == "python" or config is None:
        return None
    try:
        result = _post(config, "/__hangar_server/accounts",
                       {"process_uses": {"provider": key.provider.value, "canonical_home": str(key.canonical_home)},
                        "pid": pid},
                       timeout=30, limit=4096)
        uses = result.get("uses") if isinstance(result, dict) else None
        return uses if isinstance(uses, bool) else None
    except (OSError, ValueError):
        return None


def request_claude(action, *, path):
    """Encaminha consumidores Python ao dono Rust; pending nunca usa reserva Python."""
    from fastapi import HTTPException
    if owner_mode() == "python":
        return None
    unavailable = {"code": "account_auth_bridge_unavailable"}
    config = _preparation_transport
    if config is None:
        raise HTTPException(503, detail=unavailable)
    try:
        return _post(config, "/__hangar_server/accounts/claude",
                     {"action": action, "label": None, "path": path, "code": None},
                     timeout=30, limit=256 * 1024)
    except urllib.error.HTTPError as error:
        raise HTTPException(error.code, detail=_error_detail(error, unavailable)) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail=unavailable) from None


def request_codex(action, account, *, refresh=False):
    """Ponte de consumidores internos; pending recusa sem abrir um escritor Python."""
    from fastapi import HTTPException
    if owner_mode() == "python":
        return None
    unavailable = {"code": "account_auth_bridge_unavailable"}
    config = _preparation_transport
    if config is None:
        raise HTTPException(503, detail=unavailable)
    try:
        return _post(config, "/__hangar_server/accounts",
                     {"action": action, "account_id": account.id, "attempt_id": None, "refresh": refresh},
                     timeout=45, limit=256 * 1024)
    except urllib.error.HTTPError as error:
        detail = _error_detail(error, unavailable)
        if isinstance(detail, dict) and "code" in detail:
            from app.codex_contas import AccountError
            raise AccountError(error.code, detail["code"], detail.get("params", {})) from None
        raise HTTPException(error.code, detail=detail) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail=unavailable) from None


def request_quotas(*, force=False, cached_only=False, invalidate=None):
    """Encaminha cotas ao único escritor; falha em pending não ativa leitura Python."""
    from fastapi import HTTPException
    if owner_mode() == "python":
        return None
    unavailable = {"code": "quota_bridge_unavailable"}
    config = _preparation_transport
    if config is None:
        raise HTTPException(503, detail=unavailable)
    try:
        return _post(config, "/__hangar_server/quotas",
                     {"force": force, "cached_only": cached_only, "invalidate": invalidate},
                     timeout=45, limit=1024 * 1024)
    except (OSError, ValueError):
        raise HTTPException(503, detail=unavailable) from None


def request_device(action):
    """Encaminha somente a operação; credenciais permanecem no cofre local."""
    from fastapi import HTTPException
    mode = owner_mode()
    if mode == "python":
        raise HTTPException(503, detail=NEED_RUST)
    unavailable = {"code": "account_device_bridge_unavailable"}
    config = _preparation_transport
    if mode != "rust" or config is None:
        raise HTTPException(503, detail=unavailable)
    try:
        return _post(config, "/__hangar_server/accounts", {"device_action": action},
                     timeout=45, limit=256 * 1024)
    except urllib.error.HTTPError as error:
        raise HTTPException(error.code, detail=_error_detail(error, unavailable)) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail=unavailable) from None


# Rotas de conta e cota que só o Rust atende; sem ele, respondem NEED_RUST.
ACCOUNT_PREFIXES = ("/api/claude-configs", "/api/codex-contas", "/api/cotas", "/api/conta-estado",
                    "/api/credenciais/codex")
NEED_RUST = erro("accounts_need_rust_server", "contas e cotas precisam do servidor Rust")


async def forward_public(request, target: str = "/__hangar_server/accounts/public", *,
                         keep: tuple[str, ...] = (), limit: int = 8 * 1024 * 1024):
    """Leva ao Rust o pedido de conta que entrou pelas portas do Python, já autenticado, e devolve
    a resposta dele como veio. Em pending, sem transporte, recusa: nunca abre um segundo
    escritor. Quem chama já respondeu NEED_RUST no modo python. `keep` são os cabeçalhos da
    resposta do Rust que voltam ao cliente além do tipo."""
    import asyncio
    from starlette.responses import JSONResponse, Response
    config = _preparation_transport
    if config is None:
        return JSONResponse({"detail": {"code": "account_bridge_unavailable"}}, status_code=503)
    body = await request.body()
    query = request.url.query
    forwarded = urllib.request.Request(
        f"http://{config[0]}{target}" + (f"?{query}" if query else ""),
        data=body or None, method=request.method,
        headers={"content-type": request.headers.get("content-type", "application/json"),
                 "x-hangar-internal": config[1], "x-hangar-path": request.url.path})

    def send():
        try:
            with _opener.open(forwarded, timeout=330) as response:
                return response.status, response.headers, response.read(limit)
        except urllib.error.HTTPError as error:
            return error.code, error.headers, error.read(limit)

    try:
        status, headers, content = await asyncio.to_thread(send)
    except OSError:
        return JSONResponse({"detail": {"code": "account_bridge_unavailable"}}, status_code=503)
    kept = {name: headers[name] for name in keep if headers.get(name) is not None}
    return Response(content, status_code=status, media_type=headers.get("content-type") or "application/json",
                    headers=kept)


_HOP_BY_HOP = {"connection", "transfer-encoding", "keep-alive"}
_STREAM_HEADERS = ("content-type", "x-filename", "range", "if-range", "if-none-match", "if-modified-since", "last-event-id")


async def forward_stream(request, *, guest: bool):
    """Anexo, transcrição ou mídia da sessão que entrou por uma porta do Python, já autenticado:
    segue ao Rust em fluxo nos dois sentidos (até 100 MiB) e o transporte fecha quando o cliente sai.
    Tudo no loop, sem thread do executor padrão: uploads simultâneos não podem esgotá-lo."""
    import asyncio
    import h11
    from starlette.responses import JSONResponse, Response, StreamingResponse
    config = _preparation_transport
    if config is None:
        return JSONResponse({"detail": {"code": "account_bridge_unavailable"}}, status_code=503)
    host, _, port = config[0].rpartition(":")
    query = request.url.query
    headers = [("host", config[0]), ("x-hangar-internal", config[1]), ("x-hangar-path", request.url.path)]
    if guest:
        headers.append(("x-hangar-forwarded-guest", "1"))
    headers += [(key, request.headers[key]) for key in _STREAM_HEADERS if key in request.headers]
    body = request.method == "POST"
    if body:
        length = request.headers.get("content-length")
        headers.append(("content-length", length) if length is not None else ("transfer-encoding", "chunked"))
    conn = h11.Connection(h11.CLIENT)
    writer = None

    async def receive():
        while (event := conn.next_event()) is h11.NEED_DATA:
            conn.receive_data(await asyncio.wait_for(reader.read(64 * 1024), 480))
        return event

    try:
        reader, writer = await asyncio.wait_for(asyncio.open_connection(host, int(port)), 10)
        target = "/__hangar_server/forwarded" + (f"?{query}" if query else "")
        writer.write(conn.send(h11.Request(method=request.method, target=target, headers=headers)))
        if body:
            async for chunk in request.stream():
                if chunk:
                    writer.write(conn.send(h11.Data(data=chunk)))
                    await writer.drain()
        writer.write(conn.send(h11.EndOfMessage()))
        await writer.drain()
        response = await receive()
        while isinstance(response, h11.InformationalResponse):
            response = await receive()
        if not isinstance(response, h11.Response):
            raise h11.RemoteProtocolError("conexão fechada sem resposta")
        if response.status_code == 421:
            writer.close()
            return Response(status_code=421)
    except (OSError, TimeoutError, h11.ProtocolError) as error:
        if writer is not None:
            writer.close()
        from app import diag
        # Só a classe: a mensagem pode trazer trecho do pedido.
        diag.registrar("rust.forward_failed", "aviso", codigo="account_bridge_unavailable",
                       detalhe=request.url.path, causa=type(error).__name__)
        return JSONResponse({"detail": {"code": "account_bridge_unavailable"}}, status_code=503)
    except BaseException:
        if writer is not None:
            writer.close()
        raise

    async def chunks():
        try:
            while isinstance(event := await receive(), h11.Data):
                yield bytes(event.data)
        finally:
            writer.close()

    kept = {key.decode(): value.decode("latin-1") for key, value in response.headers
            if key.decode().lower() not in _HOP_BY_HOP}
    return StreamingResponse(chunks(), status_code=response.status_code, headers=kept)


preparation_jobs = PreparationJobs()
