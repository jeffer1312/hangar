"""Descoberta de modelos no provedor do motor: GET {base_url}/v1/models.

Por que existe em vez de um catálogo chumbado no código: os dois provedores medidos expõem esse
endpoint com `id` e `context_length`, e o valor é POR FAIXA DE ASSINATURA — o plano Moderato do Kimi
reporta 262144 para o `k3` onde a documentação fala de "até 1M". Tabela estática nasceria errada (o
id `kimi-k3`, por exemplo, não existe: o certo é `k3`, noutro host).

Separado de app/engines.py por escopo, não por camada: aquele módulo só lê/grava o engines.json
local; este fala com a rede (a chamada ao provedor). Importa só a constante _PROIBIDO_NO_VALOR de
lá — mesma lista de caracteres banidos de header/shell, uma fonte só. Usa `urllib` da stdlib, não um
cliente HTTP de terceiros — mesma regra registrada em peers.py: "sem httpx no hot path" (o pacote
instalado no projeto, httpx2, é um fork e fica só no grupo dev, usado pelo TestClient).

SSRF consciente: a URL vem do cliente e o backend a busca. Decisão registrada — sob o modelo de
ameaça do app (usuário único, LAN/VPN, quem tem o token já manda comando arbitrário via /input) isso
não amplia poder. A validação de base_url do engines.py continua valendo (https fora de rede privada).
"""
import json
import urllib.error
import urllib.request
from typing import Any

from app.engines import _PROIBIDO_NO_VALOR

_TIMEOUT = 15.0


def _validar_sem_quebra(campo: str, valor: str) -> None:
    # Mesma lista proibida do engines.py (_PROIBIDO_NO_VALOR) — um único "proibido" pros dois
    # módulos. urllib RECUSA um header com \r/\n, mas o ValueError dele ECOA a key crua na
    # mensagem ("Invalid header value b'Bearer sk-x\r\n...'"); isso vazando pro log do
    # uvicorn/journal (POST /api/engines/modelos relança pra logar) é o achado do security
    # reviewer. Validar ANTES de montar o Request barra o vazamento na origem, não no meio dele.
    if any(c in valor for c in _PROIBIDO_NO_VALOR):
        raise ValueError(f"{campo}: sem quebra de linha nem caractere nulo")


def _buscar(base_url: str, api_key: str) -> dict[str, Any]:
    _validar_sem_quebra("base_url", base_url)
    _validar_sem_quebra("api_key", api_key)
    # /v1/models é o dialeto OpenAI, que os dois provedores servem ao lado do /v1/messages Anthropic.
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}/v1/models",
        headers={"Authorization": f"Bearer {api_key}"})
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as r:
            # errors="replace": um provedor pode responder bytes fora de utf-8 válido; deixar
            # UnicodeDecodeError escapar vira 500 com traceback (foge do except abaixo, que só
            # pega URLError/OSError/TimeoutError) em vez do 502 com a mensagem do provedor.
            corpo = r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        # !2xx: a mensagem do provedor ("Invalid Authentication") é a informação útil; engolir isso
        # deixaria o usuário com "não respondeu" e nenhuma pista. O corpo do erro vem de e.read()
        # (HTTPError já É a resposta), não dá pra reler de outro lugar. `with e`: HTTPError é um
        # gerenciador de contexto (é a própria resposta) — o caminho feliz já fecha com `with`.
        with e:
            corpo_erro = e.read().decode(errors="replace")
        raise RuntimeError(f"{e.code} {corpo_erro[:200]}") from None
    except (urllib.error.URLError, OSError, TimeoutError) as e:
        # DNS, conexão recusada, timeout: nunca chegou a ter um HTTP status pra reportar.
        raise RuntimeError(f"não foi possível falar com o provedor: {e}") from None
    try:
        bruto = json.loads(corpo)
    except (json.JSONDecodeError, ValueError):
        raise RuntimeError(f"resposta não-JSON do provedor: {corpo[:200]}") from None
    if not isinstance(bruto, dict):
        # `-> dict[str, Any]` é só o type hint; um provedor que devolve um array ou escalar no
        # topo satisfaz a assinatura e não o runtime. Sem este check, listar_modelos() chama
        # `.get("data")` num objeto sem `.get` — AttributeError não pego pela rota (que só trata
        # RuntimeError) vira 500 com traceback em vez do 502-com-a-mensagem-do-provedor que todo
        # outro caminho malformado deste módulo promete.
        raise RuntimeError(f"resposta em formato inesperado do provedor: {corpo[:200]}")
    return bruto


def listar_modelos(base_url: str, api_key: str) -> list[dict[str, Any]]:
    """Modelos que ESTA key pode usar, normalizados para {id, context_length, vision}.

    `vision` é None quando o provedor não diz — melhor um "não sei" honesto do que um false que a
    tela mostraria como "não enxerga imagem".
    """
    bruto = _buscar(base_url, api_key)
    dados = bruto.get("data")
    if not isinstance(dados, list):
        raise RuntimeError("resposta inesperada do provedor (sem lista 'data')")
    out = []
    for m in dados:
        if not isinstance(m, dict) or not m.get("id"):
            continue
        # Kimi Code usa supports_image_in; OmniRoute agrupa em "capabilities" e não informa imagem.
        caps = m.get("capabilities") if isinstance(m.get("capabilities"), dict) else {}
        vision = m.get("supports_image_in", caps.get("vision", caps.get("image_in")))
        out.append({
            "id": m["id"],
            "context_length": m.get("context_length") or m.get("context_window"),
            "vision": vision if isinstance(vision, bool) else None,
        })
    return out
