"""Descoberta de modelos no provedor do motor: GET {base_url}/v1/models.

Por que existe em vez de um catálogo chumbado no código: os dois provedores medidos expõem esse
endpoint com `id` e `context_length`, e o valor é POR FAIXA DE ASSINATURA — o plano Moderato do Kimi
reporta 262144 para o `k3` onde a documentação fala de "até 1M". Tabela estática nasceria errada (o
id `kimi-k3`, por exemplo, não existe: o certo é `k3`, noutro host).

Separado de app/engines.py porque aquele módulo é stdlib pura (o shell o importa sem venv) e este usa
httpx.

SSRF consciente: a URL vem do cliente e o backend a busca. Decisão registrada — sob o modelo de
ameaça do app (usuário único, LAN/VPN, quem tem o token já manda comando arbitrário via /input) isso
não amplia poder. A validação de base_url do engines.py continua valendo (https fora de rede privada).
"""
from typing import Any

# O pacote instalado neste projeto é o fork "httpx2" (pyproject.toml, grupo dev — é o mesmo pacote
# que o TestClient do starlette já usa por baixo). Mesma API do httpx original.
import httpx2 as httpx

_TIMEOUT = 15.0


def _buscar(base_url: str, api_key: str) -> dict[str, Any]:
    # /v1/models é o dialeto OpenAI, que os dois provedores servem ao lado do /v1/messages Anthropic.
    r = httpx.get(f"{base_url.rstrip('/')}/v1/models",
                  headers={"Authorization": f"Bearer {api_key}"}, timeout=_TIMEOUT)
    if r.status_code != 200:
        # A mensagem do provedor é a informação útil ("Invalid Authentication"); engolir isso deixaria
        # o usuário com "não respondeu" e nenhuma pista.
        raise RuntimeError(f"{r.status_code} {r.text[:200]}")
    try:
        return r.json()
    except ValueError:
        raise RuntimeError(f"resposta não-JSON do provedor: {r.text[:200]}") from None


def listar_modelos(base_url: str, api_key: str) -> list[dict[str, Any]]:
    """Modelos que ESTA key pode usar, normalizados para {id, context_length, vision}.

    `vision` é None quando o provedor não diz — melhor um "não sei" honesto do que um false que a
    tela mostraria como "não enxerga imagem".
    """
    try:
        bruto = _buscar(base_url, api_key)
    except httpx.HTTPError as e:
        raise RuntimeError(f"não foi possível falar com o provedor: {e}") from None
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
