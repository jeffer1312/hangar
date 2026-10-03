# backend/app/rust_server.py
"""Sobe e vigia o `hangar-server`, o processo Rust que atende a porta pública na frente do uvicorn.

Com o binário, o uvicorn escuta numa porta interna de loopback e o filho fica com a porta do app.
Sem binário, sem resposta em 10 s, com outro protocolo ou com 3 quedas em 60 s, o Python volta a
atender a porta pública no mesmo processo, sem rodar o lifespan de novo.
"""
from __future__ import annotations

import asyncio
import ipaddress
import json
import logging
import os
import secrets
import socket
import subprocess
import sys
import time
import urllib.request
from collections.abc import Callable
from pathlib import Path

import uvicorn

from app import diag, diag_logging, log_paths, rust_bins, terminal_observer

_log = logging.getLogger("hangar.rust_server")

HEALTH_PATH = "/__hangar_server/health"
# Versão do contrato interno (rotas /internal, side-events, ambiente). Tem de casar com o
# `protocol` da saúde (hangar_server::INTERNAL_PROTOCOL); outro número = o Python atende sozinho.
RUST_SERVER_PROTOCOL = 3
START_TIMEOUT = 10.0
CRASH_WINDOW = 60.0
MAX_CRASHES = 3
_POLL = 0.25

# Literal, e não `subprocess.CREATE_NO_WINDOW`: o atributo só existe no Windows.
_CREATE_NO_WINDOW = 0x08000000


def wanted_binary(enabled: bool) -> Path | None:
    """O binário a subir, ou None quando o Python atende a porta pública sozinho."""
    if not enabled:
        return None
    found = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if found is None:
        diag.registrar("hangar_server.reserva", "aviso", codigo="sem_binario")
    return found


def listen_addr(host: str, port: int) -> str:
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def ip_literal(host: str) -> str:
    """O lado Rust só entende IP:porta; `localhost` e afins viram o IP que o bind do Python usaria."""
    try:
        ipaddress.ip_address(host)
        return host
    except ValueError:
        pass
    for family in (socket.AF_INET, socket.AF_INET6):
        try:
            return socket.getaddrinfo(host, None, family, socket.SOCK_STREAM)[0][4][0]
        except OSError:
            continue
    return host        # sem resolver, o filho recusa a config e a reserva do Python assume


def trust_loopback(ips: str) -> str:
    """Todo pedido chega ao uvicorn interno vindo do hangar-server em 127.0.0.1: sem confiar nele,
    o IP real se perde e a LAN passa por loopback (isenta do limite de tentativas)."""
    parts = [p.strip() for p in ips.split(",")]
    return ips if "127.0.0.1" in parts or "*" in parts else f"{ips},127.0.0.1"


def server_log_path() -> Path:
    folder = log_paths.base() / "privado"
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    return folder / "hangar-server.log"


def _health(host: str, port: int) -> dict | None:
    """Corpo da saúde quando ela responde `ok`; None enquanto não responde."""
    # Quem escuta em todas as interfaces responde no loopback da mesma família.
    probe = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(host, host)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(f"http://{listen_addr(probe, port)}{HEALTH_PATH}", timeout=1) as r:
            body = json.loads(r.read() or b"{}")
    except (OSError, ValueError):
        return None
    return body if isinstance(body, dict) and body.get("ok") is True else None


def _spawn(binary: Path, env: dict[str, str]) -> subprocess.Popen:
    # stdin=PIPE: o Popen guardado segura a ponta de escrita enquanto o filho vive. Quando o
    # Python morre, de qualquer jeito e em qualquer sistema, ela fecha e o binário sai.
    kw: dict = {"creationflags": _CREATE_NO_WINDOW} if sys.platform == "win32" else {}
    return subprocess.Popen([str(binary)], env=env, stdin=subprocess.PIPE,
                            stdout=subprocess.DEVNULL, **kw)


def _close_stdin(proc: subprocess.Popen) -> None:
    # Nada é escrito no cano, então fechar não tem o que despejar e não falha.
    if proc.stdin is not None:
        proc.stdin.close()


