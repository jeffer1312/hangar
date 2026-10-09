"""Ponte privada dos grupos: no modo `rust`/`pending` só o Rust grava `.hangar-pair`. Registry, par
externo, MCP e as rotas que chegam pelas portas do Python pedem aqui; o Python só lê o arquivo."""
import asyncio
import http.client
import json
import logging
import urllib.error
import urllib.request

_log = logging.getLogger("hangar.groups")
_config: tuple[str, str] | None = None
# Com o Rust esperado, os grupos são dele desde o início: antes da primeira saúde ele já atende
# `/pair` e varre, e o Python escrevendo ali seria o segundo escritor. Só a saúde com
# `groups: false` devolve ao Python; desistir do Rust já leva o modo a `python`.
_capable = True
_MAX_RESPONSE = 4 * 1024 * 1024
# A rota `/pair` entre máquinas pode esperar a outra máquina (16 s), a limpeza dela e a entrega.
_TIMEOUT = 90
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class GroupsBridgeError(RuntimeError):
    """Falha da ponte com código (`groups_bridge_off`, `groups_runtime_starting`, o código do Rust)."""
    def __init__(self, code: str, detail: str = ""):
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code, self.detail = code, detail


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = (address, secret) if address and secret else None


def set_capable(capable: bool) -> None:
    global _capable
    _capable = capable


def rust_owns_groups() -> bool:
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    return owner is not None and owner.mode != "python" and _capable


def _await_rust() -> None:
    """No `pending` espera o desfecho até `PENDING_WAIT_S`; depois disso, ou na reserva, é erro."""
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    if owner is not None and owner.mode == "pending" and not runtime_coordinator._mode_bypass.get():
        from app.runtime_adapter import run_sync
        try:
            run_sync(owner.await_mode, owner.loop)
        except (runtime_coordinator.RuntimeStarting, TimeoutError) as e:
            raise GroupsBridgeError("groups_runtime_starting") from e
        except RuntimeError as e:
            raise GroupsBridgeError("groups_wait_on_loop") from e
    if owner is None or owner.mode == "python":
        raise GroupsBridgeError("groups_runtime_python")


def call(op: str, **args) -> dict:
    """Bloqueia (nunca no laço: o Rust chama de volta as rotas internas do Python)."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        pass
    else:
        raise GroupsBridgeError("groups_wait_on_loop")
    _await_rust()
    config = _config
    if config is None:
        raise GroupsBridgeError("groups_bridge_off")
    data = json.dumps({"op": op, "args": args}, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(f"http://{config[0]}/__hangar_server/groups", data=data,
        headers={"content-type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    try:
        with _opener.open(req, timeout=_TIMEOUT) as response:
            body = response.read(_MAX_RESPONSE + 1)
        if len(body) > _MAX_RESPONSE:
            raise ValueError("resposta grande demais")
        value = json.loads(body)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("resposta sem ok")
    except (OSError, ValueError, urllib.error.URLError, http.client.HTTPException) as e:
        _failed(op, type(e).__name__)
        raise GroupsBridgeError("groups_bridge_unavailable", type(e).__name__) from e
    if not value["ok"]:
        error = value.get("error") if isinstance(value.get("error"), dict) else {}
        code = str(error.get("code") or "groups_bridge_invalid")
        _failed(op, code)
        raise GroupsBridgeError(code, str(error.get("detail") or ""))
    result = value.get("result")
    if not isinstance(result, dict):
        raise GroupsBridgeError("groups_bridge_invalid", "resultado")
    return result


def _failed(op: str, reason: str) -> None:
    from app import diag
    _log.warning("groups bridge failed op=%s reason=%s", op, reason)
    diag.registrar("grupos.ponte", "aviso", operacao_rust=op, motivo=reason)
