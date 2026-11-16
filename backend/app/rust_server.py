# backend/app/rust_server.py
"""Sobe e vigia o `hangar-server`, o processo Rust que atende a porta pública na frente do uvicorn.

O uvicorn escuta numa porta interna de loopback e o filho fica com a porta do app. Sem resposta em 10 s,
com outro protocolo ou com 3 quedas em 60 s, a porta fica sem dono e o filho é religado de tempos em
tempos (`RETRY_DELAYS`); o Python nunca a atende.
"""
from __future__ import annotations

import asyncio
import ipaddress
import json
import logging
import os
import re
import secrets
import socket
import subprocess
import time
import urllib.request
from collections.abc import Callable
from pathlib import Path

import uvicorn

from app import diag, log_paths, rust_bins

_log = logging.getLogger("hangar.rust_server")

HEALTH_PATH = "/__hangar_server/health"
# Versão do contrato interno (rotas /internal, canal privado do estado, ambiente). Tem de casar com o
# `protocol` da saúde (hangar_server::INTERNAL_PROTOCOL); outro número = o filho não serve.
RUST_SERVER_PROTOCOL = 112
START_TIMEOUT = 10.0
PROTECTED_SHUTDOWN_SECONDS = 120
CRASH_WINDOW = 60.0
MAX_CRASHES = 3
# Depois da desistência, espera antes de cada nova tentativa de subir o Rust (a última se repete).
RETRY_DELAYS = (30.0, 60.0, 120.0, 300.0, 600.0)
_POLL = 0.25

# Literal, e não `subprocess.CREATE_NO_WINDOW`: o atributo só existe no Windows.
_CREATE_NO_WINDOW = 0x08000000
# Pedido ainda aberto quando a parada começa: o uvicorn espera sem prazo antes do lifespan, e o
# systemd mata o processo no teto dele (10 s aqui), pulando a parada ordenada.
GRACEFUL_SHUTDOWN_S = 5


class Server(uvicorn.Server):
    """Uvicorn que fecha os SSE assim que o sinal de parada chega."""

    def handle_exit(self, sig, frame) -> None:
        try:
            from sse_starlette.sse import AppStatus
            AppStatus.should_exit = True        # só o sinal fecha os SSE; soltar a porta pública, não
        finally:
            super().handle_exit(sig, frame)     # falha no gancho não pode impedir a parada


def wanted_binary() -> Path | None:
    """O binário a subir, ou None quando não há um utilizável (o backend então não sobe)."""
    found = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if found is None:
        diag.registrar("hangar_server.reserva", "erro", codigo="sem_binario")
    else:
        from app import shared_auth
        from app.config import settings
        try:
            shared_auth.activate(found, settings.auth_token, 0)
        except (OSError, ValueError, AttributeError, shared_auth.Unavailable):
            _log.error("hangar-server sem par válido da biblioteca de autenticação")
            diag.registrar("hangar_server.reserva", "erro", codigo="auth_pair_invalid")
            return None
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


def _last_errors(path: Path, count: int = 3) -> str:
    """As últimas linhas ERROR/WARN do log do filho, sem a data: o pânico sai por elas (gancho do
    `tracing`), e o código de saída sozinho não diz por que ele caiu."""
    try:
        with path.open("rb") as f:
            f.seek(max(0, f.seek(0, os.SEEK_END) - 65536))
            tail = f.read().decode("utf-8", errors="replace").splitlines()
    except OSError as e:
        return f"log ilegível: {type(e).__name__}"
    kept = [line for line in tail if " ERROR " in line or " WARN " in line][-count:]
    return " | ".join(_reduce_error(line) for line in kept)


_FIELD = re.compile(r"\s[a-z_][a-z0-9_.]*=")
_KEPT_FIELD = re.compile(r"\s((?:code|panic)=(?:\"[^\"]{0,160}\"|\S{1,80}))")


def _reduce_error(line: str) -> str:
    """`2026-11-03T10:55:00.123Z  ERROR alvo: msg code=x detail="…"` vira `10:55:00 code=x`: a mensagem
    pode ser formatada com texto de conversa; ficam só os campos que nunca o levam."""
    head = line.split(maxsplit=1)[1] if line[:4].isdigit() and " " in line else line
    cut = _FIELD.search(head)
    kept = " ".join(_KEPT_FIELD.findall(head[cut.start():])) if cut else ""
    when = f"{line[11:19]} " if line[:4].isdigit() else ""
    return f"{when}{kept}".rstrip()


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
    from app.runtime_process import spawn_contained, record_path
    return spawn_contained([str(binary)], env=env, record_path=record_path())


