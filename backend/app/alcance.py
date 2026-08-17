"""Alcance — por onde este servidor responde (aba Acesso).

Duas responsabilidades e nada mais:
1. montar a lista de endereços candidatos (LAN detectado, `public_url` se houver,
   nome do Tailscale se `tailscale status` responder);
2. testar cada um, com resultado e tempo medido.

A rota HTTP `/api/alcance` (registrada pela Task 1 em `api.py`) vive aqui embaixo;
a Task 6 (Lote B) escreve a parte de pareamento dentro deste mesmo roteador.
A Task 8 (peers) reusa a primitiva `testar_endereco` sem reescrever: teto de espera,
formato de estado e motivos nomeados do "por que não" nascem aqui, uma vez.

Tudo que fala com o mundo — HTTP, socket, subprocesso — vive em funções privadas de
I/O trocadas no teste (precedente: `engine_probe._buscar`). O resto é puro.
"""
from __future__ import annotations

import json
import subprocess
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

from fastapi import APIRouter, Depends

from app.auth import require_auth
from app.config import Settings, detect_lan_ip, resolve_bind_ip, settings

alcance_router = APIRouter(prefix="/api/alcance")

# Teto de espera de TODA chamada externa destas duas responsabilidades. Um só número
# no módulo: quem reusa (Task 8) herda o mesmo teto sem precisar conhecer.
# (docs/polish-backlog.md:204-205 registra uma chamada ao Tailscale SEM timeout —
# esta é a segunda, e tem teto próprio ali embaixo.)
TETO_ESPERA_S = 3.0
_TETO_TAILSCALE_S = 2.0

# Loopback ESTRITO (só esta máquina). `0.0.0.0` e `auto` não são loopback: o primeiro
# escuta em todas as interfaces (alcançável) e o segundo resolve pro IP da LAN.
_LOOPBACK_ONLY = {"127.0.0.1", "localhost", "::1"}


def _bater(url: str) -> float:
    """Bate na porta e devolve o tempo em ms. Qualquer resposta HTTP conta como
    alcance — a porta respondeu (mesmo 4xx/5xx é porta viva); só falta de transporte
    é falha, e ela levanta exceção (quem traduz é testar_endereco).
    """
    inicio = time.monotonic()
    try:
        with urllib.request.urlopen(url + "/", timeout=TETO_ESPERA_S):
            pass
    except urllib.error.HTTPError:
        pass  # respondeu HTTP — alcance, não qualidade
    return (time.monotonic() - inicio) * 1000


def _detectar_lan() -> str:
    """IP da LAN (a UDP socket do detect_lan_ip não manda pacote). Função privada de
    I/O de propósito: o teste troca ela em vez de depender da placa da máquina."""
    return detect_lan_ip()


def _nome_tailscale() -> str:
    """Nome DNS do Tailscale (`hangar.tailXXXX.ts.net`), vazio quando indisponível.
    Com teto de espera — a dívida registrada em docs/polish-backlog.md:204-205 é
    justamente UMA chamada ao Tailscale sem timeout; não criar a segunda.
    """
    try:
        r = subprocess.run(
            ["tailscale", "status", "--json"],
            capture_output=True,
            text=True,
            timeout=_TETO_TAILSCALE_S,
        )
        return json.loads(r.stdout)["Self"]["DNSName"].rstrip(".")
    except Exception:
        return ""


def _motivo(e: BaseException) -> str:
    """Motivo nomeado do 'por que não' — estado, não texto cru. A Task 8 mostra isto."""
    if isinstance(e, urllib.error.URLError):
        e = e.reason  # urllib embrulha o transporte; o motivo real é o .reason
    if isinstance(e, TimeoutError):
        return "timeout"
    if isinstance(e, ConnectionRefusedError):
        return "recusou"
    return "erro"


def testar_endereco(url: str) -> dict:
    """PRIMITIVA DE ALCANCE — bater no endereço e devolver o veredito.

    Devolve estado nomeado (`ok` | `falhou` | `nao_configurado`), o tempo em ms e o
    motivo do "por que não" (`recusou` | `timeout` | `erro`) quando falhou. Endereço
    vazio NUNCA é testado — volta "não configurado", que não é defeito.
    """
    if not url:
        return {"estado": "nao_configurado", "tempo_ms": None, "motivo": ""}
    inicio = time.monotonic()
    try:
        tempo = _bater(url)
    except Exception as e:
        return {
            "estado": "falhou",
            "tempo_ms": round((time.monotonic() - inicio) * 1000),
            "motivo": _motivo(e),
        }
    return {"estado": "ok", "tempo_ms": round(tempo), "motivo": ""}


def levantar_estados(s: Settings) -> dict:
    """A resposta pronta da rota /api/alcance.

    `loopback` diz se o bind é SÓ local (o front mostra o alerta), `bind` é o IP
    resolvido (a frase do alerta mostra ele). Ordem das linhas segue o mock: nesta
    máquina (só em loopback), rede local, Tailscale (quando responde), público.
    Os testes rodam em paralelo: o teto vale por chamada, não por linha somada.
    """
    bind = resolve_bind_ip(s)
    loopback = bind in _LOOPBACK_ONLY
    enderecos: list[dict] = []
    if loopback:
        enderecos.append({"tipo": "nesta_maquina", "url": f"http://{bind}:{s.front_port}"})
    enderecos.append({"tipo": "rede_local", "url": f"http://{_detectar_lan()}:{s.front_port}"})
    nome_ts = _nome_tailscale()
    if nome_ts:
        enderecos.append({"tipo": "tailscale", "url": f"https://{nome_ts}"})
    enderecos.append({"tipo": "publico", "url": s.public_url.rstrip("/") if s.public_url else ""})
    with ThreadPoolExecutor(max_workers=len(enderecos)) as pool:
        futuros = {e["tipo"]: pool.submit(testar_endereco, e["url"]) for e in enderecos}
        for e in enderecos:
            e.update(futuros[e["tipo"]].result())
    return {"loopback": loopback, "bind": bind, "enderecos": enderecos}


@alcance_router.get("", dependencies=[Depends(require_auth)])
def listar_alcance() -> dict:
    """Endereços candidatos testados + sinal de bind loopback (aba Acesso)."""
    return levantar_estados(settings)