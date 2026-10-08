"""Referência isolada dos contratos de contas e prova de autoria da migração."""

from __future__ import annotations

import argparse
import asyncio
from concurrent.futures import ThreadPoolExecutor
import json
from json import dumps
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler, HTTPRedirectHandler


FIXTURES = Path(__file__).parent / "fixtures" / "accounts_contract"
_BRIDGE_OPERATIONS = frozenset({"bridge.prepare", "bridge.claude_window", "bridge.other_quotas", "bridge.propagate_device_login"})


def assert_rust_ownership(calls: list[dict]) -> None:
    """Confere que nenhum handler de conta Python atendeu ao pedido."""
    forbidden = [call["operation"] for call in calls if call["operation"] not in _BRIDGE_OPERATIONS]
    assert not forbidden, f"operação de conta ainda executada pelo Python: {forbidden}"


class ContractResponse:
    def __init__(self, status_code: int, body: bytes):
        self.status_code = status_code
        self.body = body

    def json(self):
        return json.loads(self.body)


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


class HttpTransport:
    """Transporte reutilizável contra a porta pública de uma instância isolada."""

    def __init__(self, base_url: str, token: str):
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.opener = build_opener(ProxyHandler({}), _NoRedirect())

    def request(self, method: str, path: str, json=None) -> ContractResponse:
        if not path.startswith("/") or path.startswith("//"):
            raise ValueError("a rota deve ser relativa à instância de teste")
        payload = None if json is None else dumps(json).encode("utf-8")
        request = Request(self.base_url + path, data=payload, method=method,
                          headers={"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"})
        try:
            response = self.opener.open(request, timeout=30)
        except HTTPError as error:
            response = error
        with response:
            return ContractResponse(response.status, response.read())


def normalize(value, *, root: Path):
    """Normaliza só a raiz declarada; mantém campos, ordem, nulos e tipos."""
    roots = {str(root), root.as_posix()}
    if isinstance(value, str):
        for candidate in sorted(roots, key=len, reverse=True):
            value = value.replace(candidate, "<HOME>")
        # Separadores variam por sistema apenas em caminhos da raiz isolada.
        if "<HOME>" in value:
            value = value.replace("\\", "/")
        return value
    if isinstance(value, list):
        return [normalize(item, root=root) for item in value]
    if isinstance(value, dict):
        return {normalize(key, root=root): normalize(item, root=root) for key, item in value.items()}
    return value


def isolated_environment(root: Path) -> dict[str, str]:
    """Não herda identidade, configuração, sessão nem proxies da máquina."""
    allowed = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP", "LANG", "LC_ALL"}
    result = {key: value for key, value in os.environ.items() if key.upper() in allowed}
    drive, tail = os.path.splitdrive(str(root))
    result.update({"HOME": str(root), "USERPROFILE": str(root), "HOMEDRIVE": drive,
                   "HOMEPATH": tail, "CLAUDE_CONFIG_DIR": str(root / ".claude"),
                   "CODEX_HOME": str(root / ".codex"), "CP_AUTH_TOKEN": "contract-only",
                   "CP_RUST_SERVER": "0", "CP_CODEX_SYNC_ENABLED": "0",
                   "CP_PROJECTS_DIR": str(root / ".claude" / "projects"),
                   "PYTHONPATH": str(Path(__file__).resolve().parents[1]),
                   "PYTHONIOENCODING": "utf-8", "PYTHONUTF8": "1"})
    for key, directory in {"XDG_CONFIG_HOME": ".config", "XDG_CACHE_HOME": ".cache",
                           "XDG_DATA_HOME": ".local/share", "XDG_STATE_HOME": ".local/state",
                           "APPDATA": "appdata", "LOCALAPPDATA": "localappdata"}.items():
        result[key] = str(root / directory)
    return result


