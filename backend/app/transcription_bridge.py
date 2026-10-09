"""Transcrição delegada à porta privada do Rust, sem segunda tentativa no Python."""
import http.client
import ipaddress
import json
import threading
import urllib.error
import urllib.parse
import urllib.request

from app import diag, runtime_coordinator

_config: tuple[str, str] | None = None
_ready = threading.Event()
_slots = threading.BoundedSemaphore(4)
_MAX_RESPONSE = 1024 * 1024


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())

class BridgeError(Exception):
    def __init__(self, status: int, code: str, detail: str):
        super().__init__(detail)
        self.status, self.code, self.detail = status, code, detail


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = None
    _ready.clear()
    if address is None or secret is None:
        return
    host, port = address.rsplit(":", 1)
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 1 <= int(port) <= 65535:
        raise ValueError("endereço privado de transcrição inválido")
    _config = (address, secret)
    _ready.set()


def owned_by_rust() -> bool:
    coordinator = runtime_coordinator.current()
    return coordinator.mode in ("rust", "pending") if coordinator is not None else _config is not None


def _request(operation: str, content: bytes = b"", **query) -> dict:
    if len(content) > 100 * 1024 * 1024:
        raise BridgeError(413, "transcription_audio_too_large", "O áudio excede o limite de 100 MiB.")
    config = _config
    coordinator = runtime_coordinator.current()
    if config is None and operation != "status" and coordinator is not None and coordinator.mode == "pending":
        _ready.wait(runtime_coordinator.PENDING_WAIT_S)
        config = _config
    if config is None:
        raise BridgeError(503, "transcription_rust_unavailable", "O servidor Rust de transcrição está indisponível.")
    if not _slots.acquire(blocking=False):
        raise BridgeError(503, "transcription_busy", "As vagas de transcrição estão ocupadas. Tente novamente.")
    try:
        params = urllib.parse.urlencode({key: value for key, value in query.items() if value is not None})
        request = urllib.request.Request(f"http://{config[0]}/__hangar_server/transcription/{operation}?{params}",
            data=content, method="POST", headers={"x-hangar-internal": config[1], "content-type": "application/octet-stream"})
        budget = {"dictation": 180, "file": 270, "video": 150}.get(query.get("profile"), 15)
        try:
            with _opener.open(request, timeout=budget) as response:
                body = response.read(_MAX_RESPONSE + 1)
        except urllib.error.HTTPError as error:
            body = error.read(_MAX_RESPONSE + 1)
        if len(body) > _MAX_RESPONSE:
            raise ValueError("resposta grande demais")
        value = json.loads(body)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("resposta inválida")
        if not value["ok"]:
            error = value.get("error")
            if not isinstance(error, dict) or type(error.get("status")) is not int or not 400 <= error["status"] <= 599:
                raise ValueError("erro inválido")
            raise BridgeError(error["status"], str(error.get("code") or "transcription_failed"),
                              str(error.get("detail") or "A transcrição falhou."))
        result = value.get("result")
        if not isinstance(result, dict):
            raise TypeError("resultado inválido")
        return result
    except BridgeError:
        raise
    except (OSError, ValueError, TypeError, http.client.HTTPException):
        diag.registrar("transcription.bridge_failed", "aviso", codigo="transcription_rust_unavailable")
        raise BridgeError(503, "transcription_rust_unavailable",
                          "Não foi possível confirmar a transcrição no Rust. O áudio foi preservado para tentar novamente.") from None
    finally:
        _slots.release()


def transcribe(content: bytes, filename: str | None, profile: str, provider_id: str | None = None) -> dict:
    return _request("test" if provider_id is not None else "transcribe", content,
                    filename=filename, profile=profile, provider_id=provider_id)


def providers_status() -> dict:
    return _request("status")
