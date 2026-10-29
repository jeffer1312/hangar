"""Conexão privada compartilhada pelos consumidores Python do catálogo de modelos Rust."""
import ipaddress
import threading
import urllib.request


_config: tuple[str, str] | None = None
_ready = threading.Event()


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