class PythonReference(HttpTransport):
    """Filho com rotas reais, sem lifespan do servidor nem CLI/rede real."""

    def __init__(self, root: Path, *, block_handlers: bool = False):
        self.root = root
        root.mkdir(parents=True, exist_ok=True)
        self.log = (root / "worker.log").open("w", encoding="utf-8")
        arguments = [sys.executable, str(Path(__file__).resolve()), "--worker"]
        if block_handlers:
            arguments.append("--block-handlers")
        self.process = subprocess.Popen(arguments, cwd=root, env=isolated_environment(root),
                                        stdout=subprocess.PIPE, stderr=self.log, text=True,
                                        encoding="utf-8", errors="strict")
        ready = queue.Queue()
        threading.Thread(target=lambda: ready.put(self.process.stdout.readline()), daemon=True).start()
        try:
            notice = json.loads(ready.get(timeout=30))
            super().__init__(notice["base_url"], "contract-only")
        except Exception:
            self.close()
            raise RuntimeError("a referência Python não iniciou; confira worker.log no diretório isolado") from None

    def block(self, name: str):
        assert self.request("POST", "/__contract__/barrier/" + name).status_code == 200
        reference = self
        class RemoteEvent:
            def __init__(self, action):
                self.action = action
            def wait(self, timeout=20):
                response = reference.request("GET", "/__contract__/barrier/" + name)
                return response.json()["entered"]
            def set(self):
                assert reference.request("POST", "/__contract__/release/" + name).status_code == 200
        class Barrier:
            entered = RemoteEvent("entered")
            release = RemoteEvent("release")
        return Barrier()

    def start_session_async(self, *, provider: str, account_id: str, name="birth"):
        if not hasattr(self, "_executor"):
            self._executor = ThreadPoolExecutor()
        payload = {"name": name, "cwd": str(self.root), "provider": provider,
                   "headless": False, "remember_provider": False}
        if provider == "codex":
            payload["codex_account"] = account_id
        else:
            payload["config_dir"] = str(self.root / f".claude-{account_id}")
        return self._executor.submit(self.request, "POST", "/api/sessions", payload)

    def account_exists(self, provider: str, account_id: str) -> bool:
        return (self.root / f".{provider}-{account_id}").is_dir()

    def cancel_birth(self):
        return self.request("POST", "/__contract__/cancel-birth")

    def calls(self) -> list[dict]:
        return self.request("GET", "/__contract__/calls").json()

    def tree(self, account: str) -> dict:
        return self.request("GET", f"/__contract__/tree/{account}").json()

    def close(self) -> None:
        try:
            if self.process.poll() is None and hasattr(self, "base_url"):
                self.request("POST", "/__contract__/cleanup")
        finally:
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=10)
            if hasattr(self, "_executor"):
                self._executor.shutdown(wait=True, cancel_futures=True)
            self.process.stdout.close()
            self.log.close()


class RustClaude(HttpTransport):
    """Transporte real Rust com CLI sintética em árvore descartável."""
    _prepared_targets: set[Path] = set()

    def __init__(self, reference):
        self.reference = reference
        target = Path(os.environ["CARGO_TARGET_DIR"]) / "debug/deps"
        candidates = [p for p in target.glob("accounts_claude_login-*")
                      if p.is_file() and p.suffix in {"", ".exe"}]
        if target not in self._prepared_targets:
            # O gate pode reutilizar um target cujo binário veio de outra fonte.
            build_log = reference.root / "rust-build.log"
            with build_log.open("w", encoding="utf-8") as output:
                built = subprocess.run([
                    "cargo", "test", "--locked", "-p", "hangar-server",
                    "--test", "accounts_claude_login", "--no-run",
                ], cwd=Path(__file__).resolve().parents[2] / "crates",
                    stdout=output, stderr=subprocess.STDOUT, timeout=900)
            assert built.returncode == 0, f"Falha ao compilar a sonda Rust; log: {build_log}"
            self._prepared_targets.add(target)
            candidates = [p for p in target.glob("accounts_claude_login-*")
                          if p.is_file() and p.suffix in {"", ".exe"}]
        binary = max(candidates, key=lambda p: p.stat().st_mtime)
        environment = isolated_environment(reference.root)
        fixture = reference.root / "claude-native"
        script = fixture / "node_modules/@anthropic-ai/claude-code/cli.js"
        script.parent.mkdir(parents=True, exist_ok=True)
        source = """const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;
if(process.argv.slice(-2).join(' ')==='auth logout'){fs.writeFileSync(p.join(d,'auth-reply.json'),JSON.stringify({loggedIn:false}));fs.rmSync(p.join(d,'.credentials.json'),{force:true});process.exit(0);}
let r;try{r=fs.readFileSync(p.join(d,'auth-reply.json'),'utf8')}catch{r=JSON.stringify({loggedIn:false})}
process.stdout.write(r);process.exit(r.includes('false')?1:0);"""
        script.write_text(source, encoding="utf-8")
        if os.name != "nt":
            executable = fixture / "claude"
            executable.write_text("#!/usr/bin/env node\n" + source, encoding="utf-8")
            executable.chmod(0o700)
        environment["PATH"] = str(fixture) + os.pathsep + environment["PATH"]
        environment.update(ACCOUNT_HTTP_UPSTREAM=reference.base_url.removeprefix("http://"),
                           HANGAR_RUNTIME_INSTANCE="contract-instance")
        self.process = subprocess.Popen([str(binary), "--exact", "http_probe_process", "--nocapture"],
                                        cwd=reference.root, env=environment, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding="utf-8")
        ready = queue.Queue()
        def read():
            for line in self.process.stdout:
                if line.startswith("ACCOUNT_HTTP:"):
                    ready.put(line.strip().removeprefix("ACCOUNT_HTTP:"))
                    return
            ready.put(None)
        threading.Thread(target=read, daemon=True).start()
        address = ready.get(timeout=30)
        assert address, "a sonda Rust não anunciou HTTP"
        super().__init__("http://" + address, "contract-only")

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=15)
        self.process.stdout.close()
        self.process.stderr.close()

