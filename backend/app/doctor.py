"""hangar-doctor: o que está faltando na instalação e como consertar, uma linha por item.
Roda com cwd = backend/ (o Settings lê o .env pelo diretório atual)."""
from __future__ import annotations

import io
import os
import shutil
import socket
import sys
import unicodedata
from collections import namedtuple

from app.config import pairing_url, settings

Linha = namedtuple("Linha", "nivel titulo conserto")
_WIN = os.name == "nt"


def _binario(nome: str) -> str | None:
    return shutil.which(nome)


def _porta_responde(porta: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", porta), timeout=1.5):
            return True
    except OSError:
        return False


def _claude_logado() -> bool:
    import app.conta_estado as ce
    try:
        contas = ce.listar_contas()
    except Exception:
        return False
    return any(c.login.loggedIn is True for c in contas)


def _tailscale() -> tuple[str, str]:
    """('ausente'|'instalado', ''|'logado'|'deslogado')."""
    if not _binario("tailscale"):
        return ("ausente", "")
    import app.alcance as alcance
    return ("instalado", "logado" if alcance._nome_tailscale() else "deslogado")


def _lan_responde(s) -> bool:
    import app.alcance as alcance
    try:
        estados = alcance.levantar_estados(s)["enderecos"]
    except Exception:
        return False
    return any(e.get("tipo") == "rede_local" and e.get("estado") == "ok" for e in estados)


def _conserto_backend() -> str:
    if _WIN:
        return "Start-ScheduledTask hangar-backend  (log: %LOCALAPPDATA%\\hangar\\logs\\privado\\backend.log)"
    return "systemctl --user restart hangar-backend  (log: ~/.hangar/logs/privado/backend.log; journalctl --user -u hangar-backend -n 50)"


def diagnosticar(s) -> list[Linha]:
    linhas: list[Linha] = []

    if not s.auth_token or s.auth_token == "change-me":
        linhas.append(Linha("erro", "token de acesso não definido",
                            "edite backend/.env: CP_AUTH_TOKEN=<sua senha> e reinicie o Hangar"))
    else:
        linhas.append(Linha("ok", "token de acesso definido", ""))

    if _porta_responde(s.port):
        linhas.append(Linha("ok", f"Hangar respondendo em http://127.0.0.1:{s.port}", ""))
    else:
        linhas.append(Linha("erro", f"Hangar não responde na porta {s.port}", _conserto_backend()))

    if _binario("tmux"):
        linhas.append(Linha("ok", "multiplexador de terminal (tmux) no PATH", ""))
    else:
        conserto = "winget install marlocarlo.psmux" if _WIN else "instale o tmux pelo gerenciador de pacotes"
        linhas.append(Linha("erro", "tmux não encontrado — sem ele nenhuma sessão abre", conserto))

    if not _binario("claude"):
        linhas.append(Linha("erro", "Claude Code não encontrado no PATH",
                            "curl -fsSL https://claude.ai/install.sh | bash  (Windows: irm https://claude.ai/install.ps1 | iex)"))
    elif _claude_logado():
        linhas.append(Linha("ok", "Claude Code instalado e logado", ""))
    else:
        linhas.append(Linha("erro", "Claude Code instalado mas sem login",
                            "abra um terminal, rode `claude` e siga o login; depois rode hangar-doctor de novo"))

    estado, login = _tailscale()
    if estado == "ausente":
        linhas.append(Linha("aviso", "Tailscale não instalado — o celular só entra no mesmo Wi-Fi",
                            "quer usar fora de casa? rode o instalador de novo e responda Sim ao Tailscale"))
    elif login != "logado":
        linhas.append(Linha("erro", "Tailscale instalado mas sem login",
                            "sudo tailscale up  (Windows: tailscale up) e depois o instalador de novo"))
    elif not s.public_url:
        linhas.append(Linha("aviso", "Tailscale logado mas o Hangar não está publicado nele",
                            "rode o instalador de novo (ele publica e grava CP_PUBLIC_URL em backend/.env)"))
    else:
        linhas.append(Linha("ok", f"publicado no Tailscale: {s.public_url}", ""))

    if _lan_responde(s):
        linhas.append(Linha("ok", "endereço da rede local responde (celular no mesmo Wi-Fi)", ""))
    else:
        linhas.append(Linha("aviso", "endereço da rede local não responde",
                            "firewall? Linux: sudo ./scripts/lan-setup.sh 8765 — Windows: regra 'hangar 8765' no perfil Private"))
    return linhas


_MARCA = {"ok": "ok  ", "aviso": "--  ", "erro": "X   "}


def _ascii(s: str) -> str:
    """Sem acento e sem travessão: o console do Windows é cp850, onde o U+2014 vira `?`."""
    return unicodedata.normalize("NFKD", s.replace("—", "-").replace("–", "-")) \
        .encode("ascii", "ignore").decode()


def _qr(s) -> int:
    """Desenha SÓ o QR do PWA, que é o que a tela final do instalador manda ler.

    `main.print_pairing` desenha dois QRs com legenda em inglês — ele serve ao log do backend.
    rc 2 = sem terminal, QR não desenhado (o instalador troca a linha 2 da tela final por isso).
    """
    url = pairing_url(s)
    if not sys.stdout.isatty():
        print(f"  Abra no celular: {url}\n", flush=True)
        return 2
    import qrcode
    qr = qrcode.QRCode(border=1)
    qr.add_data(url)
    qr.make(fit=True)
    buf = io.StringIO()
    qr.print_ascii(out=buf, invert=True)
    print(buf.getvalue(), flush=True)
    print(f"  Aponte a câmera do celular para o QR acima; ou abra: {url}\n", flush=True)
    return 0


def main(argv: list[str]) -> int:
    if "--qr" in argv:
        return _qr(settings)
    linhas = diagnosticar(settings)
    saida = print if not _WIN else (lambda t="": print(_ascii(t)))
    for l in linhas:
        saida(f"  {_MARCA[l.nivel]}{l.titulo}")
        if l.conserto:
            saida(f"        conserto: {l.conserto}")
    erros = sum(1 for l in linhas if l.nivel == "erro")
    avisos = sum(1 for l in linhas if l.nivel == "aviso")
    saida()
    if erros:
        saida(f"  {erros} item(ns) para consertar (marcados com X)")
    elif avisos:
        saida(f"  {avisos} aviso(s) — nada quebrado, mas veja acima")
    else:
        saida("  tudo certo")
    return 1 if erros else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
