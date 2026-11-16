import os
import socket
import sys
from pathlib import Path

# O motivo de o log não ter aberto, para o diário quando ele existir.
_LOG_FALHOU: str | None = None


def _tirar_log(argv: list[str]) -> Path | None:
    """`--log ARQUIVO` (a tarefa do Windows): tira o par do argv e devolve o caminho."""
    if "--log" not in argv or argv.index("--log") + 1 >= len(argv):
        return None
    i = argv.index("--log")
    alvo = Path(argv[i + 1])
    del argv[i:i + 2]
    return alvo


def _abrir_log(alvo: Path):
    """O log da saída do backend, aberto aqui e não pelo `>` do cmd: este `open` compartilha leitura e
    escrita no Windows, e um neto que sobreviveu ao backend com a cópia herdada não impede a próxima
    subida. Preso mesmo assim (o handle de um `cmd` antigo), vai para outro nome: nunca deixa de subir."""
    erro: OSError | None = None
    try:
        alvo.parent.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        erro = e
    candidatos = [alvo] + [alvo.with_name(f"{alvo.stem}.{n}{alvo.suffix}") for n in range(1, 6)]
    for caminho in candidatos:
        try:
            return open(caminho, "w", encoding="utf-8", errors="replace", buffering=1), caminho
        except OSError as e:
            erro = e
    return None, erro


def _saida_para_log(argv: list[str]) -> None:
    global _LOG_FALHOU
    alvo = _tirar_log(argv)
    if alvo is None:
        return
    arquivo, usado = _abrir_log(alvo)
    if arquivo is None:
        _LOG_FALHOU = f"{alvo}: {usado}"   # sem log, mas no ar; o diário registra
        return
    # Os descritores 1 e 2 também: a saída do hangar-server e dos outros filhos continua no log.
    falhas = []
    for fd in (1, 2):
        try:
            os.dup2(arquivo.fileno(), fd)
        except OSError as e:
            falhas.append(f"fd {fd}: {e}")
    sys.stdout = sys.stderr = arquivo
    if usado != alvo:
        print(f"[hangar] AVISO: {alvo} está preso por outro processo; a saída vai para {usado}")
    if falhas:
        print(f"[hangar] AVISO: a saída dos processos filhos não vem para este log ({'; '.join(falhas)})")


# Antes dos outros imports: um erro de import ou de configuração na subida também vai para o log.
if __name__ == "__main__":
    _saida_para_log(sys.argv)

from app.config import settings, resolve_bind_ip, list_config_dirs, _backend_config_base
from app import migracao_sidecars, resilient_accept, rust_server

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




def _setup_diag_logging() -> None:
    # DIAG: garante que os logs "hangar.*" (RESOLVE/SEND) saiam no stderr -> journald. O uvicorn
    # so configura os proprios loggers; sem isto, INFO de app propaga pro root (WARNING) e some.
    import logging
    import sys
    cp = logging.getLogger("hangar")
    if not cp.handlers:
        h = logging.StreamHandler(sys.stderr)
        h.setFormatter(logging.Formatter("%(levelname)s:     [%(name)s] %(message)s"))
        cp.addHandler(h)
        cp.setLevel(logging.INFO)
        cp.propagate = False
    from app import diag, diag_logging
    diag.migrar_legados(privados=True)
    diag_logging.instalar()




def _tcp_socket(host: str, port: int) -> socket.socket:
    # No Windows SO_REUSEADDR deixa um segundo processo abrir a MESMA porta ao lado do primeiro
    # (o `bind_socket` do uvicorn o liga sempre), então só o POSIX o recebe.
    s = socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET, socket.SOCK_STREAM)
    try:
        if os.name != "nt":
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind((host, port))
    except OSError:
        s.close()
        raise
    # Sem set_inheritable: é um worker só, e filho herdeiro (git, codex, claude headless) segurava a porta em LISTEN.
    return s


def main():
    bind = resolve_bind_ip(settings)
    _saida_utf8()   # antes de qualquer print: o QR abaixo quebra em cp1252
    startup_guard(settings)
    _setup_diag_logging()
    if _LOG_FALHOU:
        from app import diag
        diag.registrar("backend.log_indisponivel", "erro", detalhe=_LOG_FALHOU)
    _state_dirs = list({Path(c.path) for c in list_config_dirs(ordered=False)}
                       | {_backend_config_base().resolve(), Path.home() / ".claude"})
    # Renomeia os sidecars .hangar-* pra .hangar-* (link no caminho antigo). ANTES de tudo
    # que lê ou escreve sidecar: os hooks e o endpoint do Pi já são clientes dessas pastas —
    # migrar depois deles seria migrar por cima de arquivo recém-escrito.
    migracao_sidecars.migrar(_state_dirs)
    # Windows: cliente que aborta no meio do accept nao pode fechar o listener (ver o modulo).
    resilient_accept.install()
    # proxy_headers + forwarded_allow_ips: atras de um TLS proxy (Caddy/Tailscale), faz o uvicorn ler
    # X-Forwarded-For/-Proto SO do proxy confiavel -> request.client.host vira o IP real do cliente
    # (rate limiter por-cliente, nao um balde global) e request.url.scheme vira https (cookie Secure).
    kw = dict(host=bind, port=settings.port, workers=1, proxy_headers=True,
              forwarded_allow_ips=settings.forwarded_allow_ips,
              timeout_graceful_shutdown=rust_server.GRACEFUL_SHUTDOWN_S)
    rust_bin = rust_server.wanted_binary()
    if rust_bin is None:
        # Sem o hangar-server não há quem atenda a porta: o Python não a atende sozinho.
        print("[hangar] ERRO: hangar-server não encontrado (CP_RUST_SERVER_BIN, bin/ da versão, "
              "crates/target/release, ~/.hangar/bin) ou sem par de autenticação válido", file=sys.stderr)
        sys.exit(1)
    try:
        main_sock = _tcp_socket(bind, settings.port)
    except OSError as e:
        print(f"[hangar] ERRO: porta {settings.port} indisponível ({e})", file=sys.stderr)
        sys.exit(1)
    # A porta pública fica com o hangar-server; o bind acima só provou que ela estava livre.
    main_sock.close()
    sys.exit(rust_server.run("app.api:app", kw, rust_bin, settings.auth_token, [_tcp_socket("127.0.0.1", 0)]))


if __name__ == "__main__":
    main()