def _worker(block_handlers: bool) -> None:
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from unittest.mock import patch
    from fastapi import FastAPI
    from fastapi.responses import JSONResponse
    from fastapi.routing import APIRoute
    from fastapi.testclient import TestClient
    from app import api, conta_estado, codex_contas_api, codex_contas_login, cotas, credenciais, account_bridge
    import psutil
    from types import SimpleNamespace
    from app import internal_api, runtime_coordinator

    root = Path.home()
    assert root == Path(os.environ["CLAUDE_CONFIG_DIR"]).parent
    assert root == Path(os.environ["CODEX_HOME"]).parent
    (root / ".claude").mkdir(exist_ok=True)
    (root / ".codex").mkdir(exist_ok=True)
    work = root / ".claude-work"
    work.mkdir(exist_ok=True)
    (work / ".hangar-conta").write_text("", encoding="utf-8")
    (work / "projects").mkdir(exist_ok=True)
    recent = work / "projects" / "recent.jsonl"
    recent.write_text("", encoding="utf-8")
    os.utime(recent, (200, 200))
    backup = root / ".claude-backup"
    backup.mkdir(exist_ok=True)
    (backup / ".credentials.json").write_text("{}", encoding="utf-8")
    (backup / "projects").mkdir(exist_ok=True)
    older = backup / "projects" / "older.jsonl"
    older.write_text("", encoding="utf-8")
    os.utime(older, (100, 100))
    (root / ".claude" / ".hangar-apelidos.json").write_text(
        json.dumps({f"claude:{work.resolve()}": "Trabalho de revisão"}, ensure_ascii=False), encoding="utf-8")
    for name in ("zeta", "alpha"):
        target = root / f".codex-{name}"
        target.mkdir(exist_ok=True)
        (target / ".hangar-codex-conta").write_text(json.dumps({"version": 1, "id": name}), encoding="utf-8")
        (target / "config.toml").write_text('cli_auth_credentials_store = "file"\n', encoding="utf-8")

    class DisconnectedNative:
        def __init__(self, *args, **kwargs):
            pass

        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            await self.close()

        async def close(self):
            pass

        async def request(self, method, params=None, **kwargs):
            assert method == "account/read", f"chamada nativa não prevista: {method}"
            return {"account": None}

    barriers = {}
    instance = SimpleNamespace(instance="contract-instance", legacy=None, managed_queue=lambda name: False)
    preparation_calls = []
    preparation_entered = asyncio.Event()
    from app import codex_contas_sync
    original_prepare = codex_contas_sync._prepare_account_guarded
    async def prepare_at_barrier(account, force=False, *, validate=None):
        barrier = barriers.get("account_prepare")
        if barrier is None:
            return await original_prepare(account, force, validate=validate)
        if validate is not None:
            validate()
        preparation_calls.append(force)
        barrier[0].set()
        if not await asyncio.to_thread(barrier[1].wait, 25):
            raise RuntimeError("barreira de preparo não foi liberada")
        return {"status": "ready", "trust_pending": False, "issues": []}
    births = {}
    published = threading.Event()
    children = []
    mux = {}
    launcher_options = {}
    original_popen = subprocess.Popen
    from app import account_lifecycle
    original_publish = account_lifecycle.publish_terminal_birth

    def publish_at_barrier(*args, **kwargs):
        barrier = barriers.get("birth_publication")
        if barrier:
            barrier[0].set()
            if not barrier[1].wait(25):
                raise RuntimeError("barreira de publicação não foi liberada")
        if launcher_options.get("unknown_instance"):
            with patch.object(account_bridge, "terminal_instances", side_effect=RuntimeError("snapshot indisponível")):
                return original_publish(*args, **kwargs)
        return original_publish(*args, **kwargs)

    def create_at_barrier(name, cwd, config_dir, **kwargs):
        from app.models import SessionInfo
        from app.adapters.codex import sessions
        barrier = barriers.get("session_before_registration")
        if barrier:
            barrier[0].set()
            if not barrier[1].wait(25):
                raise RuntimeError("barreira não foi liberada")
        home = root / (".codex-" + kwargs.get("codex_account", "alpha"))
        command = [sys.executable, "-c", "import sys; sys.stdin.readline()"]
        if not launcher_options.get("opaque"):
            command += ["--", kwargs["provider"], "--codex-home", str(home)]
        child = original_popen(command,
                               cwd=root, env=isolated_environment(root), stdin=subprocess.PIPE,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        children.append(child)
        mux[name] = str(child.pid)
        launcher = barriers.get("launcher_before_publication")
        if launcher:
            def publish():
                launcher[0].set()
                if launcher[1].wait(25) and child.poll() is None:
                    current_name = next((key for key, value in mux.items() if value == str(child.pid)), name)
                    sessions.save(current_name, "thread-born", "", str(cwd), app_pid=child.pid,
                                  endpoint="ws://127.0.0.1:1", codex_home=home,
                                  codex_account=kwargs.get("codex_account", "alpha"))
                    published.set()
            threading.Thread(target=publish, daemon=True).start()
        return SessionInfo(name=name, cwd=cwd, provider=kwargs["provider"])

    service = codex_contas_login.CodexContasLogin(native=DisconnectedNative, account_in_use=lambda account: False)
    app = FastAPI()
    internal_api.set_secret("contract-internal")
    app.include_router(internal_api.router)
    app.state.codex_contas_login = service
    prefixes = {"/api/claude-configs": "claude", "/api/conta-estado": "claude",
                "/api/codex-contas": "codex", "/api/cotas": "quotas",
                "/api/credenciais/codex": "device"}
    calls = []
    from app import login_conta
    windows = {}
    window_calls = []
    code_entered = threading.Event()
    @app.get("/__contract__/wait-claude-code")
    async def wait_claude_code():
        return {"entered": await asyncio.to_thread(code_entered.wait, 20)}
    def window_create(name, cwd, config_dir=None):
        windows["term-" + name] = True
        window_calls.append({"action": "open", "name": "term-" + name, "config_dir": config_dir})
        return "term-" + name
    def window_send(name, text):
        window_calls.append({"action": "command", "name": name, "command": text})
    def window_code(name, text):
        # A prova guarda somente a classificação da entrada, nunca o código.
        window_calls.append({"action": "code", "name": name, "protected": True})
        code_entered.set()
    def window_close(name):
        windows.pop(name, None)
        window_calls.append({"action": "close", "name": name})
    @app.get("/__contract__/claude-windows")
    def claude_windows():
        return {"windows": list(windows), "calls": window_calls}


    @app.post("/__contract__/claude-owner")
    def claude_owner(body: dict):
        from app import account_bridge
        account_bridge.configure_preparation(body["address"], "contract-internal")
        instance.mode = "rust"
        return {"ok": True}

    @app.get("/__contract__/claude-auth")
    def claude_auth(path: str):
        from app import account_bridge
        return account_bridge.request_claude("auth", path=path)
    for route in api.app.routes:
        if isinstance(route, APIRoute) and route.path.startswith("/api/claude-configs"):
            app.router.routes.append(route)
    app.include_router(conta_estado.conta_estado_router)
    app.include_router(codex_contas_api.codex_contas_router)
    app.include_router(cotas.cotas_router)
    app.include_router(credenciais.credenciais_router)

    @app.post("/api/sessions")
    async def birth(body: dict):
        task = asyncio.current_task()
        births["task"] = task
        try:
            return await api.create_session(api.CreateBody.model_validate(body))
        except asyncio.CancelledError:
            return JSONResponse({"cancelled": True}, status_code=499)
        finally:
            births.pop("task", None)

    @app.post("/api/sessions/{name}/rename")
    async def rename(name: str, body: dict):
        return await api.rename_session(name, api.RenameBody.model_validate(body))

    @app.post("/__contract__/launcher-options")
    def options(body: dict):
        launcher_options.update(body)
        if body.get("live_claude"):
            environment = isolated_environment(root)
            environment["CLAUDE_CONFIG_DIR"] = str(work)
            child = original_popen([sys.executable, "-c", "import sys; sys.stdin.readline()", "--", "claude"],
                                   cwd=root, env=environment, stdin=subprocess.PIPE,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            children.append(child)
            assert child.poll() is None
            return {"ok": True, "pid": child.pid}
        return {"ok": True}

    @app.post("/__contract__/reuse-name/{name}")
    def reuse(name: str):
        mux[name] = "replacement-instance"
        return {"ok": True}

    def mux_command(arguments, **kwargs):
        if arguments[1] == "display-message" and arguments[-1] == "#{session_created}":
            return subprocess.CompletedProcess(arguments, 0, "200\n", "")
        if arguments[1] == "list-sessions" and "@cp_shortcut_owner" in arguments[-1]:
            return subprocess.CompletedProcess(arguments, 0, "", "")
        raise AssertionError(f"comando do multiplexador não previsto: {arguments}")

    def rename_mux(old, new):
        mux[new] = mux.pop(old)
        return True

    @app.post("/__contract__/barrier/{name}")
    def block(name: str):
        assert name in {"session_before_registration", "launcher_before_publication", "birth_publication", "account_prepare"}
        barriers[name] = (threading.Event(), threading.Event())
        return {"ok": True}

    @app.get("/__contract__/barrier/{name}")
    def entered(name: str):
        return {"entered": barriers[name][0].wait(20)}

    @app.post("/__contract__/release/{name}")
    def release(name: str):
        barriers[name][1].set()
        return {"ok": True}

    @app.post("/__contract__/cancel-birth")
    async def cancel_birth():
        task = births.get("task")
        if task:
            task.cancel()
        return {"cancelled": task is not None}

    @app.post("/__contract__/reserve-other")
    async def reserve_other():
        from app import codex_contas
        lease = service.reserve_creation(codex_contas.resolve_account("zeta"))
        lease.release()
        return {"ok": True}

    @app.get("/__contract__/usage")
    def usage():
        from app import account_bridge
        from app.account_lifecycle import AccountKey
        from dataclasses import asdict
        import psutil
        with patch.object(account_bridge, "system_processes", return_value=[
                psutil.Process(child.pid) for child in children if child.poll() is None]), \
                patch.object(account_bridge, "terminal_sessions", return_value={
                    "birth" for child in children if child.poll() is None}):
            return asdict(account_bridge.inspect_usage(AccountKey.new("codex", root / ".codex-alpha")))

    @app.get("/__contract__/published")
    def publication():
        assert published.wait(20)
        return {"published": True}

    @app.post("/__contract__/retire-birth")
    def retire_birth():
        for state in list(api._codex_live_leases.values()):
            state["lease"].retire_birth()
        return {"ok": True}

    @app.post("/__contract__/stop-launcher")
    def stop_launcher():
        for child in children:
            if child.poll() is None:
                child.communicate(b"release\\n", timeout=10)
        return {"ok": True}

    @app.post("/__contract__/cleanup")
    def cleanup():
        for _, release in barriers.values():
            release.set()
        for child in children:
            if child.poll() is None:
                child.communicate(b"release\n", timeout=10)
        for state in list(api._codex_live_leases.values()):
            state["lease"].release()
        return {"ok": True}

    @app.middleware("http")
    async def record(request, call_next):
        if request.url.path == "/internal/accounts/claude-window":
            response = await call_next(request)
            calls.append({"operation": "bridge.claude_window", "status": response.status_code})
            return response
        prefix = next((prefix for prefix in prefixes if request.url.path.startswith(prefix)), None)
        if prefix:
            suffix = "catalog" if request.url.path == prefix else "operation"
            operation = f"{prefixes[prefix]}.{suffix}"
            entry = {"operation": operation, "method": request.method, "path": request.url.path}
            calls.append(entry)
            if block_handlers:
                entry["status"] = 503
                return JSONResponse({"detail": {"code": "contract_python_handler_blocked", "operation": operation}}, status_code=503)
            response = await call_next(request)
            entry["status"] = response.status_code
            return response
        return await call_next(request)

    @app.post("/__contract__/runtime-instance")
    def change_instance(body: dict):
        instance.instance = body["instance"]
        return {"ok": True}

    @app.post("/__contract__/source-update")
    def source_update(body: dict):
        async def update(_force):
            preparation_entered.set()
            if body.get("raise"):
                raise RuntimeError("falha sintética da atualização principal")
            return body
        service.atualizar_principal = update
        return {"ok": True}

    @app.get("/__contract__/wait-preparations")
    async def wait_preparations():
        await asyncio.wait_for(preparation_entered.wait(), 20)
        jobs = list(account_bridge.preparation_jobs.jobs.values())
        assert jobs
        return await jobs[-1][1]

    @app.get("/__contract__/preparation-calls")
    def preparation_journal():
        return preparation_calls

    @app.get("/__contract__/preparation-operations")
    def preparation_operations():
        return list(account_bridge.preparation_jobs.jobs)

    @app.get("/__contract__/calls")
    def journal():
        return calls

    @app.get("/__contract__/tree/{account}")
    def tree(account: str):
        assert account in {"fresh", "alpha", "zeta"}
        target = root / f".codex-{account}"
        result = {}
        for path in sorted(target.rglob("*")):
            if not path.is_file():
                continue
            name = str(path.relative_to(target)).replace("\\", "/")
            text = path.read_text(encoding="utf-8")
            if name == ".hangar-codex-conta":
                result[name] = json.loads(text)
            elif name == "config.toml":
                result[name] = tomllib.loads(text)
            else:
                result[name] = text
        return result

    def deny_external(*args, **kwargs):
        raise AssertionError("uma operação de contrato tentou executar CLI ou acessar rede real")


    import socket
    original_connection = socket.create_connection
    def private_connection(address, *args, **kwargs):
        transport = account_bridge._preparation_transport
        if transport is not None:
            host, port = transport[0].rsplit(":", 1)
            if address == (host.strip("[]"), int(port)):
                return original_connection(address, *args, **kwargs)
        return deny_external()

    with patch.multiple(login_conta, _shell_criar=window_create, _shell_submeter=window_send,
                        _shell_ler=lambda name: "https://claude.ai/oauth/authorize?fixture=1\nPaste code here if prompted",
                        _shell_matar=window_close, _shell_code=window_code, create=True), \
            patch.object(runtime_coordinator, "current", return_value=instance), \
            patch.object(conta_estado, "_auth_status", return_value={"loggedIn": False}), \
            patch.object(api.app.state, "codex_contas_login", service, create=True), \
            patch.object(api.registry, "create", side_effect=create_at_barrier), \
            patch.object(account_lifecycle, "publish_terminal_birth", side_effect=publish_at_barrier), \
            patch.object(api, "_codex_require_idle_preparation"), \
            patch.object(api, "_invalidate_lists"), \
            patch.object(codex_contas_sync, "_prepare_account_guarded", side_effect=prepare_at_barrier), \
            patch.object(api.tmux, "has_session", side_effect=lambda name: name in mux and any(child.poll() is None for child in children)), \
            patch.object(api.tmux, "_run", side_effect=mux_command), \
            patch.object(api.tmux, "rename_session", side_effect=rename_mux), \
            patch.object(api.tmux, "is_hidden", return_value=False), \
            patch.object(api.registry, "_rename_rust"), \
            patch.object(account_bridge, "terminal_instances", side_effect=lambda: dict(mux) if any(
                child.poll() is None for child in children) else {}), \
            patch.object(account_bridge, "system_processes", side_effect=lambda: [
                psutil.Process(child.pid) for child in children if child.poll() is None]), \
            patch("subprocess.Popen", side_effect=deny_external), \
            patch("socket.create_connection", side_effect=private_connection), TestClient(app, client=("127.0.0.1", 32123)) as client:
        client.portal.call(service.aquecer)
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _handle(self):
                size = int(self.headers.get("Content-Length", "0"))
                body = self.rfile.read(size) if size else None
                # Controle fora do loop testado permite liberar a barreira até se ele travar.
                if self.command == "POST" and self.path.startswith("/__contract__/release/"):
                    barriers[self.path.rsplit("/", 1)[1]][1].set()
                    self.send_response(200)
                    self.send_header("Content-Length", "2")
                    self.end_headers()
                    self.wfile.write(b"{}")
                    return
                response = client.request(self.command, self.path, content=body,
                                          headers={"Authorization": self.headers.get("Authorization", ""),
                                                   "Content-Type": "application/json",
                                                   "x-hangar-internal": self.headers.get("x-hangar-internal", ""),
                                                   "x-hangar-runtime-instance": self.headers.get("x-hangar-runtime-instance", "")})
                self.send_response(response.status_code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(response.content)))
                self.end_headers()
                self.wfile.write(response.content)

            do_GET = do_POST = do_DELETE = _handle

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            print(json.dumps({"base_url": f"http://127.0.0.1:{server.server_port}"}), flush=True)
            server.serve_forever()


REFERENCE_CASES = (
    ("claude_catalog", "GET", "/api/claude-configs", None),
    ("claude_state", "GET", "/api/conta-estado", None),
    ("claude_invalid_name", "POST", "/api/claude-configs", {"nome": "conta\n"}),
    ("claude_extra_field", "POST", "/api/claude-configs", {"nome": "valid", "extra": True}),
    ("claude_missing_login", "POST", "/api/conta-estado/absent/login", None),
    ("claude_login_idle", "GET", "/api/conta-estado/work/login/passo", None),
    ("claude_login_cancel_idle", "POST", "/api/conta-estado/work/login/cancelar", None),
    ("claude_logout_missing", "POST", "/api/claude-configs/absent/logout", None),
    ("codex_catalog", "GET", "/api/codex-contas", None),
    ("codex_missing", "DELETE", "/api/codex-contas/absent", None),
    ("codex_protected", "DELETE", "/api/codex-contas/default", None),
    ("codex_invalid_name", "POST", "/api/codex-contas", {"name": "conta\n"}),
    ("codex_extra_field", "POST", "/api/codex-contas", {"name": "valid", "extra": True}),
    ("codex_create", "POST", "/api/codex-contas", {"name": "fresh"}),
    ("codex_duplicate", "POST", "/api/codex-contas", {"name": "fresh"}),
    ("codex_prepare_idle", "GET", "/api/codex-contas/fresh/prepare", None),
    ("codex_login_null", "GET", "/api/codex-contas/fresh/login", None),
    ("codex_cancel_requires_attempt", "DELETE", "/api/codex-contas/fresh/login", None),
    ("codex_cancel_old_attempt", "DELETE", "/api/codex-contas/fresh/login?attempt_id=old", None),
    ("codex_reset_invalid_uuid", "POST", "/api/codex-contas/fresh/rate-limit-reset", {"idempotency_key": "invalid"}),
    ("device_state", "GET", "/api/credenciais/codex", None),
    ("device_login_idle", "GET", "/api/credenciais/codex/login", None),
    ("device_cancel_idle", "DELETE", "/api/credenciais/codex/login", None),
    ("quotas_disconnected", "GET", "/api/cotas", None),
    ("quota_suggestion_empty", "GET", "/api/cotas/sugestao", None),
    ("codex_delete", "DELETE", "/api/codex-contas/fresh", None),
)


def capture_reference(reference: PythonReference) -> dict:
    result = {}
    for name, method, path, payload in REFERENCE_CASES:
        response = reference.request(method, path, json=payload)
        if response.status_code == 404:
            assert isinstance(response.json().get("detail"), dict) or name == "quota_suggestion_empty", \
                f"rota de referência não montada: {method} {path}"
        result[name] = {"method": method, "path": path, "status": response.status_code,
                        "body": normalize(response.json(), root=reference.root)}
        if name == "codex_create":
            result[name]["tree"] = reference.tree("fresh")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Referência Python isolada de contas")
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--block-handlers", action="store_true")
    parser.add_argument("--capture", type=Path)
    arguments = parser.parse_args()
    if arguments.worker:
        _worker(arguments.block_handlers)
    elif arguments.capture:
        import tempfile
        with tempfile.TemporaryDirectory(prefix="hangar-accounts-contract-") as temporary:
            reference = PythonReference(Path(temporary))
            try:
                result = capture_reference(reference)
                arguments.capture.parent.mkdir(parents=True, exist_ok=True)
                arguments.capture.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
            finally:
                reference.close()