class Supervisor:
    """Um filho por vez; religa a cada queda até desistir."""

    def __init__(self, binary: Path, host: str, port: int, upstream_port: int, token: str,
                 forwarded: str, stopping: Callable[[], bool]):
        self.binary = binary
        self.stopping = stopping
        self.host = ip_literal(host)
        self.port = port
        self.upstream_port = upstream_port
        self.token = token
        self.forwarded = forwarded
        self.proc: subprocess.Popen | None = None
        self.announced = False

    def _env(self) -> dict[str, str]:
        # Import tardio: internal_api puxa o app, que o uvicorn interno já carregou a esta altura.
        from app import internal_api

        secret = secrets.token_hex(32)
        # O segredo vai só no ambiente do filho; no os.environ ele vazaria para toda sessão que o
        # backend sobe. O require_internal lê da memória do módulo a cada pedido.
        internal_api.set_secret(secret)
        return {**os.environ,
                "HANGAR_SERVER_LISTEN": listen_addr(self.host, self.port),
                "HANGAR_SERVER_UPSTREAM": f"127.0.0.1:{self.upstream_port}",
                "HANGAR_INTERNAL_SECRET": secret,
                # Os dois podem estar só no backend/.env, que o pydantic lê sem exportar.
                "CP_AUTH_TOKEN": self.token,
                "CP_FORWARDED_ALLOW_IPS": self.forwarded,
                "HANGAR_SERVER_LOG": str(server_log_path())}

    async def _start(self) -> str:
        """`up`, `died` (morreu subindo), `silent` (vivo e calado até o prazo) ou `protocol`."""
        terminal_observer.configure(None, None)
        if self.proc is not None:
            _close_stdin(self.proc)                     # o anterior já saiu
        env = self._env()
        self.proc = _spawn(self.binary, env)
        deadline = time.monotonic() + START_TIMEOUT
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                return "died"
            health = await asyncio.to_thread(_health, self.host, self.port)
            if health is not None:
                got = health.get("protocol")
                if isinstance(got, bool) or got != RUST_SERVER_PROTOCOL:
                    _log.error("hangar-server fala o protocolo %r; este backend fala %d",
                               got, RUST_SERVER_PROTOCOL)
                    diag.registrar("hangar_server.protocolo", "erro",
                                   esperado=RUST_SERVER_PROTOCOL, recebido=got)
                    return "protocol"
                if self.proc.poll() is not None:
                    return "died"
                address = health.get("terminal_address")
                try:
                    if address is None:
                        raise ValueError("missing terminal address")
                    terminal_observer.configure(address, env["HANGAR_INTERNAL_SECRET"])
                except ValueError:
                    _log.warning("terminal observer address unavailable; using Python")
                    diag.registrar("terminal_observer.reserva", "aviso", codigo="endereco_invalido")
                return "up"
            await asyncio.sleep(_POLL)
        return "silent"

    async def run(self) -> str:
        """Mantém o filho de pé. Só volta quando desiste, com o motivo que vai pro diário."""
        crashes: list[float] = []
        try:
            while True:
                state = await self._start()
                if state in ("silent", "protocol") and self.stopping():
                    await self.stop()
                    return "parada"
                if state in ("silent", "protocol"):
                    # Religar não adianta: o mesmo binário volta calado ou com o mesmo protocolo.
                    await self.stop()
                    return "sem_resposta" if state == "silent" else "protocolo"
                if state == "up" and not self.announced:
                    self.announced = True
                    print(f"[hangar] hangar-server de pé em {listen_addr(self.host, self.port)}; "
                          f"o Python atende atrás dele em 127.0.0.1:{self.upstream_port}", flush=True)
                    diag.registrar("hangar_server.de_pe")
                while state == "up" and self.proc.poll() is None:
                    await asyncio.sleep(_POLL)
                terminal_observer.configure(None, None)
                # Parada normal (systemctl, Ctrl+C) leva o filho junto, no mesmo instante em que o uvicorn
                # recebe o sinal: dá um respiro para a flag dele subir antes de contar queda.
                await asyncio.sleep(_POLL)
                if self.stopping():
                    return "parada"
                now = time.monotonic()
                crashes = [t for t in crashes if now - t < CRASH_WINDOW] + [now]
                _log.warning("hangar-server saiu (código %s), queda %d em %ds",
                             self.proc.returncode, len(crashes), int(CRASH_WINDOW))
                diag.registrar("hangar_server.caiu", "aviso", retorno=self.proc.returncode,
                               tentativa=len(crashes))
                if len(crashes) >= MAX_CRASHES:
                    return "quedas"
        except Exception as e:                           # noqa: BLE001 — a porta pública não fica sem dono
            _log.exception("a vigia do hangar-server falhou")
            # A linha `reserva` que vem depois só diz "erro"; a causa fica aqui.
            diag.registrar("hangar_server.vigia_falhou", "erro", **diag.erro_campos(e))
            await self.stop()
            return "erro"

    async def stop(self) -> None:
        terminal_observer.configure(None, None)
        proc = self.proc
        if proc is None:
            return
        if proc.poll() is None:
            # Fechar o cano é o pedido de saída que o binário entende em todo sistema.
            _close_stdin(proc)
            try:
                await asyncio.to_thread(proc.wait, 5)
            except subprocess.TimeoutExpired:
                proc.kill()
                await asyncio.to_thread(proc.wait)
        _close_stdin(proc)


