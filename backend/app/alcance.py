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

import io
import json
import subprocess
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import qrcode
import qrcode.image.svg
from fastapi import APIRouter, Depends, HTTPException

from app.auth import require_auth
from app.config import Settings, detect_lan_ip, pairing_url, resolve_bind_ip, settings
from app.mensagens import erro

alcance_router = APIRouter(prefix="/api/alcance")

# Credencial de pareamento NAO configurada (o token de fábrica não protege nada —
# a Task 3 e o main.py já recusam publicar ele; aqui ele também não vira QR).
_CREDENCIAL_DE_FABRICA = "change-me"

# Teto de espera de TODA chamada externa destas duas responsabilidades. Um só número
# no módulo: quem reusa (Task 8) herda o mesmo teto sem precisar conhecer.
# (docs/polish-backlog.md:204-205 registra uma chamada ao Tailscale SEM timeout —
# esta é a segunda, e tem teto próprio ali embaixo.)
TETO_ESPERA_S = 3.0
_TETO_TAILSCALE_S = 2.0

# Loopback ESTRITO (só esta máquina). `0.0.0.0` e `auto` não são loopback: o primeiro
# escuta em todas as interfaces (alcançável) e o segundo resolve pro IP da LAN.
_LOOPBACK_ONLY = {"127.0.0.1", "localhost", "::1"}


class _SemRedirect(urllib.request.HTTPRedirectHandler):
    """Não segue redirecionamento. Devolver None faz o 3xx subir como HTTPError, que
    `_bater` já trata como alcance ("a porta respondeu")."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


# `urlopen` reaplica o teto INTEIRO a cada um dos até 10 redirects: medido, "teto de 3s"
# virou 11s numa cadeia de 302. Alcance é "a porta respondeu"; seguir o link não faz
# parte da pergunta. build_opener SUBSTITUI o handler padrão porque _SemRedirect é
# subclasse dele — passar só os outros handlers NÃO tira o redirect (medido).
_ABRIDOR = urllib.request.build_opener(_SemRedirect())


def _bater(url: str) -> float:
    """Bate na porta e devolve o tempo em ms. Qualquer resposta HTTP conta como
    alcance — a porta respondeu (mesmo 4xx/5xx é porta viva); só falta de transporte
    é falha, e ela levanta exceção (quem traduz é testar_endereco).
    """
    inicio = time.monotonic()
    try:
        with _ABRIDOR.open(url + "/", timeout=TETO_ESPERA_S):
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
            # `text=True` sozinho decodifica pelo locale, e no Windows isso e cp1252 (medido:
            # utf8_mode=0, locale.getencoding()='cp1252'). O nome de maquina do tailnet pode ter
            # acento, e ai o `json.loads` recebe texto corrompido — ou estoura, porque cp1252 tem
            # bytes indefinidos. No Linux o locale ja e UTF-8 e isto e no-op.
            encoding="utf-8",
            errors="replace",
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


# ── Pareamento (Task 6, Lote B) ────────────────────────────────────────────────
# O QR é desenhado AQUI, no backend — decisão de plano (17/08): o front só tem
# qr-scanner, que lê e não gera; o backend já tem qrcode>=8.2 e já o usa no QR ASCII
# do boot (main.py). A rota devolve a imagem pronta (SVG).


def _qr_pareamento(url: str) -> str:
    """SVG do QR para o endereço+token de pareamento. Função privada de propósito:
    o teste troca ela quando quer um QR curto sem custo de geração."""
    qr = qrcode.QRCode(border=1)
    qr.add_data(url)
    qr.make(fit=True)
    buf = io.BytesIO()
    qr.make_image(image_factory=qrcode.image.svg.SvgPathImage).save(buf)
    return buf.getvalue().decode("utf-8")


@alcance_router.get("/pareamento", dependencies=[Depends(require_auth)])
def pareamento(endereco: str) -> dict:
    """Endereço + credencial para o candidato pedido, com o QR pronto (SVG).

    `endereco` é o TIPO da linha da aba Acesso (nesta_maquina | rede_local |
    tailscale | publico) — o mesmo que a listagem devolve, pra tela embutir o que
    o usuário escolheu. Candidato desconhecido é recusado; sem credencial
    configurada, erro nomeado (token de fábrica não vira QR).
    """
    conhecidos = {e["tipo"]: e for e in levantar_estados(settings)["enderecos"]}
    if endereco not in conhecidos:
        raise HTTPException(
            404, detail=erro("alcance_endereco_desconhecido", "candidato de pareamento desconhecido", endereco=endereco)
        )
    if settings.auth_token == _CREDENCIAL_DE_FABRICA:
        raise HTTPException(
            400, detail=erro("alcance_sem_credencial", "credencial de pareamento nao configurada")
        )
    # O QR embute o endereço do CANDIDATO ESCOLHIDO — o mesmo URL que a listagem
    # testou (o mock: "troque para o endereço do Tailscale acima"). O pairing_url
    # genérico só vale quando o candidato não tem URL própria (ex.: público sem
    # public_url) — mas aí ele não é oferecido na tela.
    candidato = conhecidos[endereco]
    if candidato["url"]:
        base = candidato["url"].rstrip("/")
        url = f"{base}/?token={settings.auth_token}"
    else:
        url = pairing_url(settings)
    return {"url": url, "qr_svg": _qr_pareamento(url)}