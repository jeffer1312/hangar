"""Endereço e segredo da porta privada do Rust, para o terminal que o Python ainda recebe."""
import ipaddress

_config: tuple[str, str] | None = None


def configure(address: str | None, secret: str | None) -> None:
    global _config
    _config = None
    if address is None or secret is None:
        return
    host, port = address.rsplit(":", 1)
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 1 <= int(port) <= 65535:
        raise ValueError("invalid list address")
    _config = (address, secret)


def endpoint() -> tuple[str, str] | None:
    """Endereço e segredo da porta privada do Rust, ou None com a ponte desligada."""
    return _config
