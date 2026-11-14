"""Ponte privada dos grupos: só o Rust grava `.hangar-pair`. O que chega às portas do Python (rotas
da 8768, fechamento pelo Connect, renomear, associar o `orq`) pede aqui, num sentido; o Python só
lê o arquivo."""
import http.client
import json
import logging
import urllib.error
import urllib.request

_log = logging.getLogger("hangar.groups")
_config: tuple[str, str] | None = None
_MAX_RESPONSE = 4 * 1024 * 1024
# A rota `/pair` entre máquinas pode esperar a outra máquina (16 s), a limpeza dela e a entrega.
_TIMEOUT = 90
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class GroupsBridgeError(RuntimeError):
    """Falha da ponte com código (`groups_bridge_off`, o código do Rust)."""
    def __init__(self, code: str, detail: str = ""):
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code, self.detail = code, detail


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = (address, secret) if address and secret else None


def rust_owns_groups() -> bool:
    """Os grupos são do Rust; sem coordenador ainda não há dono."""
    from app import runtime_coordinator
    return runtime_coordinator.current() is not None


def _await_rust() -> None:
    """O servidor limpa as pontes antes de (re)subir o Rust: no `pending` espera o desfecho até
    `PENDING_WAIT_S`."""
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    if owner is not None and (owner.mode == "pending" or getattr(owner, "_settling", False)) \
            and not runtime_coordinator._mode_bypass.get():
        from app.runtime_adapter import run_sync
        try:
            run_sync(owner.await_mode, owner.loop)
        except (runtime_coordinator.RuntimeStarting, TimeoutError) as e:
            raise GroupsBridgeError("groups_runtime_starting") from e
        except RuntimeError as e:
            raise GroupsBridgeError("groups_wait_on_loop") from e


def call(op: str, **args) -> dict:
    """Bloqueia: quem está no laço chama por `to_thread`."""
    _await_rust()
    config = _config
    if config is None:
        raise GroupsBridgeError("groups_bridge_off")
    # Surrogate solto (texto colado) vira o caractere de troca: o Rust só aceita UTF-8 válido, e o
    # erro de codificação aqui era um 500.
    text = json.dumps({"op": op, "args": args}, ensure_ascii=False)
    data = text.encode("utf-16", "surrogatepass").decode("utf-16", "replace").encode("utf-8")
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
