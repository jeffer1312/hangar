"""Descoberta de modelos no provedor do motor: GET {base_url}/v1/models.

Por que existe em vez de um catálogo chumbado no código: os dois provedores medidos expõem esse
endpoint com `id` e `context_length`, e o valor é POR FAIXA DE ASSINATURA — o plano Moderato do Kimi
reporta 262144 para o `k3` onde a documentação fala de "até 1M". Tabela estática nasceria errada (o
id `kimi-k3`, por exemplo, não existe: o certo é `k3`, noutro host).

Separado de app/engines.py por escopo, não por dependência: aquele módulo só lê/grava o
engines.json local; este fala com a rede (a chamada ao provedor). Usa `urllib` da stdlib, não um
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

_TIMEOUT = 15.0


def _buscar(base_url: str, api_key: str) -> dict[str, Any]:
    # /v1/models é o dialeto OpenAI, que os dois provedores servem ao lado do /v1/messages Anthropic.
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}/v1/models",
        headers={"Authorization": f"Bearer {api_key}"})
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as r:
            corpo = r.read().decode()
    except urllib.error.HTTPError as e:
        # !2xx: a mensagem do provedor ("Invalid Authentication") é a informação útil; engolir isso
        # deixaria o usuário com "não respondeu" e nenhuma pista. O corpo do erro vem de e.read()
        # (HTTPError já É a resposta), não dá pra reler de outro lugar.
        corpo_erro = e.read().decode(errors="replace")
        raise RuntimeError(f"{e.code} {corpo_erro[:200]}") from None
    except (urllib.error.URLError, OSError, TimeoutError) as e:
        # DNS, conexão recusada, timeout: nunca chegou a ter um HTTP status pra reportar.
        raise RuntimeError(f"não foi possível falar com o provedor: {e}") from None
    try:
        return json.loads(corpo)
    except (json.JSONDecodeError, ValueError):
        raise RuntimeError(f"resposta não-JSON do provedor: {corpo[:200]}") from None


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