class ProtocolMismatch(ValueError):
    """O binário fala outro contrato: religar não adianta, e o motivo tem de dizer isso."""


def _runtime_ready(proc, instance: str) -> dict:
    if proc.stdout is None:
        raise ValueError("partida sem cano de resposta")
    raw = proc.stdout.readline(4097)
    if not raw.endswith(b"\n") or len(raw) > 4096:
        raise ValueError("partida incompleta ou acima do teto")
    ready = json.loads(raw.decode("utf-8"))
    # Antes do formato: um binário de outro contrato pode ter outra linha de partida.
    if isinstance(ready, dict) and "protocol" in ready and ready["protocol"] != RUST_SERVER_PROTOCOL:
        raise ProtocolMismatch(ready["protocol"])
    if not isinstance(ready, dict) or set(ready) != {"type", "protocol", "instance", "port"}:
        raise ValueError("resposta de partida inválida")
    if ready["type"] != "runtime_ready" or ready["instance"] != instance:
        raise ValueError("resposta de outra instância")
    if type(ready["protocol"]) is not int or ready["protocol"] != RUST_SERVER_PROTOCOL:
        raise ProtocolMismatch(ready["protocol"])
    if type(ready["port"]) is not int or not 1 <= ready["port"] <= 65535:
        raise ValueError("porta privada inválida")
    return ready


def _close_stdin(proc: subprocess.Popen) -> None:
    # Nada é escrito no cano, então fechar não tem o que despejar e não falha.
    if proc.stdin is not None:
        proc.stdin.close()


# O da execução atual, para a tela de migração ler pid e binário sem guardar outra cópia.
_supervisor: "Supervisor | None" = None


def child() -> subprocess.Popen | None:
    return _supervisor.proc if _supervisor is not None else None


