import io
import sys
from pathlib import Path

import uvicorn
import qrcode

from app.config import settings, resolve_bind_ip, pairing_url, list_config_dirs, _backend_config_base
from app.hook_installer import ensure_askq_hook_installed, ensure_state_hooks_installed
from app.hook_state import hook_state

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


def _saida_utf8() -> None:
    """Garante que stdout/stderr aguentem qualquer caractere, em qualquer plataforma.

    O QR de pareamento e desenhado com blocos (U+2588 e vizinhos). Quando a saida vai pra ARQUIVO
    em vez de console, o Python no Windows usa a codepage local (cp1252), que nao tem esses
    caracteres — e o `print` levanta UnicodeEncodeError. Medido: o backend morria na SUBIDA, antes
    de abrir a porta, sempre que rodava como tarefa agendada (que redireciona a saida pra log).
    No console interativo passava, porque ali a codepage costuma ser UTF-8: o bug so aparecia
    exatamente onde ninguem estava olhando.

    `errors="replace"` alem do encoding: um caractere inesperado num log NUNCA pode derrubar o
    servidor. Perder um glifo e aceitavel; perder o backend nao.
    """
    for fluxo in (sys.stdout, sys.stderr):
        try:
            fluxo.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass   # fluxo sem reconfigure (redirecionado pra objeto custom) — segue como estiver


def print_pairing(settings) -> None:
    """Print a scannable QR (PWA URL + token) so a phone pairs without typing anything."""
    url = pairing_url(settings)
    # QR so faz sentido em TERMINAL: ele existe pra ser apontado com a camera do celular. Rodando
    # como servico (systemd, tarefa agendada) a saida vai pra arquivo, onde o desenho e ruido puro
    # — e era justamente ali que ele derrubava o backend por codificacao. A URL com o token, essa
    # sim, e util no log: e o que se copia pra parear na mao.
    if sys.stdout.isatty():
        qr = qrcode.QRCode(border=1)
        qr.add_data(url)
        qr.make(fit=True)
        buf = io.StringIO()
        qr.print_ascii(out=buf, invert=True)
        print(buf.getvalue(), flush=True)
        print(f"  Scan to pair, or open: {url}\n", flush=True)
    else:
        print(f"  Pareamento (abra no celular): {url}\n", flush=True)


def _setup_diag_logging() -> None:
    # DIAG: garante que os logs "claude_pocket.*" (RESOLVE/SEND) saiam no stderr -> journald. O uvicorn
    # so configura os proprios loggers; sem isto, INFO de app propaga pro root (WARNING) e some.
    import logging
    import sys
    cp = logging.getLogger("claude_pocket")
    if not cp.handlers:
        h = logging.StreamHandler(sys.stderr)
        h.setFormatter(logging.Formatter("%(levelname)s:     [%(name)s] %(message)s"))
        cp.addHandler(h)
        cp.setLevel(logging.INFO)
        cp.propagate = False


def main():
    bind = resolve_bind_ip(settings)
    _saida_utf8()   # antes de qualquer print: o QR abaixo quebra em cp1252
    startup_guard(settings)
    _setup_diag_logging()
    # Instala (idempotente, fail-soft) os hooks de estado e de AskUserQuestion.
    ensure_askq_hook_installed()
    ensure_state_hooks_installed()
    _state_dirs = list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    hook_state.load_existing(_state_dirs)
    print_pairing(settings)
    # workers=1 explicito: o cache de classe SessionRegistry._jsonl_cache e compartilhado SO dentro de
    # um processo. Multi-worker daria cache frio por worker -> transcript errado em requests roteados
    # pra outro worker. Multi-worker exigiria mover o cache pra um backend compartilhado.
    # proxy_headers + forwarded_allow_ips: atras de um TLS proxy (Caddy/Tailscale), faz o uvicorn ler
    # X-Forwarded-For/-Proto SO do proxy confiavel -> request.client.host vira o IP real do cliente
    # (rate limiter por-cliente, nao um balde global) e request.url.scheme vira https (cookie Secure).
    uvicorn.run("app.api:app", host=bind, port=settings.port, reload=settings.reload, workers=1,
                proxy_headers=True, forwarded_allow_ips=settings.forwarded_allow_ips)


if __name__ == "__main__":
    main()
