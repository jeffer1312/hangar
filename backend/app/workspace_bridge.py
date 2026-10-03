"""Ponte privada de Git/arquivos. Falta de confirmação nunca repete uma escrita."""
import contextvars
import functools
import inspect
import ipaddress
import json
import logging
import os
import threading
import urllib.error
import urllib.request

_log = logging.getLogger("hangar.workspace")
_config: tuple[str, str] | None = None
_fallback = contextvars.ContextVar("workspace_fallback", default=False)
_slots = threading.BoundedSemaphore(8)
_MAX_RESPONSE = 32 * 1024 * 1024


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = None
    if address is None or secret is None:
        return
    host, port = address.rsplit(":", 1)
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 1 <= int(port) <= 65535:
        raise ValueError("invalid workspace address")
    _config = (address, secret)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


_opener = urllib.request.OpenerDirector()
for _handler in (urllib.request.ProxyHandler({}), urllib.request.HTTPHandler(),
                 urllib.request.HTTPDefaultErrorHandler(), urllib.request.HTTPErrorProcessor(), _NoRedirect()):
    _opener.add_handler(_handler)


def request(operation: str, arguments: dict, *, mutation: bool = False) -> dict | None:
    config = _config
    if config is None or _fallback.get() or not _slots.acquire(blocking=False):
        return None
    try:
        data = json.dumps({"op": operation, "args": arguments}, ensure_ascii=False,
                          allow_nan=False, default=os.fspath).encode("utf-8")
        req = urllib.request.Request(f"http://{config[0]}/__hangar_server/workspace", data=data,
            headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
        with _opener.open(req, timeout=160 if mutation else 50) as response:
            body = response.read(_MAX_RESPONSE + 1)
        if len(body) > _MAX_RESPONSE:
            raise ValueError("invalid workspace response")
        value = json.loads(body, parse_constant=lambda _: None)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("invalid workspace response")
        if value["ok"] and "result" not in value:
            raise ValueError("missing workspace result")
        if not value["ok"] and (not isinstance(value.get("error"), dict)
                or type(value["error"].get("status")) is not int or "detail" not in value["error"]):
            raise ValueError("invalid workspace error")
        return value
    except (OSError, ValueError, urllib.error.URLError):
        _log.warning("workspace bridge unavailable op=%s mutation=%s", operation, mutation)
        if mutation:
            return {"ok": False, "error": {"status": 503, "code": "workspace_action_uncertain",
                "detail": "Não foi possível confirmar o resultado. Confira o estado antes de repetir."}}
        return None
    finally:
        _slots.release()


def delegate(operation: str, exception, *, mutation=False, prepare=None, decode=None):
    """Mantém a assinatura e a reserva original, inclusive chamadas aninhadas."""
    def decorate(original):
        signature = inspect.signature(original)
        @functools.wraps(original)
        def wrapped(*args, **kwargs):
            bound = signature.bind(*args, **kwargs)
            bound.apply_defaults()
            arguments = dict(bound.arguments)
            if prepare is not None:
                arguments = prepare(arguments)
            changing = mutation
            if operation == "git_action":
                changing = arguments["action"] not in ("status", "log")
            result = request(operation, arguments, mutation=changing)
            if result is None:
                token = _fallback.set(True)
                try:
                    return original(*args, **kwargs)
                finally:
                    _fallback.reset(token)
            if not result["ok"]:
                failure = result["error"]
                if operation in {"read_file", "read_at", "write_file", "write_at", "list_dir", "search", "resolver"}:
                    raise exception(failure["status"], failure.get("code") or "erro_arq_lista_falhou", failure["detail"])
                raise exception(failure["status"], failure["detail"])
            value = result["result"]
            if decode is not None:
                return decode(value)
            if operation in {"head_info", "create_worktree", "cited_elsewhere"}:
                return tuple(value)
            return value
        return wrapped
    return decorate
