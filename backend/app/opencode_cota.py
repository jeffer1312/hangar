"""Cota do OpenCode Go — a única que não sai de uma API, e por isso mora num módulo separado.

O OpenCode NÃO tem rota de cota. Verificado em 18/08/2026: `/v1/usage`, `/v1/usages`, `/v1/me`,
`/v1/balance` e `/v1/account` respondem 404 com a chave de API válida; a documentação do plano Go
manda acompanhar o uso "no console"; e existe um pedido ABERTO pra criar `GET /zen/v1/balance`
(anomalyco/opencode#10448) sem ninguém trabalhando nele.

O que dá pra fazer é o que o pacote `pi-quotas` (latentminds-ai) faz: baixar a PÁGINA do painel com
o cookie de sessão do navegador e pescar os números que o site deixa embutidos no HTML. As três
janelas do plano Go ($12/5h, $30/semana, $60/mês) saem daí, como percentual e segundos até o reset.

Duas fragilidades que são da abordagem, não do código, e por isso o resultado é sempre "não informa
cota" em vez de erro barulhento:
  1. Não é uma API. É o HTML de um site em SolidJS; o dia que renomearem uma variável interna, o
     número some.
  2. O cookie é sessão de navegador, expira, e é credencial mais poderosa que a chave de API —
     por isso o arquivo é 0600 e o valor nunca volta inteiro pra tela.
"""
import json
import logging
import os
import re
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from app import atomico, contas, migracao_sidecars

_log = logging.getLogger("hangar.opencode_cota")

_ARQUIVO = ".hangar-opencode.json"
_URL = "https://opencode.ai/workspace/{}/go"
# O painel recusa o User-Agent do urllib; o pi-quotas manda um de navegador e é o que responde.
_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Gecko/20100101 Firefox/148.0"
_TIMEOUT = 10.0
_lock = threading.Lock()

_NUM = r"(-?\d+(?:\.\d+)?)"


def _pares(chave: str) -> tuple[re.Pattern, re.Pattern]:
    """Os dois regex de uma janela. A ORDEM dos campos no HTML não é estável entre renderizações
    (o SSR do SolidJS emite `usagePercent` antes ou depois de `resetInSec`), então são dois padrões
    e não um — com um só, metade das leituras voltaria vazia sem erro nenhum."""
    return (
        re.compile(rf"{chave}:\$R\[\d+\]=\{{[^}}]*usagePercent:{_NUM}[^}}]*resetInSec:{_NUM}[^}}]*\}}"),
        re.compile(rf"{chave}:\$R\[\d+\]=\{{[^}}]*resetInSec:{_NUM}[^}}]*usagePercent:{_NUM}[^}}]*\}}"),
    )


# rótulo exibido -> chave no HTML. O plano Go tem TRÊS janelas ($12/5h, $30/semana, $60/mês).
_JANELAS = (("5h", "rollingUsage"), ("7d", "weeklyUsage"), ("30d", "monthlyUsage"))


def _janela(html: str, chave: str) -> tuple[float, float] | None:
    """(pct, segundos até o reset) de uma janela, ou None se o HTML não trouxer."""
    pct_primeiro, reset_primeiro = _pares(chave)
    m = pct_primeiro.search(html)
    if m:
        return float(m.group(1)), float(m.group(2))
    m = reset_primeiro.search(html)
    if m:
        return float(m.group(2)), float(m.group(1))
    return None


def _caminho() -> Path:
    return contas.compartilhado() / _ARQUIVO


