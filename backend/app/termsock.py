"""Porta de entrada do terminal real: confere quem chega e liga os bytes ao painel do Rust.

O PTY é do Rust (`/__hangar_server/term`); aqui só a autenticação, a sessão e o cano.
"""
import asyncio
import contextlib
import logging
import secrets
import time
from typing import Optional

from fastapi import WebSocket
from starlette.websockets import WebSocketDisconnect
from uvicorn.protocols.utils import ClientDisconnected

from app import tmux
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings
from app.guest_header import guest_of

_log = logging.getLogger(__name__)


# Teto da fila de saida (bytes acumulados esperando o `send_bytes`): sem isso um cliente lento
# (rede ruim, aba em segundo plano) deixa a fila crescer sem limite num `cat` grande — a task
# escritora unica so resolve a ORDEM dos bytes, nao o volume (achado da revisao).
_SAIDA_MAX = 1 << 20


async def term_ws(ws: WebSocket, name: str, resolve=None) -> None:
    """Porta de entrada -> painel do Rust. Autenticação, Origin, sessão existe e clamp de
    cols/rows ficam aqui, uma vez só: é o caminho que abre um shell completo."""
    prep = await _porta_de_entrada(ws, name, resolve)
    if prep is None:
        return                                   # ja fechou o ws com o codigo certo
    cols, rows, name = prep
    await _motor_rust(ws, name, cols, rows)


async def _porta_de_entrada(ws: WebSocket, name: str, resolve=None) -> Optional[tuple[int, int, str]]:
    """Trava e validação. Devolve (cols, rows, alvo tmux) ou None se já
    recusou. `resolve` troca o alvo (terminal de atalho da sessao) e so roda depois do token."""
    host = ws.client.host if ws.client else ""
    agora = time.time()
    tok = ws.query_params.get("token", "")
    from app import shared_auth
    decision = await shared_auth.websocket(ws, shared_auth.TERMINAL_WS,
                                          verified_guest=guest_of(ws) is not None)
    if decision is False:
        return None
    if decision is not None and decision.status not in (shared_auth.OWNER, shared_auth.GUEST):
        await ws.close(code=1008)
        return None
    if decision is None and _blocked(host, agora):
        await ws.close(code=1008)
        return                                   # NAO registra: registrar aqui estende o bloqueio
    # Convidado ja passou pelo porteiro (token dele + a sessao dele); a Origin e a do app DELE.
    if guest_of(ws) is None:
        # compare_digest, nao `!=` de string (mesmo cuidado do require_auth, auth.py:104): `!=` sai fora
        # na primeira letra diferente e vira canal lateral de tempo. Este e o endpoint que abre um shell
        # completo. `.encode()` tambem evita o TypeError do compare_digest com string nao-ASCII.
        dono = (decision.status == shared_auth.OWNER if decision is not None else
                bool(settings.auth_token) and secrets.compare_digest(tok.encode(), settings.auth_token.encode()))
        if not dono:
            if host not in _LOOPBACK:            # mesma isencao do require_auth (auth.py:46)
                _record_fail(host, agora)
            await ws.close(code=1008)            # fecha SEM accept: o PTY nunca chega a nascer
            return
    if resolve is not None:
        try:
            name = await asyncio.to_thread(resolve)
        except tmux.MuxIndisponivel:
            # Sem resposta nao da pra dizer que o terminal nao existe: o cliente tenta de novo.
            _log.warning("termsock: multiplexador sem resposta ao resolver o terminal")
            await ws.close(code=1013, reason="multiplexador indisponivel")
            return None
        if not name:
            await ws.close(code=1008, reason="terminal nao existe")
            return None
    if not await asyncio.to_thread(tmux.has_session, name):
        await ws.close(code=1008, reason="sessao nao existe")
        return

    try:
        cols = max(20, min(500, int(ws.query_params.get("cols", "80"))))
        rows = max(5, min(200, int(ws.query_params.get("rows", "24"))))
    except ValueError:
        await ws.close(code=1008, reason="cols/rows invalidos")
        return None
    return cols, rows, name


async def _recusa_ponte(ws: WebSocket, name: str, codigo: str, **campos) -> None:
    """Sem o Rust, nenhum PTY abre aqui: 1013 (o cliente tenta de novo) e o código no diário."""
    from app import diag
    _log.warning("termsock: %r sem o terminal do Rust (%s %s)", name, codigo, campos)
    diag.registrar("terminal.ponte", "erro", codigo=codigo, sessao=name, **campos)
    await ws.accept()
    await ws.close(code=1013, reason="terminal indisponivel")


