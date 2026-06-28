import io

import uvicorn
import qrcode

from app.config import settings, resolve_bind_ip, pairing_url
from app.hook_installer import ensure_askq_hook_installed

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
    # Instala (idempotente, fail-soft) o hook que captura o payload do AskUserQuestion.
    ensure_askq_hook_installed()
    print_pairing(settings)
    # workers=1 explicito: o cache de classe SessionRegistry._jsonl_cache e compartilhado SO dentro de
    # um processo. Multi-worker daria cache frio por worker -> transcript errado em requests roteados
    # pra outro worker. Multi-worker exigiria mover o cache pra um backend compartilhado.
    uvicorn.run("app.api:app", host=bind, port=settings.port, reload=settings.reload, workers=1)


if __name__ == "__main__":
    main()
