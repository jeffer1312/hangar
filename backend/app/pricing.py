"""Tarifa por modelo. Núcleo: o catálogo e como uma entrada do models.dev vira Rate.

REGRA DURA: modelo sem tarifa devolve None. Nunca zero — zero afirma "não custou nada", que é
mentira diferente de "não sei o preço". Foi o fallback silencioso pra Sonnet (costs.py:29) que
motivou este módulo.
"""
from __future__ import annotations

from dataclasses import dataclass

# Só provedores de primeira mão. Varrer os 176 do models.dev casa 'k3' com uma entrada de preço
# ZERO de alguma revenda e a sessão inteira vira US$ 0,00, calada — medido. Ampliar esta lista
# quebra test_lista_de_provedores_e_fechada de propósito.
PROVEDORES = (
    "anthropic", "openai", "moonshotai", "zhipuai",
    "deepseek", "google", "xai", "mistral",
)


@dataclass(frozen=True)
class Rate:
    input: float
    output: float
    cache_read: float
    cache_write: float
    provider: str
    origin: str            # "override" | "models.dev" | "snapshot"
    cache_estimado: bool   # provedor não publica preço de cache; cobrado como input


def slim(bruto: dict) -> dict[str, dict]:
    """Catálogo cru do models.dev (176 provedores, ~3,2 MB) -> mapa enxuto modelo -> tarifa."""
    out: dict[str, dict] = {}
    for pid in PROVEDORES:
        p = bruto.get(pid) or {}
        if not isinstance(p, dict):
            continue
        for mid, m in (p.get("models") or {}).items():
            if not isinstance(m, dict):
                continue
            c = m.get("cost") or {}
            if not isinstance(c, dict):
                continue
            # 0/0 é marcador de "grátis por enquanto", não tarifa.
            if not c.get("input") and not c.get("output"):
                continue
            entrada = {
                "provider": pid,
                "input": float(c["input"]),
                "output": float(c["output"]),
                # None, não um palpite: aplicar a regra 1.25x/0.1x da Anthropic num modelo de
                # outro provedor é inventar tarifa. Quem decide o que fazer com o None é o
                # _rate(), e ele MARCA a linha como estimada.
                "cache_read": float(c["cache_read"]) if c.get("cache_read") is not None else None,
                "cache_write": float(c["cache_write"]) if c.get("cache_write") is not None else None,
            }
            out.setdefault(mid, entrada)
            out.setdefault(f"{pid}/{mid}", entrada)
    return out


def _rate(d: dict, origin: str) -> Rate:
    entrada = float(d["input"])
    cr, cw = d.get("cache_read"), d.get("cache_write")
    estimado = cr is None or cw is None
    return Rate(
        input=entrada,
        output=float(d["output"]),
        # Sem preço publicado, cache conta como input — e a linha vai MARCADA.
        cache_read=float(cr) if cr is not None else entrada,
        cache_write=float(cw) if cw is not None else entrada,
        provider=d.get("provider", "?"),
        origin=origin,
        cache_estimado=estimado,
    )
