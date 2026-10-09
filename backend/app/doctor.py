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
_OUTROS_AGENTES = (("codex", "Codex"), ("pi", "Pi"), ("omp", "omp"), ("kimi", "Kimi Code"))


def _binario(nome: str) -> str | None:
    return shutil.which(nome)


def _porta_responde(porta: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", porta), timeout=1.5):
            return True
    except OSError:
        return False


def _tempo_resposta(porta: int) -> float | None:
    """Segundos até a primeira resposta HTTP; None = aceita conexão mas não responde em 10 s."""
    import http.client
    import time
    import urllib.error
    import urllib.request
    inicio = time.monotonic()
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{porta}/", timeout=10).close()
    except urllib.error.HTTPError:
        pass  # 401/404 também é resposta: o processo está atendendo
    except (OSError, http.client.HTTPException):
        return None
    return time.monotonic() - inicio


def _reinicios_automaticos(dias: int = 7) -> list[str] | None:
    """Carimbo de cada reinício feito pelo vigia no período; None = fonte ilegível."""
    import datetime as dt
    if _WIN:
        # A vigia do Windows (scripts/windows-tasks.ps1) escreve "<data ISO> vigia: ... sem resposta HTTP".
        from app.log_paths import base
        try:
            texto = (base() / "privado" / "hangar-vigia.log").read_text(encoding="utf-8-sig", errors="replace")
        except FileNotFoundError:
            return []
        except OSError:
            return None
        corte = (dt.datetime.now() - dt.timedelta(days=dias)).isoformat(timespec="seconds")
        return [l.split(" ", 1)[0] for l in texto.splitlines()
                if "hangar-backend sem resposta HTTP" in l and l[:19] >= corte]
    import subprocess
    journalctl = shutil.which("journalctl")
    if not journalctl:
        return None
    try:
        r = subprocess.run([journalctl, "--user", "-u", "hangar-backend", "--since", f"-{dias}d",
                            "-o", "short-iso", "--no-pager", "-q"],
                           capture_output=True, text=True, errors="replace", timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if r.returncode != 0:
        return None
    # O systemd só escreve esta frase quando o Restart=on-failure reergue o processo.
    return [l.split(" ", 1)[0] for l in r.stdout.splitlines() if "Scheduled restart job" in l]


def _prioridade_backend(porta: int) -> str | None:
    """Classe de prioridade do processo que escuta a porta (só Windows); None = não achou."""
    import psutil
    nomes = {"IDLE_PRIORITY_CLASS": "ociosa", "BELOW_NORMAL_PRIORITY_CLASS": "abaixo do normal",
             "NORMAL_PRIORITY_CLASS": "normal", "ABOVE_NORMAL_PRIORITY_CLASS": "acima do normal",
             "HIGH_PRIORITY_CLASS": "alta", "REALTIME_PRIORITY_CLASS": "tempo real"}
    try:
        for c in psutil.net_connections(kind="tcp"):
            if c.status == psutil.CONN_LISTEN and c.laddr and c.laddr.port == porta and c.pid:
                return nomes.get(getattr(psutil.Process(c.pid).nice(), "name", ""))
    except (OSError, psutil.Error):
        pass
    return None


def _claude_logado() -> bool:
    """Alguma conta Claude logada, pela CLI: o doctor roda sem o backend (e o Rust) de pé."""
    import json
    import subprocess
    from pathlib import Path
    from app import contas
    from app.config import list_config_dirs
    exe = _binario("claude")
    if exe is None:
        return False
    for conta in list_config_dirs(ordered=False):
        if not (conta.active or contas.e_conta(Path(conta.path))):
            continue
        try:
            r = subprocess.run([exe, "auth", "status", "--json"], capture_output=True, text=True,
                               encoding="utf-8", errors="replace", timeout=10,
                               env={**os.environ, "CLAUDE_CONFIG_DIR": conta.path})
            # rc 1 numa conta deslogada ainda traz o JSON: a resposta vale pelo parse.
            if json.loads(r.stdout).get("loggedIn") is True:
                return True
        except (OSError, subprocess.TimeoutExpired, ValueError, AttributeError):
            continue
    return False


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

    if not _porta_responde(s.port):
        linhas.append(Linha("erro", f"Hangar não responde na porta {s.port}", _conserto_backend()))
    else:
        t = _tempo_resposta(s.port)
        if t is None:
            linhas.append(Linha("erro", f"Hangar travado: a porta {s.port} aceita conexão mas não responde em 10 s",
                                _conserto_backend()))
        elif t > 3:
            # 3 s é o limite da vigia do Windows: acima disso ela derruba o backend como se tivesse caído.
            linhas.append(Linha("aviso", f"Hangar lento: respondeu em {t:.1f} s na porta {s.port}",
                                "máquina sobrecarregada? veja CPU/memória; no Windows a vigia reinicia acima de 3 s"))
        else:
            linhas.append(Linha("ok", f"Hangar respondendo em http://127.0.0.1:{s.port} ({t * 1000:.0f} ms)", ""))

    if _WIN:
        prioridade = _prioridade_backend(s.port)
        if prioridade is None:
            linhas.append(Linha("aviso", "não deu para ler a prioridade do processo do backend", ""))
        elif prioridade in ("ociosa", "abaixo do normal"):
            linhas.append(Linha("aviso", f"backend rodando com prioridade {prioridade} — com a CPU cheia ele fica lento",
                                "rode o Atualizar do app (ou install.ps1 -Update): a tarefa volta a nascer normal"))
        else:
            linhas.append(Linha("ok", f"backend com prioridade {prioridade}", ""))

    reinicios = _reinicios_automaticos()
    if reinicios is None:
        linhas.append(Linha("aviso", "não deu para ler os reinícios automáticos do backend", ""))
    elif not reinicios:
        linhas.append(Linha("ok", "nenhum reinício automático do backend nos últimos 7 dias", ""))
    else:
        onde = (r"%LOCALAPPDATA%\hangar\logs\privado\hangar-vigia.log" if _WIN
                else "journalctl --user -u hangar-backend --since -7d")
        linhas.append(Linha("aviso", f"o vigia reiniciou o backend {len(reinicios)} vez(es) nos últimos 7 dias"
                                     f" (última: {reinicios[-1]})", f"detalhes: {onde}"))

    if _binario("tmux"):
        linhas.append(Linha("ok", "multiplexador de terminal (tmux) no PATH", ""))
    else:
        conserto = "winget install marlocarlo.psmux" if _WIN else "instale o tmux pelo gerenciador de pacotes"
        linhas.append(Linha("erro", "tmux não encontrado — sem ele nenhuma sessão abre", conserto))

    # O Claude Code é o agente padrão, não obrigatório: o mínimo é ter algum agente de código.
    outros = [nome for cli, nome in _OUTROS_AGENTES if _binario(cli)]
    if not _binario("claude"):
        if outros:
            linhas.append(Linha("ok", f"agentes de código: {', '.join(outros)}", ""))
        else:
            linhas.append(Linha("erro", "nenhum agente de código no PATH (Claude Code, Codex, Pi, omp ou Kimi Code)",
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
