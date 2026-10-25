import io
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

import uvicorn
import qrcode

from app.config import pairing_url_api, settings, resolve_bind_ip, pairing_url, list_config_dirs, _backend_config_base
from app.hook_installer import (
    ensure_askq_hook_installed,
    ensure_guard_hooks_installed,
    ensure_nav_hook_installed,
    ensure_pair_hook_installed,
    ensure_preview_hook_installed,
    ensure_subagent_hook_installed,
    ensure_state_hooks_installed,
)
from app import migracao_sidecars, orq_politica, resilient_accept, rust_server
from app.hook_state import hook_state
from app.pi_inbox import escrever_endpoint

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
    url_api = pairing_url_api(settings)
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
        # Segundo QR — app nativo (API direto, sem PWA)
        qr2 = qrcode.QRCode(border=1)
        qr2.add_data(url_api)
        qr2.make(fit=True)
        buf2 = io.StringIO()
        qr2.print_ascii(out=buf2, invert=True)
        print(buf2.getvalue(), flush=True)
        print(f"  App nativo: {url_api}\n", flush=True)
    else:
        print(f"  Pareamento (abra no celular): {url}\n", flush=True)
        print(f"  App nativo: {url_api}\n", flush=True)


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


def _passos_pendentes_da_versao(windows: bool = os.name == "nt") -> None:
    """Passos que a versão exige e que ainda não rodaram AQUI — os não destrutivos.

    Aqui, e não no installer, pelo mesmo motivo do `migracao_sidecars` logo acima: atualizar neste
    projeto é `git pull` + reiniciar o serviço, e rodar `install-*.sh` não é garantido. Assim quem
    atualizou pela linha de comando, sem tocar no botão do app, fica em dia no restart seguinte.

    Só os NÃO destrutivos: passo que apaga ou sobrescreve alguma coisa espera o botão, onde há uma
    pessoa olhando e o motor já guardou o disco numa branch de resgate antes.

    Fail-soft, como os hooks: um passo com defeito não pode impedir o backend de subir — a máquina
    ficaria sem app por causa de uma linha errada num arquivo de atualização.
    """
    try:
        from app import atualizacoes, diag
        # Pacote: quem aplica os passos de máquina é o atualizador do Rust, fora deste processo.
        if diag.install_manifest():
            return
        # O instalador reinicia as tarefas e mataria este backend no meio da subida: fica para o botão.
        if windows and any("install.ps1" in p["comando"] for p in atualizacoes.pendentes(incluir_destrutivos=False)):
            print("[hangar] passos de atualizacao pendentes que chamam o instalador ficam para o botao Atualizar")
            return
        feitos = atualizacoes.aplicar_pendentes(incluir_destrutivos=False)
        if feitos:
            print(f"[hangar] passos de atualizacao aplicados: {', '.join(feitos)}")
    except Exception as e:                            # noqa: BLE001 — nunca derruba a subida
        print(f"[hangar] AVISO: passo de atualizacao falhou ({e}); o app segue subindo")


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
    # que lê ou escreve sidecar: os hooks, o endpoint do Pi e o hook_state logo abaixo já são
    # clientes dessas pastas — migrar depois deles seria migrar por cima de arquivo recém-escrito.
    migracao_sidecars.migrar(_state_dirs)
    orq_politica.migrar()
    # Uma configuração do Jev: a chave antiga do Computer Use vira a dela e o MCP recebe endereço e modelo.
    # Modelo auxiliar único ANTES do repasse ao MCP: o `sync_jev` reescreve o `LLM_*` com o auxiliar, e
    # o legado precisa ter sido lido do MCP antes disso.
    try:
        from app import runtime_config
        runtime_config.migrar_jev_aux()
    except Exception as e:                            # noqa: BLE001 — nunca derruba a subida
        print(f"[hangar] AVISO: o modelo auxiliar antigo não virou o do Jev ({e})")
    try:
        from app import computer_control
        computer_control.migrate_jev()
    except Exception as e:                            # noqa: BLE001 — nunca derruba a subida
        print(f"[hangar] AVISO: configuração do Jev não chegou ao Computer Use ({e})")
    # Ponte de skills (pi/kimi/codex) — rebuilda as fazendas de symlinks a partir das fontes do
    # Claude. Aqui e no installer: bump de versão de plugin muda o caminho do cache e link velho
    # fica pendurado; fail-soft como os hooks, um erro aqui não pode impedir o backend de subir.
    try:
        from app import skill_bridge
        skill_bridge.rebuild(log=lambda m: print(f"[hangar] {m}"))
    except Exception as e:                            # noqa: BLE001 — nunca derruba a subida
        print(f"[hangar] AVISO: ponte de skills falhou ({e}); o app segue subindo")
    if settings.omp_claude_context_enabled:
        from app.omp_context import configure_claude_context
        context_result = configure_claude_context(claude_dir=_backend_config_base(), enabled=True)
        if context_result["errors"]:
            print(f"[hangar] AVISO: contexto OMP não configurado: {context_result['errors']}")
    _passos_pendentes_da_versao()
    # Instala (idempotente, fail-soft) os hooks de estado e de AskUserQuestion.
    ensure_askq_hook_installed()
    ensure_state_hooks_installed()
    ensure_preview_hook_installed()
    ensure_subagent_hook_installed()
    ensure_pair_hook_installed()
    ensure_nav_hook_installed()
    ensure_guard_hooks_installed()
    # Idem pro hook do Kimi (config.toml do ~/.kimi-code) — ver app/kimi_hook_installer.py.
    from app.kimi_hook_installer import ensure_kimi_hooks_installed
    ensure_kimi_hooks_installed()
    # E pro Codex (hooks.json do ~/.codex) — ver app/codex_hook_installer.py.
    from app.codex_hook_installer import ensure_codex_state_hook_installed
    ensure_codex_state_hook_installed()
    # Endereço pra extensão do Pi ligar de volta (ver pi_inbox.escrever_endpoint).
    escrever_endpoint()
    hook_state.load_existing(_state_dirs)
    print_pairing(settings)
    # Windows: cliente que aborta no meio do accept nao pode fechar o listener (ver o modulo).
    resilient_accept.install()
    # workers=1 explicito: o cache de classe SessionRegistry._jsonl_cache e compartilhado SO dentro de
    # um processo. Multi-worker daria cache frio por worker -> transcript errado em requests roteados
    # pra outro worker. Multi-worker exigiria mover o cache pra um backend compartilhado.
    # proxy_headers + forwarded_allow_ips: atras de um TLS proxy (Caddy/Tailscale), faz o uvicorn ler
    # X-Forwarded-For/-Proto SO do proxy confiavel -> request.client.host vira o IP real do cliente
    # (rate limiter por-cliente, nao um balde global) e request.url.scheme vira https (cookie Secure).
    kw = dict(host=bind, port=settings.port, workers=1, proxy_headers=True,
              forwarded_allow_ips=settings.forwarded_allow_ips,
              timeout_graceful_shutdown=rust_server.GRACEFUL_SHUTDOWN_S)
    if settings.reload:
        # O reload recria o processo e só religa o endereço da config: no dev não há porta de convite.
        uvicorn.run("app.api:app", reload=True, **kw)
        return
    rust_bin = rust_server.wanted_binary(settings.rust_server)
    try:
        main_sock = _tcp_socket(bind, settings.port)
    except OSError as e:
        print(f"[hangar] ERRO: porta {settings.port} indisponível ({e})", file=sys.stderr)
        sys.exit(1)
    # A porta do convite (8766) é do hangar-server: o Python nunca a abre, com Rust ou sem.
    extras = []
    if rust_bin is not None:
        # A porta pública fica com o hangar-server; o bind acima só provou que ela estava livre.
        main_sock.close()
        sys.exit(rust_server.run("app.api:app", kw, rust_bin, settings.auth_token,
                                 [_tcp_socket("127.0.0.1", 0)] + extras,
                                 lambda: _tcp_socket(bind, settings.port)))
    from app import migration_status
    if settings.rust_server:
        migration_status.set_reason("sem_binario")
    migration_status.set_listen_port(settings.port)
    # Um Server com dois sockets: um lifespan só (dois Server rodariam watchers e hooks em dobro).
    config = uvicorn.Config("app.api:app", **kw)
    server = rust_server.Server(config)
    server.run(sockets=[main_sock] + extras)
    if not server.started:
        sys.exit(3)                              # mesmo código do uvicorn.run: o systemd reinicia


if __name__ == "__main__":
    main()