def ler_configs() -> dict[str, dict[str, str]]:
    """{id_da_credencial: {workspace_id, auth_cookie}}. Ilegível ou tipo errado = vazio."""
    try:
        bruto = json.loads(migracao_sidecars.caminho_de_leitura(_caminho()).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(bruto, dict):
        return {}
    out = {}
    for k, v in bruto.items():
        if not isinstance(k, str) or not isinstance(v, dict):
            continue
        w, c = v.get("workspace_id"), v.get("auth_cookie")
        if isinstance(w, str) and w and isinstance(c, str) and c:
            out[k] = {"workspace_id": w, "auth_cookie": c}
    return out


def config_de(id_credencial: str) -> dict[str, str] | None:
    """A config desta credencial, ou a do AMBIENTE (mesmas variáveis do pi-quotas) como reserva.

    O ambiente vem por último de propósito: quem colou na tela mandou mais do que quem exportou
    numa shell qualquer, e trocar a ordem faria uma variável esquecida no `.bashrc` mandar calada.
    """
    salva = ler_configs().get(id_credencial)
    if salva:
        return salva
    w = (os.environ.get("OPENCODE_GO_WORKSPACE_ID") or "").strip()
    c = (os.environ.get("OPENCODE_GO_AUTH_COOKIE") or "").strip()
    return {"workspace_id": w, "auth_cookie": c} if w and c else None


def definir_config(id_credencial: str, workspace_id: str, auth_cookie: str) -> None:
    """Grava (ou apaga, com os dois vazios). Arquivo 0600: o cookie abre a conta inteira no site."""
    w, c = workspace_id.strip(), auth_cookie.strip()
    with _lock:
        atual = ler_configs()
        if w and c:
            atual[id_credencial] = {"workspace_id": w, "auth_cookie": c}
        else:
            atual.pop(id_credencial, None)
        alvo = _caminho()
        tmp = alvo.with_suffix(f".tmp{os.getpid()}")
        try:
            alvo.parent.mkdir(parents=True, exist_ok=True)
            # 0600 na CRIAÇÃO, não depois: entre o write e o chmod o arquivo com o cookie de
            # sessão ficaria legível por qualquer usuário local. O cookie abre a conta inteira no
            # site do provedor — é a credencial mais poderosa que este app guarda.
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as fh:
                fh.write(json.dumps(atual, ensure_ascii=False, indent=1))
            os.chmod(tmp, 0o600)
            atomico.substituir(tmp, alvo)
        except OSError:
            tmp.unlink(missing_ok=True)
            raise


def _baixar(workspace_id: str, auth_cookie: str) -> tuple[int, str]:
    """(status, html). Status 0 = nem houve resposta."""
    req = urllib.request.Request(
        _URL.format(urllib.parse.quote(workspace_id, safe="")),
        headers={"User-Agent": _UA, "Accept": "text/html", "Cookie": f"auth={auth_cookie}"},
        method="GET",
    )
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, ""
    except (urllib.error.URLError, OSError, ValueError, TimeoutError) as e:
        _log.debug("opencode: painel nao respondeu: %r", e)
        return 0, ""


def ler(workspace_id: str, auth_cookie: str) -> tuple[str, list[dict[str, Any]], str | None]:
    """(estado, janelas, motivo) — o mesmo formato que `cotas._Leitura` espera.

    Cookie vencido devolve 3xx/401 e vira `indisponivel`, não `expirada`: aqui "expirada" é
    vocabulário de credencial de agente, e o cookie do painel não é a credencial que roda nada.
    """
    status, html = _baixar(workspace_id, auth_cookie)
    if not html:
        return "indisponivel", [], (f"http-{status}" if status else "sem-resposta")
    janelas = []
    agora = time.time()
    for rotulo, chave in _JANELAS:
        achado = _janela(html, chave)
        if achado is None:
            continue
        pct, falta_s = achado
        janelas.append({"rotulo": rotulo, "pct": max(0.0, min(100.0, pct)),
                        "reset_ts": agora + max(0.0, falta_s)})
    if not janelas:
        # HTML veio, mas sem os números: ou o cookie não autenticou (o site devolve a página de
        # login com 200) ou o painel mudou. Os dois são "não informa cota", nunca zero.
        return "indisponivel", [], "painel-sem-numeros"
    return "lida", janelas, None