def current_binary() -> Path | None:
    return _supervisor.binary if _supervisor is not None else None


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
        self.attempt = 0        # tentativas de religar depois da desistência; 0 = o Rust nunca desistiu

    def _env(self) -> dict[str, str]:
        # Import tardio: internal_api puxa o app, que o uvicorn interno já carregou a esta altura.
        from app import internal_api

        secret = secrets.token_hex(32)
        # O segredo vai só no ambiente do filho; no os.environ ele vazaria para toda sessão que o
        # backend sobe. O require_internal lê da memória do módulo a cada pedido.
        internal_api.set_secret(secret)
        from app import pair
        from app.config import settings
        groups = {
                  "HANGAR_PAIR_ARCHIVE": str(pair._arquivo_dir())}
        from app.config import GUEST_PORT, _backend_config_base
        # Convites: o Rust é o único dono do arquivo e da porta. Pela internet quem chega é o Funnel
        # (127.0.0.1); com o backend aberto para a rede, o convite também vale na casa.
        guest = {"HANGAR_SHARES_FILE": str(Path(_backend_config_base()) / "shares.json")}
        if self.port != GUEST_PORT:
            host = "0.0.0.0" if self.host in ("0.0.0.0", "::") else "127.0.0.1"
            guest["HANGAR_SERVER_GUEST_LISTEN"] = listen_addr(host, GUEST_PORT)
        from app import shared_auth
        context = shared_auth.current()
        auth_env = context.child_env(self.binary) if context is not None else {}
        env = {k: v for k, v in os.environ.items() if k not in ("HANGAR_SERVER_GUEST_LISTEN", "HANGAR_AUTH_CONTEXT")}
        return {**env, **groups, **guest, **auth_env,
                "HANGAR_SERVER_LISTEN": listen_addr(self.host, self.port),
                "HANGAR_SERVER_UPSTREAM": f"127.0.0.1:{self.upstream_port}",
                "HANGAR_INTERNAL_SECRET": secret,
                "HANGAR_RUNTIME_INSTANCE": secrets.token_hex(16),
                # Os dois podem estar só no backend/.env, que o pydantic lê sem exportar.
                "CP_AUTH_TOKEN": self.token,
                "CP_FORWARDED_ALLOW_IPS": self.forwarded,
                "HANGAR_SERVER_LOG": str(server_log_path())}

    async def _start(self) -> str:
        """`up`, `died` (morreu subindo), `silent` (vivo e calado até o prazo) ou `protocol`."""
        if self.proc is not None:
            from app.runtime_process import cleanup
            await asyncio.to_thread(cleanup, self.proc)
            _close_stdin(self.proc)
        from app import shared_auth
        try:
            env = await asyncio.to_thread(self._env)
        except (OSError, ValueError, shared_auth.Unavailable):
            diag.registrar("hangar_server.partida", "erro", codigo="auth_pair_invalid")
            return "protocol"
        self.proc = _spawn(self.binary, env)
        try:
            await asyncio.wait_for(asyncio.to_thread(_runtime_ready, self.proc,
                                            env["HANGAR_RUNTIME_INSTANCE"]), START_TIMEOUT)
        except ProtocolMismatch as e:
            _log.error("hangar-server fala o protocolo %r; este backend fala %d", e.args[0], RUST_SERVER_PROTOCOL)
            diag.registrar("hangar_server.protocolo", "erro", esperado=RUST_SERVER_PROTOCOL, recebido=repr(e.args[0]))
            return "protocol"
        except (OSError, ValueError, asyncio.TimeoutError):
            diag.registrar("hangar_server.partida", "erro", codigo="runtime_invalido")
            return "silent" if self.proc.poll() is None else "died"
        deadline = time.monotonic() + START_TIMEOUT
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                return "died"
            health = await asyncio.to_thread(_health, self.host, self.port)
            if health is not None:
                got = health.get("protocol")
                if isinstance(got, bool) or got != RUST_SERVER_PROTOCOL:
                    _log.error("hangar-server fala o protocolo %r; este backend fala %d", got, RUST_SERVER_PROTOCOL)
                    diag.registrar("hangar_server.protocolo", "erro", esperado=RUST_SERVER_PROTOCOL, recebido=repr(got))
                    return "protocol"
                if self.proc.poll() is not None:
                    return "died"
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
                    return {"silent":"sem_resposta", "protocol":"protocolo"}[state]
                if state == "up" and not self.announced:
                    print(f"[hangar] hangar-server de pé em {listen_addr(self.host, self.port)}; "
                          f"o Python atende atrás dele em 127.0.0.1:{self.upstream_port}", flush=True)
                    diag.registrar("hangar_server.voltou" if self.attempt else "hangar_server.de_pe")
                    self.announced = True
                record_failed, up_at = False, time.monotonic()
                while state == "up" and self.proc.poll() is None:
                    await asyncio.sleep(_POLL)
                    if self.attempt and time.monotonic() - up_at >= CRASH_WINDOW:
                        self.attempt = 0            # de pé por uma janela inteira: a espera volta ao começo
                    from app.runtime_process import refresh_members
                    try:
                        await asyncio.to_thread(refresh_members, self.proc)
                        if record_failed:
                            _log.info("registro de contenção do hangar-server voltou a gravar")
                            diag.registrar("hangar_server.registro_voltou")
                        record_failed = False
                    except OSError as e:
                        # Disco cheio é transitório e o filho segue vivo: grava de novo na volta seguinte.
                        if not record_failed:
                            _log.warning("registro de contenção do hangar-server não gravou: %s", e)
                            diag.registrar("hangar_server.registro_falhou", "aviso", **diag.erro_campos(e))
                        record_failed = True
                # Parada normal (systemctl, Ctrl+C) leva o filho junto, no mesmo instante em que o uvicorn
                # recebe o sinal: dá um respiro para a flag dele subir e decide ANTES de qualquer ação.
                await asyncio.sleep(_POLL)
                if self.stopping():
                    return "parada"
                await self.deactivate_runtime(confirmed_dead=self.proc.poll() is not None)
                now = time.monotonic()
                crashes = [t for t in crashes if now - t < CRASH_WINDOW] + [now]
                _log.warning("hangar-server saiu (código %s), queda %d em %ds",
                             self.proc.returncode, len(crashes), int(CRASH_WINDOW))
                saida = await asyncio.to_thread(_last_errors, server_log_path())
                diag.registrar("hangar_server.caiu", "aviso", retorno=self.proc.returncode,
                               tentativa=len(crashes), saida=saida)
                if len(crashes) >= MAX_CRASHES:
                    return "quedas"
        except Exception as e:                           # noqa: BLE001 — a porta pública não fica sem dono
            # Parar no meio da subida corta a leitura da saúde: é a parada, não falha da vigia.
            if self.stopping():
                await self.stop()
                return "parada"
            _log.exception("a vigia do hangar-server falhou")
            # A linha `reserva` que vem depois só diz "erro"; a causa fica aqui.
            diag.registrar("hangar_server.vigia_falhou", "erro", **diag.erro_campos(e))
            await self.stop()
            return "erro"

    async def stop(self) -> None:
        proc = self.proc
        if proc is None:
            return
        if proc.poll() is None:
            # Fechar o cano é o pedido de saída que o binário entende em todo sistema.
            _close_stdin(proc)
            try:
                await asyncio.to_thread(proc.wait, PROTECTED_SHUTDOWN_SECONDS)
            except subprocess.TimeoutExpired:
                proc.kill()
                await asyncio.to_thread(proc.wait)
        _close_stdin(proc)
        await self.deactivate_runtime(confirmed_dead=proc.poll() is not None)

    async def deactivate_runtime(self, confirmed_dead: bool) -> None:
        """O filho morreu: as sessões ficam sem dono até o próximo subir."""
        if not confirmed_dead:
            raise RuntimeError("morte do Rust não confirmada")
        from app.runtime_process import cleanup
        if self.proc is not None:
            await asyncio.to_thread(cleanup, self.proc)