async def _motor_rust(ws: WebSocket, name: str, cols: int, rows: int) -> None:
    """Liga os bytes do cliente ao painel do Rust (`/__hangar_server/term` na porta privada).

    A porta de entrada já rodou aqui; o Rust recebe só o alvo conferido e tem o painel único de
    todas as portas. Fechamento do Rust (1000 "outra conexao assumiu", 1011, 1013) chega igual ao
    cliente; cliente que sai fecha o lado do Rust, que desmonta.
    """
    from urllib.parse import urlencode
    from websockets.asyncio.client import connect
    from websockets.exceptions import ConnectionClosed, InvalidStatus
    from websockets.protocol import State
    from app import diag, list_bridge
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        await _recusa_ponte(ws, name, "terminal_bridge_off")
        return
    address, secret = endpoint
    url = f"ws://{address}/__hangar_server/term?" + urlencode({"target": name, "cols": cols, "rows": rows})
    try:
        # Sem proxy do ambiente, sem compressão e sem ping: o Rust pinga este lado, e o uvicorn
        # pinga o cliente. Quadros do Rust têm até 64 KiB; o teto é o do cliente nativo.
        upstream = await connect(url, additional_headers={"x-hangar-internal": secret}, proxy=None,
                                 compression=None, ping_interval=None, open_timeout=5,
                                 close_timeout=2, max_size=_SAIDA_MAX)
    except InvalidStatus as e:
        status = e.response.status_code
        # 403 é a sessão que morreu depois da porta de entrada: como o `has_session` daqui.
        if status == 403:
            await ws.close(code=1008, reason="sessao nao existe")
            return
        await _recusa_ponte(ws, name, "terminal_bridge_refused", status=status)
        return
    except Exception as e:                       # noqa: BLE001 — qualquer falha da ponte vira código
        codigo = "terminal_bridge_timeout" if isinstance(e, TimeoutError) else "terminal_bridge_unavailable"
        await _recusa_ponte(ws, name, codigo, tipo=type(e).__name__)
        return

    async def do_cliente():
        while True:
            msg = await ws.receive()
            if msg["type"] == "websocket.disconnect":
                return
            if (b := msg.get("bytes")) is not None:
                await upstream.send(b)
            elif (t := msg.get("text")) is not None:
                await upstream.send(t)

    async def do_rust():
        async for dados in upstream:
            if isinstance(dados, bytes):
                await ws.send_bytes(dados)
            else:
                await ws.send_text(dados)

    tarefas: set[asyncio.Future] = set()
    rust_fechou = False
    try:
        await ws.accept()
        _log.info("termsock: %r ligado ao terminal do Rust (%dx%d)", name, cols, rows)
        tarefas = {asyncio.ensure_future(do_cliente()), asyncio.ensure_future(do_rust())}
        await asyncio.wait(tarefas, return_when=asyncio.FIRST_COMPLETED)
    finally:
        # Cancelamento que chega durante a limpeza interrompe qualquer `await` daqui; o `abort` do
        # fim roda mesmo assim, senão o Rust fica com a conexão do repasse presa.
        try:
            for t in tarefas:
                t.cancel()
            if tarefas:
                # `wait`, não `await` em cada uma: o cancelamento desta task (convidado revogado)
                # segue valendo em vez de ser engolido junto com o das filhas.
                await asyncio.wait(tarefas)
            for t in tarefas:
                erro_t = None if t.cancelled() else t.exception()
                if erro_t is not None and not isinstance(
                        erro_t, (ConnectionClosed, WebSocketDisconnect, ClientDisconnected)):
                    _log.error("termsock: %r — repasse terminou com %s", name, type(erro_t).__name__)
            # O Rust fechou primeiro: o código dele vale, quem terminou antes aqui não importa.
            rust_fechou = upstream.state is not State.OPEN
            await upstream.close()
        finally:
            if upstream.state is not State.CLOSED:
                upstream.transport.abort()
    if rust_fechou:
        code, reason = upstream.close_code, upstream.close_reason or ""
        if code in (None, 1005, 1006):
            # Sem código: o painel do outro lado caiu sem fechar.
            diag.registrar("terminal.ponte", "erro", codigo="terminal_bridge_dropped", sessao=name)
            code, reason = 1011, "terminal do Rust caiu"
        with contextlib.suppress(RuntimeError, WebSocketDisconnect, ClientDisconnected):
            await ws.close(code=code, reason=reason)
    _log.info("termsock: %r desligado do terminal do Rust", name)
