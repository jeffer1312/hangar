"""Ponte privada da lista: com o Rust de pé, a descoberta e o cache de resolução do transcript são
dele. Falha levanta, nunca devolve lista vazia: lista vazia é "ninguém vivo" para quem varre pares
e faz `prune`."""
import asyncio
import http.client
import ipaddress
import json
import logging
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

import pydantic

from app import tmux
from app.models import SessionInfo
from app.workspace_bridge import _opener

_log = logging.getLogger("hangar.list")
_config: tuple[str, str] | None = None
_MAX_RESPONSE = 32 * 1024 * 1024
# A produção espera capturas de pane; a descoberta, o `list-panes` (5 s no pior caso).
_TIMEOUT = 20


class ListBridgeError(RuntimeError):
    """Falha da ponte com código (`list_bridge_off`, `list_bridge_unavailable`, o código do Rust)."""
    def __init__(self, code: str, detail: str = ""):
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = None
    if address is None or secret is None:
        return
    host, port = address.rsplit(":", 1)
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 1 <= int(port) <= 65535:
        raise ValueError("invalid list address")
    _config = (address, secret)


def dirs_env() -> str:
    """Pastas que só o Python sabe resolver (`HANGAR_LIST_DIRS` do filho Rust), as mesmas da
    descoberta de hoje."""
    from app import codex_contas, omp_dirs
    from app.adapters.kimi.sessions import kimi_home
    from app.adapters.pi.sessions import sessions_root
    from app.config import settings
    home = Path.home()
    return json.dumps({
        "home": str(home),
        "claude": str(settings.projects_dir.parent),
        "codex_home": str(codex_contas.default_home()),
        "pi_sessions": str(sessions_root("pi")),
        "omp_config": str(home / (os.environ.get("PI_CONFIG_DIR") or ".omp")),
        "omp_agent": str(omp_dirs.agent_dir()),
        "kimi_home": str(kimi_home()),
    }, ensure_ascii=False)


_on_loop_seen: set[tuple[str, tuple]] = set()


def _warn_if_on_loop(operation: str) -> None:
    """No laço, a espera pelo Rust congela o backend inteiro. Não recusa, porque há chamadores
    indiretos que hoje funcionam; o diário aponta cada um, uma vez."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return
    try:
        caller = sys._getframe(2)
        # Chave barata primeiro: o texto da pilha lê o disco, e só a primeira vez de cada ponto precisa dele.
        frame, key = caller, []
        while frame is not None and len(key) < 8:
            key.append((frame.f_code, frame.f_lineno))
            frame = frame.f_back
        if (operation, tuple(key)) in _on_loop_seen:
            return
        _on_loop_seen.add((operation, tuple(key)))
        from app import diag, loop_monitor
        where = loop_monitor.compact_stack(caller)
        _log.warning("list bridge on the event loop op=%s at %s", operation, where)
        diag.registrar("lista.ponte_no_laco", "aviso", codigo=operation, pilha=where)
    except Exception:
        # O aviso nunca muda o resultado da chamada que ele descreve.
        _log.debug("aviso de ponte no laço falhou op=%s", operation, exc_info=True)


def _request(operation: str, arguments: dict, timeout: float = _TIMEOUT, report: bool = True):
    config = _config
    if config is None:
        raise ListBridgeError("list_bridge_off")
    _warn_if_on_loop(operation)
    try:
        data = json.dumps({"op": operation, "args": arguments}, ensure_ascii=False,
                          allow_nan=False).encode("utf-8")
    except ValueError as e:
        raise ListBridgeError("list_bridge_invalid", "argumentos") from e
    req = urllib.request.Request(f"http://{config[0]}/__hangar_server/list", data=data,
        headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    try:
        with _opener.open(req, timeout=timeout) as response:
            body = response.read(_MAX_RESPONSE + 1)
        if len(body) > _MAX_RESPONSE:
            raise ValueError("resposta grande demais")
        value = json.loads(body, parse_constant=lambda _: None)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("resposta sem ok")
    except (OSError, ValueError, urllib.error.URLError, http.client.HTTPException) as e:
        if report:
            _failed(operation, type(e).__name__)
        raise ListBridgeError("list_bridge_unavailable", type(e).__name__) from e
    if not value["ok"]:
        error = value.get("error") if isinstance(value.get("error"), dict) else {}
        code = str(error.get("code") or "list_bridge_invalid")
        detail = str(error.get("detail") or "")
        if report:
            _failed(operation, code)
        if code == "mux_unavailable":
            raise tmux.MuxIndisponivel(detail or code)
        raise ListBridgeError(code, detail)
    return value.get("result")


def _failed(operation: str, reason: str) -> None:
    from app import diag
    _log.warning("list bridge failed op=%s reason=%s", operation, reason)
    diag.registrar("lista.ponte", "aviso", operacao_rust=operation, motivo=reason)


def _rows(result) -> list[SessionInfo]:
    if not isinstance(result, list):
        raise ListBridgeError("list_bridge_invalid", "linhas")
    try:
        return [SessionInfo.model_validate(row) for row in result]
    except pydantic.ValidationError:
        _failed("linhas", "list_bridge_invalid")
        # Sem encadear: a mensagem do pydantic repete a linha, e ela carrega a última resposta.
        raise ListBridgeError("list_bridge_invalid", "linhas") from None


def discover(newer_than: float | None = None) -> list[SessionInfo]:
    """`registry.list()`. `newer_than`: a sessão foi criada há menos de 1 s, então só vale uma
    descoberta que começou depois deste instante (época)."""
    return _rows(_request("list.discover", {} if newer_than is None else {"newer_than": newer_than}))


def snapshot() -> list[SessionInfo]:
    return _rows(_request("list.snapshot", {}))


def invalidate() -> None:
    _request("list.invalidate", {})


def resolve(name: str, cwd: str, pid: int | None = None) -> tuple[str | None, bool]:
    result = _request("list.resolve", {"name": name, "cwd": cwd, "pid": pid})
    if (not isinstance(result, dict) or type(result.get("tracked")) is not bool
            or not isinstance(result.get("jsonl"), (str, type(None)))):
        raise ListBridgeError("list_bridge_invalid", "resolução")
    return result.get("jsonl"), result["tracked"]


def seed(name: str, jsonl: str) -> None:
    _request("list.seed", {"name": name, "jsonl": str(jsonl)})


def forget(name: str) -> None:
    _request("list.forget", {"name": name})


def endpoint() -> tuple[str, str] | None:
    """Endereço e segredo da porta privada do Rust, ou None com a ponte desligada."""
    return _config


def term_active(name: str) -> bool:
    result = _request("term.active", {"name": name}, timeout=5)
    if not isinstance(result, dict) or type(result.get("active")) is not bool:
        _failed("term.active", "list_bridge_invalid")
        raise ListBridgeError("list_bridge_invalid", "term.active")
    return result["active"]


def push_state_facts(name: str, facts: dict) -> None:
    """Empurrão de `state_facts`: prazo curto e sem diário aqui, porque quem envia registra uma vez
    por queda, não uma por envio."""
    result = _request("state.facts", {"name": name, "facts": facts}, timeout=2, report=False)
    # Ninguém observando lá é normal (o `Monitor` acabou antes de o interesse vencer); observando e
    # recusado é sequência que andou para trás.
    if not isinstance(result, dict) or result.get("watched") and not result.get("accepted"):
        raise ListBridgeError("state_facts_rejected")