async def serve(server: uvicorn.Server, sockets: list[socket.socket], binary: Path, kw: dict, token: str) -> None:
    """O uvicorn interno e o hangar-server juntos, até a parada. O Rust que desiste fica sem dono
    da porta até a próxima tentativa (`RETRY_DELAYS`).

    `kw` é o da porta pública: a lista `forwarded_allow_ips` do dono vai ao filho."""
    from app import shared_auth
    await asyncio.to_thread(shared_auth.activate, binary, token, sockets[0].getsockname()[1])
    serving = asyncio.create_task(server.serve(sockets=sockets))
    # O filho só nasce com o uvicorn interno de pé: antes disso todo repasse dele daria 502.
    while not server.started and not serving.done():
        await asyncio.sleep(0.05)
    if serving.done():
        await serving
        return
    supervisor = Supervisor(binary, kw["host"], kw["port"], sockets[0].getsockname()[1], token,
                            kw["forwarded_allow_ips"], lambda: server.should_exit)
    global _supervisor
    _supervisor = supervisor
    # Religar o Rust não pode fechar os SSE do processo: só o sinal fecha (`Server.handle_exit`).
    from sse_starlette.sse import AppStatus
    AppStatus.disable_automatic_graceful_drain()
    while True:
        watch = asyncio.create_task(supervisor.run())
        await asyncio.wait({serving, watch}, return_when=asyncio.FIRST_COMPLETED)
        if serving.done():
            watch.cancel()
            await asyncio.gather(watch, return_exceptions=True)
            await supervisor.stop()
            await serving
            return
        reason = watch.result()
        if reason == "parada":                       # o uvicorn já está saindo
            await serving
            return
        _log.error("hangar-server desligado (%s); a porta %s fica sem dono até a próxima tentativa", reason, kw["port"])
        # O nome do evento é o que o relatório de diagnóstico do Rust colhe.
        diag.registrar("hangar_server.reserva", "erro", codigo=reason)
        supervisor.announced = False
        # O mesmo binário falha igual nestes: só se tenta de novo depois de ele mudar no disco.
        stamp = _stamp(binary) if reason in ("sem_resposta", "protocolo") else None
        retry = asyncio.create_task(_retry_wait(supervisor, stamp))
        await asyncio.wait({serving, retry}, return_when=asyncio.FIRST_COMPLETED)
        if serving.done():
            retry.cancel()
            await asyncio.gather(retry, return_exceptions=True)
            await serving
            return
        diag.registrar("hangar_server.religar", "aviso", tentativa=supervisor.attempt)


def _stamp(binary: Path) -> tuple | None:
    try:
        st = binary.stat()
    except OSError:
        return None
    return st.st_ino, st.st_size, st.st_mtime_ns


async def _retry_wait(supervisor: Supervisor, stamp: tuple | None) -> None:
    while True:
        supervisor.attempt += 1
        await asyncio.sleep(RETRY_DELAYS[min(supervisor.attempt, len(RETRY_DELAYS)) - 1])
        if stamp is None or _stamp(supervisor.binary) != stamp:
            return


def run(app: str, kw: dict, binary: Path, token: str, sockets: list[socket.socket]) -> int:
    """Bloqueia até o fim. Devolve o código de saída: 0 ou 3 (não subiu)."""
    # Só o uvicorn interno confia no 127.0.0.1 (o hangar-server); a porta pública segue com o `kw`.
    config = uvicorn.Config(app, **{**kw, "forwarded_allow_ips": trust_loopback(kw["forwarded_allow_ips"])})
    server = Server(config)
    try:
        asyncio.run(serve(server, sockets, binary, kw, token), loop_factory=config.get_loop_factory())
    finally:
        from app import shared_auth
        shared_auth.shutdown()
    return 0 if server.started else 3
