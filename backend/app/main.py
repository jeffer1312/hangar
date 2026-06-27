import io

import uvicorn
import qrcode

from app.config import settings, resolve_bind_ip, pairing_url

LOOPBACK = {"127.0.0.1", "localhost", "::1"}


def startup_guard(settings) -> None:
    """Refuse to bind a non-loopback interface with the default token."""
    if settings.auth_token == "change-me" and settings.lan_bind_ip not in LOOPBACK:
        raise SystemExit(
            "Refusing to start: CP_AUTH_TOKEN is still the default 'change-me' "
            f"while binding {settings.lan_bind_ip}. Set CP_AUTH_TOKEN to a strong "
            "secret, or bind 127.0.0.1 for local dev."
        )
    if settings.auth_token == "change-me":
        print("WARNING: using the default 'change-me' token on loopback. "
              "Set CP_AUTH_TOKEN before exposing this on your LAN.")


def print_pairing(settings) -> None:
    """Print a scannable QR (PWA URL + token) so a phone pairs without typing anything."""
    url = pairing_url(settings)
    qr = qrcode.QRCode(border=1)
    qr.add_data(url)
    qr.make(fit=True)
    buf = io.StringIO()
    qr.print_ascii(out=buf, invert=True)
    print(buf.getvalue(), flush=True)
    print(f"  Scan to pair, or open: {url}\n", flush=True)


def main():
    bind = resolve_bind_ip(settings)
    startup_guard(settings)
    print_pairing(settings)
    uvicorn.run("app.api:app", host=bind, port=settings.port, reload=settings.reload)


if __name__ == "__main__":
    main()
