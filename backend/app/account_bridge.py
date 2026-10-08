"""Fatos de uso sem adquirir novamente a proteção de existência da conta."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import os

try:
    import psutil
except ImportError:
    psutil = None

from app.account_lifecycle import AccountKey, Provider


@dataclass
class UsageFacts:
    complete: bool = False
    sessions: list[str] = field(default_factory=list)
    pids: list[int] = field(default_factory=list)

    def ensure_unused(self) -> None:
        if not self.complete:
            raise RuntimeError("account_usage_unknown")
        if self.sessions or self.pids:
            raise RuntimeError("account_in_use")


_HOSTS = {"node", "bun", "python", "python3", "python3.14", "hangar-cano",
          "bash", "sh", "zsh", "fish", "cmd", "powershell", "pwsh"}
_CLIENTS = {"claude", "codex", "pi", "omp"}


_GONE = (FileNotFoundError, ProcessLookupError) + ((psutil.NoSuchProcess, psutil.ZombieProcess) if psutil else ())
_UNREADABLE = (OSError, ValueError, IndexError) + ((psutil.AccessDenied,) if psutil else ())
_SCAN_ERRORS = (OSError,) + ((psutil.Error,) if psutil else ())


class LinuxProcess:
    """Inspeção estrita em /proc; produção Linux não depende de psutil."""
    def __init__(self, pid: int):
        self.pid = pid
        self.root = Path("/proc") / str(pid)

    def name(self):
        return (self.root / "comm").read_text().strip()

    def cmdline(self):
        return [part.decode("utf-8", "surrogateescape") for part in (self.root / "cmdline").read_bytes().split(b"\0") if part]

    def cwd(self):
        return os.readlink(self.root / "cwd")

    def create_time(self):
        stat = (self.root / "stat").read_text()
        return int(stat[stat.rindex(")") + 1:].split()[19])

    def environ(self):
        return dict(entry.decode("utf-8", "surrogateescape").split("=", 1)
                    for entry in (self.root / "environ").read_bytes().split(b"\0") if b"=" in entry)


def system_process(pid: int):
    return LinuxProcess(pid) if os.name == "posix" and Path("/proc").is_dir() else psutil.Process(pid)


def system_processes():
    if os.name == "posix" and Path("/proc").is_dir():
        return (LinuxProcess(int(path.name)) for path in Path("/proc").iterdir() if path.name.isdigit())
    return (psutil.Process(pid) for pid in psutil.pids())


def inspect_processes(key: AccountKey, *, processes=None, process_factory=None) -> UsageFacts:
    """Ambiente ilegível e identidade trocada nunca provam ausência de uso."""
    facts = UsageFacts(complete=True)
    variable = "CODEX_HOME" if key.provider == Provider.CODEX else "CLAUDE_CONFIG_DIR"
    default = Path.home() / (".codex" if key.provider == Provider.CODEX else ".claude")
    try:
        for process in processes if processes is not None else system_processes():
            try:
                name = process.name().lower().removesuffix(".exe")
                if name not in _CLIENTS | _HOSTS:
                    continue
                before = process.create_time()
                argv = process.cmdline()
                import re
                names = {Path(item).name.lower().removesuffix(".exe") for item in argv}
                names.update(re.findall(r"\b(?:claude|codex|pi|omp)\b", " ".join(argv).lower()))
                pertinent = name in _CLIENTS or bool(names & _CLIENTS) or any(
                    "cano.py" in item or "hangar-cano" in item or "codex.js" in item or "claude-code" in item
                    for item in argv)
                if not pertinent:
                    continue
                env = process.environ()
                if not env:
                    facts.complete = False
                    continue
                home = env.get(variable)
                if key.provider == Provider.CODEX:
                    for index, argument in enumerate(argv):
                        if argument == "--codex-home":
                            home = argv[index + 1]
                        elif argument.startswith("--codex-home="):
                            home = argument.split("=", 1)[1]
                    if home == "":
                        raise ValueError("caminho de conta vazio no lançador")
                if not home:
                    # Ambiente pode conter as duas contas herdadas; só o provider pertinente usa o padrão.
                    expected = {"codex"} if key.provider == Provider.CODEX else {"claude", "pi", "omp"}
                    if not names.intersection(expected) and name not in expected:
                        continue
                    home = str(default)
                path = Path(home).expanduser()
                if not path.is_absolute():
                    path = Path(process.cwd()) / path
                owner = AccountKey.new(key.provider, path)
                after = (process_factory or system_process)(process.pid).create_time()
                if before != after:
                    facts.complete = False
                    continue
                if owner == key:
                    facts.pids.append(process.pid)
            except _GONE:
                continue
            except _UNREADABLE:
                facts.complete = False
    except _SCAN_ERRORS:
        facts.complete = False
    facts.pids = sorted(set(facts.pids))
    return facts


def terminal_sessions() -> set[str]:
    return set(terminal_instances())


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


def inspect_usage(key: AccountKey) -> UsageFacts:
    """Inclui sidecars sem filho iniciado: o nascimento adiado ainda usa a conta."""
    from app.adapters.claude_headless import sessions as headless_sessions
    from app.adapters.codex import sessions as codex_sessions

    facts = inspect_processes(key)
    try:
        import json
        from app import account_lifecycle
        births = account_lifecycle.default_lock_root() / "births"
        if births.exists():
            instances = None
            for path in births.iterdir():
                if path.suffix != ".json":
                    continue
                pending = json.loads(path.read_text(encoding="utf-8"))
                owner = AccountKey.new(pending["provider"], Path(pending["canonical_home"]))
                if not pending.get("name") or "instance" not in pending:
                    raise ValueError("registro de nascimento incompleto")
                if owner != key:
                    continue
                if instances is None:
                    instances = terminal_instances()
                if pending["instance"] is None:
                    # Sem identidade, ausência do nome pode ser apenas um rename.
                    facts.complete = False
                else:
                    facts.sessions.extend(name for name, instance in instances.items()
                                          if instance == pending["instance"])
        directory = (codex_sessions if key.provider == Provider.CODEX else headless_sessions)._dir()
        rows = []
        if directory.exists():
            for path in directory.iterdir():
                if path.suffix == ".json":
                    row = json.loads(path.read_text(encoding="utf-8"))
                    if not isinstance(row, dict) or not row.get("name"):
                        raise ValueError("sidecar de sessão incompleto")
                    rows.append(row)
        terminals = None
        for row in rows:
            home = row.get("codex_home") if key.provider == Provider.CODEX else row.get("config_dir")
            if not home and key.provider == Provider.CODEX:
                from app import codex_contas
                home = str(codex_contas.resolve_account(row.get("codex_account") or "default").home)
            home = home or str(Path.home() / ".claude")
            if AccountKey.new(key.provider, Path(home)) == key:
                if key.provider == Provider.CODEX and not row.get("headless") and not row.get("launching"):
                    if terminals is None:
                        terminals = terminal_sessions()
                    if row["name"] not in terminals:
                        continue
                facts.sessions.append(row["name"])
    except Exception:  # noqa: BLE001 — um leitor parcial não pode autorizar exclusão.
        facts.complete = False
    facts.sessions = sorted(set(facts.sessions))
    return facts

class PreparationJobs:
    """Trabalhos de configuração que continuam protegidos quando o chamador desaparece."""

    def __init__(self):
        self.jobs = {}
        self.stages = {}

    @staticmethod
    def validate(body):
        import json
        import re
        from pathlib import Path
        from app import runtime_coordinator, account_lifecycle, contas, codex_contas
        coordinator = runtime_coordinator.current()
        if coordinator is None or coordinator.instance != body["instance"]:
            raise ValueError("instância de preparo encerrada")
        key = account_lifecycle.AccountKey.new(body["key"]["provider"], Path(body["key"]["canonical_home"]))
        name = body["account_id"]
        if not isinstance(name, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,31}", name):
            raise ValueError("conta inválida")
        expected = (codex_contas.resolve_account(name).home if key.provider.value == "codex" and name == "default"
                    else Path.home() / f".{key.provider.value}-{name}")
        if key != account_lifecycle.AccountKey.new(key.provider, expected) or expected.is_symlink() or not expected.is_dir():
            raise ValueError("destino inválido")
        record = account_lifecycle.default_lock_root() / (key.digest + ".prepare.json")
        if record.is_symlink() or json.loads(record.read_text(encoding="utf-8")) != body:
            raise ValueError("operação de preparo substituída")
        if body["seed"]:
            if key.provider.value != "claude" or not (expected / ".hangar-account-pending").is_file() or (expected / contas.MARCADOR).exists():
                raise ValueError("semeadura sem cadastro pendente")
        elif key.provider.value == "claude":
            if not contas.e_conta(expected):
                raise ValueError("conta Claude inválida")
        elif codex_contas.resolve_account(name).home.resolve() != expected.resolve():
            raise ValueError("conta Codex inválida")
        cwd = body["cwd"]
        if cwd is not None and (not isinstance(cwd, str) or not Path(cwd).is_absolute() or not Path(cwd).is_dir()):
            raise ValueError("pasta de trabalho inválida")
        return key, expected

    async def start(self, body):
        import asyncio
        import re
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
        from pathlib import Path
        from app import account_lifecycle, codex_contas, codex_contas_sync, contas
        key = account_lifecycle.AccountKey.new(body["key"]["provider"], Path(body["key"]["canonical_home"]))

        async def owned():
            guard = await asyncio.to_thread(account_lifecycle.acquire, key, account_lifecycle.GuardMode.SHARED)
            with guard:
                _, target = self.validate(body)
                if key.provider.value == "claude":
                    def reconcile():
                        contas.compartilhado().mkdir(parents=True, exist_ok=True)
                        with contas._trava_compartilhada(), contas._trava(target):
                            self.validate(body)
                            return contas.prepare_configuration(target, seed=body["seed"])
                    return await account_lifecycle.complete_on_cancel(asyncio.to_thread(reconcile))
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
                if isinstance(source_result, dict) and source_result.get("estado") not in (None, "ok", "ocioso"):
                    if result["status"] == "ready":
                        result["status"] = "partial"
                    result.setdefault("issues", []).insert(0, {"code": "codex_account_source_sync_incomplete",
                                                               "params": {"status": str(source_result.get("estado"))}})
                return result
        try:
            return await account_lifecycle.complete_on_cancel(owned())
        except asyncio.CancelledError:
            return {"status": "error", "trust_pending": False,
                    "issues": [{"code": "account_prepare_cancelled", "params": {}}]}
        except Exception as exc:
            return {"status": "error", "trust_pending": False,
                    "issues": [{"code": "codex_account_prepare_failed" if key.provider.value == "codex" else "account_prepare_failed",
                                "params": {"error": type(exc).__name__}}]}

_preparation_transport = None


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


def publish_preparation_result(account, result):
    """Publica a operação completa no registro compartilhado com o coordenador Rust."""
    import json
    import os
    import tempfile
    from pathlib import Path
    from app import account_lifecycle, atomico
    if result.get("status") not in {"ready", "partial", "error"}:
        raise ValueError("resultado de preparo ainda não concluído")
    key = account_lifecycle.AccountKey.new("codex", account.home)
    directory = account_lifecycle.default_lock_root()
    target = directory / (key.digest + ".prepare-result.json")
    if directory.is_symlink() or target.is_symlink():
        raise ValueError("registro de preparo é um link")
    # O chamador conserva a reserva da conta até a publicação atômica do resultado.
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=directory,
                                     suffix=".tmp", delete=False) as temporary:
        try:
            os.chmod(temporary.name, 0o600)
            json.dump(result, temporary, ensure_ascii=False, allow_nan=False)
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary.close()
            Path(temporary.name).unlink(missing_ok=True)
            raise
    try:
        atomico.substituir(temporary.name, target)
    finally:
        Path(temporary.name).unlink(missing_ok=True)

def request_preparation(account, *, prepare=False, force=False, cwd=None):
    import json
    import urllib.request
    from app import runtime_coordinator, codex_contas
    coordinator = runtime_coordinator.current()
    if coordinator is None or getattr(coordinator, "mode", "python") == "python":
        return None
    config = _preparation_transport
    if config is None:
        raise codex_contas.AccountError(503, "account_prepare_bridge_unavailable", {})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request(f"http://{config[0]}/__hangar_server/accounts",
        data=json.dumps({"account_id": account.id, "prepare": prepare, "force": force, "cwd": cwd}).encode(),
        headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    try:
        with opener.open(request, timeout=15) as response:
            result = json.loads(response.read(64 * 1024))
        if not isinstance(result, dict) or result.get("status") not in {"idle", "running", "ready", "partial", "error"}:
            raise ValueError("resposta de preparo inválida")
        return result
    except (OSError, ValueError):
        raise codex_contas.AccountError(503, "account_prepare_bridge_unavailable", {}) from None

class ClaudeWindows:
    """Transporte auxiliar: só janela nativa e invalidação, sem decidir autenticação."""
    def __init__(self):
        import threading
        self.lock = threading.RLock()
        self.active = {}

    def run(self, body):
        import re
        from app import login_conta, conta_estado, runtime_coordinator
        if not isinstance(body, dict) or set(body) != {"instance", "key", "operation", "action", "code"}:
            raise ValueError("pedido inválido")
        coordinator = runtime_coordinator.current()
        if coordinator is None or body["instance"] != coordinator.instance:
            raise ValueError("instância inválida")
        operation, action, code = body["operation"], body["action"], body["code"]
        if not isinstance(operation, str) or not re.fullmatch("[a-f0-9]{32}", operation):
            raise ValueError("operação inválida")
        if action not in {"open", "read", "code", "close", "invalidate"}:
            raise ValueError("ação inválida")
        if action == "code":
            if not isinstance(code, str) or not code or len(code) > 4096 or any(c in code for c in ("\n", "\r", "\x00")):
                raise ValueError("código inválido")
        elif code is not None:
            raise ValueError("entrada inesperada")
        raw_key = body["key"]
        if not isinstance(raw_key, dict) or set(raw_key) != {"provider", "canonical_home"} or raw_key["provider"] != "claude":
            raise ValueError("conta inválida")
        if not isinstance(raw_key["canonical_home"], str):
            raise ValueError("caminho inválido")
        key = AccountKey.new("claude", Path(raw_key["canonical_home"]))
        from app.config import list_config_dirs
        if not any(AccountKey.new("claude", Path(c.path)) == key for c in list_config_dirs()):
            raise ValueError("conta ausente")
        name = "login-" + operation
        target = "term-" + name
        with self.lock:
            current = runtime_coordinator.current()
            if current is None or current.instance != body["instance"]:
                raise ValueError("instância inválida")
            previous = self.active.get(operation)
            if previous is not None and (previous[0] != key or (action != "close" and previous[1] != body["instance"])):
                raise ValueError("operação divergente")
            # A identidade é da operação: limpar uma tentativa antiga não toca na nova.
            if action == "close":
                login_conta._shell_matar(target)
                self.active.pop(operation, None)
            elif action == "open":
                previous = self.active.get(operation)
                if previous is not None and previous != (key, body["instance"]):
                    raise ValueError("operação divergente")
                if previous is None:
                    created = login_conta._shell_criar(name, str(key.canonical_home), config_dir=str(key.canonical_home))
                    if created != target:
                        login_conta._shell_matar(target)
                        raise RuntimeError("não consegui abrir a janela escondida")
                    self.active[operation] = (key, body["instance"])
                    try:
                        login_conta._shell_submeter(target, "claude auth login --claudeai")
                    except Exception:
                        login_conta._shell_matar(target)
                        self.active.pop(operation, None)
                        raise
            elif action == "invalidate":
                conta_estado.esquecer_conta(str(key.canonical_home))
            else:
                if self.active.get(operation) != (key, body["instance"]):
                    raise ValueError("operação ausente")
                if action == "read":
                    match = login_conta._URL_RE.search(login_conta._shell_ler(target))
                    return {"ok": True, "url": match.group(1) if match else None}
                if not login_conta._PROMPT_RE.search(login_conta._shell_ler(target)):
                    raise RuntimeError("a CLI não está aguardando o código de autorização")
                login_conta._shell_code(target, code)
        return {"ok": True}


claude_windows = ClaudeWindows()


def request_claude(action, *, label=None, path=None, code=None):
    """Encaminha consumidores Python ao dono Rust; pending nunca usa reserva Python."""
    import json
    import urllib.request
    import urllib.error
    from fastapi import HTTPException
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None or getattr(coordinator, "mode", "python") == "python":
        return None
    config = _preparation_transport
    if config is None:
        raise HTTPException(503, detail={"code": "account_auth_bridge_unavailable"})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request("http://" + config[0] + "/__hangar_server/accounts/claude",
        data=json.dumps({"action": action, "label": label, "path": path, "code": code}).encode(),
        headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    try:
        with opener.open(request, timeout=320 if action == "code" else 30) as response:
            return json.loads(response.read(256 * 1024))
    except urllib.error.HTTPError as error:
        try:
            detail = json.loads(error.read(16384)).get("detail")
        except (ValueError, OSError):
            detail = {"code": "account_auth_bridge_unavailable"}
        raise HTTPException(error.code, detail=detail) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail={"code": "account_auth_bridge_unavailable"}) from None


def request_codex(action, account, *, attempt_id=None, refresh=False):
    """Ponte de consumidores internos; pending recusa sem abrir um escritor Python."""
    import json
    import urllib.request
    import urllib.error
    from fastapi import HTTPException
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None or getattr(coordinator, "mode", "python") == "python":
        return None, False
    config = _preparation_transport
    if config is None:
        raise HTTPException(503, detail={"code": "account_auth_bridge_unavailable"})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request("http://" + config[0] + "/__hangar_server/accounts",
        data=json.dumps({"action": action, "account_id": account.id,
                         "attempt_id": attempt_id, "refresh": refresh}).encode(),
        headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    try:
        with opener.open(request, timeout=45) as response:
            return json.loads(response.read(256 * 1024)), True
    except urllib.error.HTTPError as error:
        try:
            detail = json.loads(error.read(16384)).get("detail")
        except (ValueError, OSError):
            detail = {"code": "account_auth_bridge_unavailable"}
        if isinstance(detail, dict) and "code" in detail:
            from app.codex_contas import AccountError
            raise AccountError(error.code, detail["code"], detail.get("params", {})) from None
        raise HTTPException(error.code, detail=detail) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail={"code": "account_auth_bridge_unavailable"}) from None


preparation_jobs = PreparationJobs()