async def serve(server: uvicorn.Server, sockets: list[socket.socket], binary: Path, kw: dict,
                token: str, bind_public: Callable[[], socket.socket]) -> bool:
    """O uvicorn interno e o hangar-server juntos. `False` = a porta pública ficou sem dono.

    `kw` é o da porta pública: a lista `forwarded_allow_ips` do dono vai ao filho e à reserva."""
    serving = asyncio.create_task(server.serve(sockets=sockets))
    # O filho só nasce com o uvicorn interno de pé: antes disso todo repasse dele daria 502.
    while not server.started and not serving.done():
        await asyncio.sleep(0.05)
    if serving.done():
        await serving
        return True
    supervisor = Supervisor(binary, kw["host"], kw["port"], sockets[0].getsockname()[1], token,
                            kw["forwarded_allow_ips"], lambda: server.should_exit)
    watch = asyncio.create_task(supervisor.run())
    await asyncio.wait({serving, watch}, return_when=asyncio.FIRST_COMPLETED)
    if serving.done():
        watch.cancel()
        await asyncio.gather(watch, return_exceptions=True)
        await supervisor.stop()
        await serving
        return True
    reason = watch.result()
    if reason == "parada":                       # o uvicorn já está saindo: nada a assumir
        await serving
        return True
    return await _take_over(server, serving, reason, kw, bind_public)


async def _take_over(server: uvicorn.Server, serving: asyncio.Task, reason: str, kw: dict,
                     bind_public: Callable[[], socket.socket]) -> bool:
    """O Python passa a atender a porta pública até o fim do processo."""
    _log.error("hangar-server desligado (%s); o Python assume a porta %s", reason, kw["port"])
    diag.registrar("hangar_server.reserva", "erro", codigo=reason)
    try:
        sock = bind_public()
    except OSError as e:
        _log.error("a porta %s ficou sem dono: %s", kw["port"], e)
        diag.registrar("hangar_server.reserva", "erro", codigo="porta_ocupada", **diag.erro_campos(e))
        server.should_exit = True
        await serving
        return False
    # lifespan="off": ele já rodou no servidor interno, e rodar de novo duplicaria watchers e hooks.
    public = uvicorn.Server(uvicorn.Config(server.config.app, **{**kw, "lifespan": "off"}))
    # O Config novo refaz o logging do uvicorn e tira dele os handlers do diário.
    diag_logging.instalar()
    public_task = asyncio.create_task(public.serve(sockets=[sock]))
    await asyncio.wait({serving, public_task}, return_when=asyncio.FIRST_COMPLETED)
    server.should_exit = True
    public.should_exit = True
    await asyncio.gather(serving, public_task)
    return True


def run(app: str, kw: dict, binary: Path, token: str, sockets: list[socket.socket],
        bind_public: Callable[[], socket.socket]) -> int:
    """Bloqueia até o fim. Devolve o código de saída: 0, 1 (porta sem dono) ou 3 (não subiu)."""
    # Só o uvicorn interno confia no 127.0.0.1 (o hangar-server); a porta pública segue com o `kw`.
    config = uvicorn.Config(app, **{**kw, "forwarded_allow_ips": trust_loopback(kw["forwarded_allow_ips"])})
    server = uvicorn.Server(config)
    public_ok = asyncio.run(serve(server, sockets, binary, kw, token, bind_public),
                            loop_factory=config.get_loop_factory())
    if not server.started:
        return 3
    return 0 if public_ok else 1
