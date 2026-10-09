"""Hangar Connect neste PC: código de ligação, configs do frpc e do Caddy, binários e os dois processos."""
from __future__ import annotations

import asyncio
import base64
import binascii
import hashlib
import io
import json
import logging
import os
import platform
import re
import signal
import subprocess
import tarfile
import time
import urllib.error
import urllib.request
import uuid
import zipfile
from dataclasses import dataclass
from pathlib import Path

from app import atomico, connect_port, log_paths, procinfo
from app.connect_port import CONNECT_PORT

_log = logging.getLogger(__name__)

CADDY_PORT = 18443
# fullmatch, não ^…$: o `$` do Python aceita um \n no fim, que quebraria o TOML.
_NAME = re.compile(r"(?=.{1,253}\Z)[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+")
_TOKEN = re.compile(r"[A-Za-z0-9._~+/=-]{16,256}")


class ConnectError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


@dataclass(frozen=True)
class Code:
    server: str
    token: str
    host: str
    key: str | None = None


def folder() -> Path:
    return Path.home() / ".hangar" / "connect"


def parse_code(text: str) -> Code:
    # Os três valores vão crus para dentro do TOML e do Caddyfile: a regex é o que impede injeção.
    raw = text.strip()
    try:
        data = json.loads(base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)))
    except (binascii.Error, ValueError) as e:
        raise ConnectError(400, "erro_connect_codigo_invalido", "código de ligação ilegível") from e
    server, token, host, key = (data.get(k) if isinstance(data, dict) else None
                                for k in ("server", "token", "host", "key"))
    if not (isinstance(data, dict) and data.get("v") == 1
            and isinstance(server, str) and _NAME.fullmatch(server)
            and isinstance(host, str) and _NAME.fullmatch(host) and host.count(".") >= 2
            and isinstance(token, str) and _TOKEN.fullmatch(token)
            and (key is None or (isinstance(key, str) and _TOKEN.fullmatch(key)))):
        raise ConnectError(400, "erro_connect_codigo_invalido", "código de ligação incompleto")
    return Code(server, token, host, key)


def render_frpc(code: Code) -> str:
    return (
        f'serverAddr = "{code.server}"\nserverPort = 443\ntransport.protocol = "wss"\n'
        f'auth.method = "token"\nauth.token = "{code.token}"\nloginFailExit = false\n'
        + (f'metadatas.key = "{code.key}"\n' if code.key else "") +
        # Com multiplexação o frpc não manda Ping (padrão -1): sem isto, conta vencida seguiria no ar.
        'transport.heartbeatInterval = 30\n'
        'log.to = "console"\nlog.level = "warn"\n\n'
        f'[[proxies]]\nname = "{code.host}"\ntype = "https"\nlocalIP = "127.0.0.1"\n'
        f'localPort = {CADDY_PORT}\ncustomDomains = ["{code.host}"]\n')


def render_caddyfile(code: Code, data: Path) -> str:
    # TLS-ALPN-01 pela 443 repassada: a porta 80 da VPS é do traefik, o desafio HTTP nunca chegaria.
    return (
        "{\n\tadmin off\n\thttp_port 18080\n\thttps_port %d\n\tauto_https disable_redirects\n"
        '\tstorage file_system "%s"\n\tlog {\n\t\tlevel WARN\n\t}\n}\n\n'
        "%s {\n\tbind 127.0.0.1\n\ttls {\n\t\tissuer acme {\n\t\t\tdisable_http_challenge\n\t\t}\n\t}\n"
        "\treverse_proxy 127.0.0.1:%d\n}\n"
    ) % (CADDY_PORT, data.as_posix(), code.host, CONNECT_PORT)


def _state_file() -> Path:
    return folder() / "connect.json"


def read_state() -> dict:
    try:
        data = json.loads(_state_file().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _write_private(dest: Path, content: bytes) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(f"{dest.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(content)
    atomico.substituir(tmp, dest)


def write_state(state: dict) -> None:
    _write_private(_state_file(), json.dumps(state).encode())


_FRP = "0.71.0"
_CADDY = "2.11.6"
_ARCH = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}
_FRP_SHA256 = {
    ("linux", "amd64"): "84f27e39f11169f7adcef8e8b70c9329de17747b1f14dad9fb95eef5682ea716",
    ("linux", "arm64"): "f33c293c275d8fc68c654b6fba8f10b2551d6463d09a9fc9cffb7227eae82266",
    ("darwin", "amd64"): "1b1b4e2f1836e21e8733f1dddaacd4ed9ae67d7dbee39046b9d7b7eda6253637",
    ("darwin", "arm64"): "45be02b186860d375ed49a8941ae9569628a54bf14e67fc36b29c98c99dabcc6",
    ("windows", "amd64"): "9e5062e3e5cf07e67144a3a4acf175ef6a2486f3605dd6cf288bae34ab39819f",
}
_CADDY_SHA512 = {
    ("linux", "amd64"): "422771007d505ea97efd1177a4905b2c1a471cd426668f2ace3bcda3d8e30b11f9b1610bfb02c6ad60f2a795f56124f2f5eec6409c17d5a0dd4c21a11375fb94",
    ("linux", "arm64"): "bd228ea44b6b95720a0c2d7b62886e99cb4a0b05356fe6e058c4a155618c913377b18e686839ae7fcc6c2c05aea2d549fa91f25aa3ef8d43cdead117259577ed",
    ("darwin", "amd64"): "7e53d356e09cbc7ac8d7959d4515a2e0adebd66548c31fefda7d6771665263c70a30e1b8d3f37633c75b05454b8f3a97be38b60ef6c4d84fb1b0e117c7972c5f",
    ("darwin", "arm64"): "8b08d5a25ab612946b18b82c78fc32b194e69e576dd120cdca44f467ded2d6cb6fcfe899f05a71774f04b454e5cc9adcf08f8545c8d560bd92079ce015c8bf2e",
    ("windows", "amd64"): "e2626405400f15131886a6dc03883962bdc960fdcb1cdd0882f28af8821c9b370c83fdc60cb8a4deaf27008166f0e080cc7208e699dc0ce15a534ae75cf53d93",
}
_MAX_DOWNLOAD = 200 << 20


def _platform() -> tuple[str, str]:
    system = {"Linux": "linux", "Darwin": "darwin", "Windows": "windows"}.get(platform.system())
    arch = _ARCH.get(platform.machine().lower())
    if system is None or arch is None or (system, arch) not in _FRP_SHA256:
        raise ConnectError(400, "erro_connect_plataforma",
                           f"plataforma sem suporte: {platform.system()} {platform.machine()}")
    return system, arch


def _download(url: str) -> bytes:
    try:
        with urllib.request.urlopen(url, timeout=120) as r:
            data = r.read(_MAX_DOWNLOAD + 1)
    except (urllib.error.URLError, OSError) as e:
        raise ConnectError(502, "erro_connect_download", f"não consegui baixar {url}: {e}") from e
    if len(data) > _MAX_DOWNLOAD:
        raise ConnectError(502, "erro_connect_download", f"{url} maior que o esperado")
    return data


def _extract(data: bytes, member: str) -> bytes:
    try:
        if data[:2] == b"PK":
            with zipfile.ZipFile(io.BytesIO(data)) as z:
                return z.read(member)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as t:
            f = t.extractfile(member)
            if f is None:
                raise KeyError(member)
            return f.read()
    except (KeyError, zipfile.BadZipFile, tarfile.TarError) as e:
        raise ConnectError(502, "erro_connect_download", f"pacote sem {member}") from e


def binaries() -> dict[str, Path]:
    system, arch = _platform()
    exe = ".exe" if system == "windows" else ""
    ext = "zip" if system == "windows" else "tar.gz"
    frp = f"frp_{_FRP}_{system}_{arch}"
    caddy = f"caddy_{_CADDY}_{'mac' if system == 'darwin' else system}_{arch}.{ext}"
    packages = [
        ("frpc", _FRP, f"https://github.com/fatedier/frp/releases/download/v{_FRP}/{frp}.{ext}",
         f"{frp}/frpc{exe}", "sha256", _FRP_SHA256[(system, arch)]),
        ("caddy", _CADDY, f"https://github.com/caddyserver/caddy/releases/download/v{_CADDY}/{caddy}",
         f"caddy{exe}", "sha512", _CADDY_SHA512[(system, arch)]),
    ]
    out: dict[str, Path] = {}
    for name, version, url, member, algorithm, expected in packages:
        dest = folder() / "bin" / version / f"{name}{exe}"
        if not dest.exists():
            data = _download(url)
            # Hash conferido no pacote inteiro, antes de extrair: nada que não bata chega ao disco.
            if hashlib.new(algorithm, data).hexdigest() != expected:
                raise ConnectError(502, "erro_connect_download", f"{url} não bate com o hash esperado")
            content = _extract(data, member)
            dest.parent.mkdir(parents=True, exist_ok=True)
            tmp = dest.with_name(f"{dest.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}")
            tmp.write_bytes(content)
            tmp.chmod(0o755)
            atomico.substituir(tmp, dest)
        out[name] = dest
    return out


_WINDOWS_FLAGS = 0x00000200 | 0x08000000   # CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
_WAITS = (1, 2, 5, 10, 30)
_HEALTHY = 60.0


@dataclass
class _Child:
    name: str
    argv: list[str]
    proc: asyncio.subprocess.Process | None = None
    restarts: int = 0
    last_exit: int | None = None


_children: dict[str, _Child] = {}
_task: asyncio.Task | None = None
_error: str | None = None
# A subida do backend e um PUT da tela podem chegar juntos: sem fila, nasceriam dois de cada.
_lock = asyncio.Lock()


def _kill(pid: int) -> None:
    if os.name == "nt":
        exe = procinfo.taskkill_path()
        if exe:
            # 128 = já morreu; qualquer outro código só vai ao log, quem decide é o pid_vivo.
            subprocess.run([exe, "/T", "/F", "/PID", str(pid)], capture_output=True, timeout=10,
                           encoding="utf-8", errors="replace")
        return
    try:
        os.killpg(pid, signal.SIGTERM)          # start_new_session: o grupo tem o pid do filho
    except ProcessLookupError:
        pass


def _kill_logged(pid: int, name: str) -> None:
    # Falha ao matar não pode derrubar o supervisor calada: o outro filho ficaria sem dono.
    try:
        _kill(pid)
    except (OSError, subprocess.SubprocessError):
        _log.exception("connect: não consegui encerrar %s (pid %s)", name, pid)


async def _reap(proc: asyncio.subprocess.Process, name: str) -> None:
    _kill_logged(proc.pid, name)
    try:
        await asyncio.wait_for(proc.wait(), 5)
    except asyncio.TimeoutError:
        _log.warning("connect: %s não saiu em 5 s; forçando", name)
        try:
            proc.kill()
        except ProcessLookupError:
            pass


def _kill_leftover(name: str) -> None:
    # No Windows o filho sobrevive à morte do backend; sobrando, ele seguraria a 18443 e o nome no
    # frps. Só mata se o executável for um dos nossos (`bin/<versão>/<nome>`, de qualquer versão):
    # pid reaproveitado pode ser qualquer coisa.
    try:
        pid = int((folder() / f"{name}.pid").read_text())
    except (OSError, ValueError):
        return
    argv0 = procinfo._argv(pid)[:1] if procinfo.pid_vivo(pid) else []
    if argv0 and Path(argv0[0]).parent.parent == folder() / "bin" and Path(argv0[0]).stem == name:
        _kill_logged(pid, name)


async def _keep(child: _Child) -> None:
    log_dir = log_paths.base()
    while True:
        started = time.monotonic()
        try:
            log_dir.mkdir(parents=True, exist_ok=True)
            _kill_leftover(child.name)
            extra: dict = {"creationflags": _WINDOWS_FLAGS} if os.name == "nt" else {"start_new_session": True}
            # "ab": num laço de quedas, a causa da anterior continua no arquivo.
            with open(log_dir / f"connect-{child.name}.log", "ab") as out:
                spawn = asyncio.ensure_future(asyncio.create_subprocess_exec(
                    *child.argv, stdin=asyncio.subprocess.DEVNULL, stdout=out,
                    stderr=asyncio.subprocess.STDOUT, **extra))
                try:
                    child.proc = await asyncio.shield(spawn)
                except asyncio.CancelledError:
                    # Cancelado bem no nascimento: o processo existe, mas ainda sem pid gravado.
                    child.proc = await spawn
                    raise
            (folder() / f"{child.name}.pid").write_text(str(child.proc.pid))
            child.last_exit = await child.proc.wait()
        except Exception:
            _log.exception("connect: %s não subiu", child.name)
            child.last_exit = None
        finally:
            proc, child.proc = child.proc, None
            if proc is not None and proc.returncode is None:
                await _reap(proc, child.name)
        if time.monotonic() - started >= _HEALTHY:
            child.restarts = 0
        wait = _WAITS[min(child.restarts, len(_WAITS) - 1)]
        child.restarts += 1
        _log.warning("connect: %s saiu com %s; volta em %ss", child.name, child.last_exit, wait)
        await asyncio.sleep(wait)


async def _stop() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        await asyncio.gather(_task, return_exceptions=True)
        _task = None


async def stop() -> None:
    async with _lock:
        await _stop()


async def start() -> None:
    global _task, _error
    async with _lock:
        await _stop()
        state = read_state()
        if not state.get("enabled"):
            return
        if connect_port.unavailable:
            _error = connect_port.unavailable
            _log.warning("connect: não liguei (%s)", _error)
            return
        try:
            code = parse_code(state.get("code", ""))
            bins = await asyncio.to_thread(binaries)
            _write_private(folder() / "frpc.toml", render_frpc(code).encode())
            _write_private(folder() / "Caddyfile", render_caddyfile(code, folder() / "caddy").encode())
        except ConnectError as e:
            _error = e.msg
            _log.warning("connect: não liguei (%s)", e.msg)
            return
        except Exception as e:
            # Disco, download cortado (IncompleteRead não é OSError) ou o que for: sem isto a tela
            # ficaria em "Subindo…" para sempre, sem erro nenhum.
            _error = f"não liguei: {e}"
            _log.exception("connect: não liguei")
            return
        _error = None
        _children.clear()
        _children["caddy"] = _Child("caddy", [str(bins["caddy"]), "run", "--config", str(folder() / "Caddyfile"),
                                              "--adapter", "caddyfile"])
        _children["frpc"] = _Child("frpc", [str(bins["frpc"]), "-c", str(folder() / "frpc.toml")])

        async def run() -> None:
            async with asyncio.TaskGroup() as group:
                for child in _children.values():
                    group.create_task(_keep(child), name=f"connect-{child.name}")

        _task = asyncio.create_task(run(), name="connect")


def save(text: str) -> None:
    parse_code(text)
    write_state({"code": text.strip(), "enabled": True})


async def forget() -> None:
    global _error
    async with _lock:
        # Desligado primeiro no disco: se parar os filhos falhar, o próximo boot não religa.
        write_state({})
        await _stop()
        # Se a subida falhou antes do supervisor rodar, um filho de antes ainda pode estar publicando.
        for name in ("frpc", "caddy"):
            _kill_leftover(name)
        # O frpc.toml guarda o token: desligado, ele não fica no disco.
        for name in ("frpc.toml", "frpc.pid", "caddy.pid"):
            (folder() / name).unlink(missing_ok=True)
        _children.clear()
        _error = None


def status() -> dict:
    state = read_state()
    host = None
    if state.get("code"):
        try:
            host = parse_code(state["code"]).host
        except ConnectError:
            pass
    children = list(_children.items())          # a lista muda no loop; isto roda em thread
    error = _error
    if error is None and _task is not None and _task.done() and not _task.cancelled() and _task.exception():
        error = f"o supervisor parou: {_task.exception()}"
    if error is None:
        falling = [f"{n} caiu {c.restarts} vezes (saída {c.last_exit})" for n, c in children if c.restarts >= 3]
        if falling:
            error = "; ".join(falling) + "; veja connect-<nome>.log na pasta de logs do Hangar"
    return {
        "configured": bool(state.get("code")), "enabled": bool(state.get("enabled")),
        "host": host, "url": f"https://{host}" if host else None, "error": error,
        "processes": {n: {"running": c.proc is not None and c.proc.returncode is None,
                          "restarts": c.restarts, "last_exit": c.last_exit} for n, c in children},
    }
