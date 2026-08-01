"""Agregação do painel de custos: linhas das três fontes -> cortes por dimensão.

Quem LÊ as fontes é o costs_sources; quem sabe preço é o pricing. Aqui só se soma.
"""
from __future__ import annotations

import json
import logging
import time
import urllib.request
from collections import defaultdict
from datetime import datetime, timedelta

from app import pricing
from app.costs_sources import LOCAL, UsageRow, coletar
from app.models import Applied, CostReport, DimBucket, KindBucket, RateInfo

TIPOS = ("input", "output", "cache_write", "cache_read")
PERIODOS = {"7d": 7, "30d": 30, "90d": 90}


# _account_info NÃO mora mais aqui: virou costs_sources.account_info. Este módulo importa
# costs_sources, então manter a função aqui fecharia um ciclo.


def _custo_da_linha(r: UsageRow) -> dict[str, float] | None:
    rate = pricing.rate_for(r.model)
    if rate is None:
        return None
    return pricing.custo(rate, r.input, r.output, r.cache_write, r.cache_read)


def _somar(b: dict, r: UsageRow, c: dict[str, float] | None) -> None:
    b["sessions"] += 1
    b["input"] += r.input
    b["output"] += r.output
    b["cache_write"] += r.cache_write
    b["cache_read"] += r.cache_read
    if c:
        for t in TIPOS:
            b[f"cost_{t}"] += c[t]
        b["cost"] += sum(c.values())


def _zero() -> dict:
    z = {"sessions": 0, "cost": 0.0}
    for t in TIPOS:
        z[t] = 0
        z[f"cost_{t}"] = 0.0
    return z


def agrupar(linhas: list[UsageRow], chave, custos: dict[int, dict | None]) -> list[DimBucket]:
    agg: dict[str, dict] = defaultdict(_zero)
    for i, r in enumerate(linhas):
        _somar(agg[chave(r)], r, custos[i])
    return sorted(
        (DimBucket(key=k, **v) for k, v in agg.items()),
        key=lambda b: (-b.cost, b.key),
    )


def _totais(linhas: list[UsageRow]) -> dict:
    b = _zero()
    for r in linhas:
        _somar(b, r, _custo_da_linha(r))
    return b


def _janela_anterior(todas: list[UsageRow], dias: int, now: datetime) -> DimBucket | None:
    """Totais da janela imediatamente anterior, do MESMO tamanho.

    Devolve None quando a janela anterior tem menos de 1/3 dos dias com registro: comparar
    contra um período quase vazio não é comparação, é divisão pelo vazio (medido: ▲574% porque
    "30 dias anteriores" tinha 3 dias de dado). Melhor a tela dizer "sem período anterior
    completo" do que inventar um número.
    """
    fim = (now - timedelta(days=dias)).date()
    ini = (now - timedelta(days=dias * 2 - 1)).date()
    janela = [r for r in todas if ini <= r.ts.date() <= fim]
    if not janela:
        return None
    cobertos = len({r.ts.date() for r in janela})
    if cobertos * 3 < dias:
        return None
    return DimBucket(key="anterior", **_totais(janela))


def montar(linhas: list[UsageRow], period: str = "all",
           now: datetime | None = None) -> CostReport:
    """Agrega as linhas já coletadas. O corte do dia é do SERVIDOR, no fuso do servidor — o
    front nunca recalcula data a partir de new Date()."""
    now = now or datetime.now(LOCAL)
    dias = PERIODOS.get(period)
    # A janela anterior sai da lista COMPLETA e ANTES do corte: depois de filtrar, o dado dela
    # não existe mais.
    anterior = _janela_anterior(linhas, dias, now) if dias else None
    if dias:
        corte = (now - timedelta(days=dias - 1)).date()
        linhas = [r for r in linhas if r.ts.date() >= corte]

    # `custos` é indexado pela POSIÇÃO em `linhas` e tem que ser montado DEPOIS do corte:
    # construir antes desalinharia os índices que o agrupar() usa, em silêncio.
    custos = {i: _custo_da_linha(r) for i, r in enumerate(linhas)}
    total = _zero()
    kinds = {t: {"tokens": 0, "cost": 0.0} for t in TIPOS}
    sem_cache = 0.0
    equivalente = 0.0
    sem_tarifa: set[str] = set()
    rates: dict[str, RateInfo] = {}
    for i, r in enumerate(linhas):
        c = custos[i]
        _somar(total, r, c)
        for t in TIPOS:
            kinds[t]["tokens"] += getattr(r, t)
            if c:
                kinds[t]["cost"] += c[t]
        rate = pricing.rate_for(r.model)
        if rate is None:
            # O id CANÔNICO, igual ao by_model: guardar o cru faria duas grafias do mesmo
            # modelo virarem duas linhas de "sem tarifa".
            canon = pricing.canonizar(r.model)
            # IGNORADOS não são modelo ('<synthetic>', 'unknown', ''): rate_for devolve None pra
            # eles também, e listá-los como "sem tarifa" põe um traço na tabela sugerindo preço
            # faltando — que é outra coisa (pricing.py:111). O linhas_claude já os descarta; as
            # linhas de Codex e Pi não passam por lá.
            if canon not in pricing.IGNORADOS:
                sem_tarifa.add(canon)
            continue
        # Preço cheio: os mesmos tokens se NENHUM fosse cache.
        sem_cache += ((r.input + r.cache_write + r.cache_read) / 1e6 * rate.input
                      + r.output / 1e6 * rate.output)
        # Equivalente-input: cada tipo pesado pela própria tarifa. Sem preço de input não há
        # régua pra converter, e a linha simplesmente não entra (o custo dela já entrou).
        if rate.input:
            equivalente += (r.input
                            + r.output * (rate.output / rate.input)
                            + r.cache_write * (rate.cache_write / rate.input)
                            + r.cache_read * (rate.cache_read / rate.input))
        rates.setdefault(pricing.canonizar(r.model), RateInfo(
            model=pricing.canonizar(r.model), provider=rate.provider,
            input=rate.input, output=rate.output, cache_read=rate.cache_read,
            cache_write=rate.cache_write, origin=rate.origin,
            cache_estimado=rate.cache_estimado))

    return CostReport(
        totals=DimBucket(key="totals", **total),
        by_day=sorted(agrupar(linhas, lambda r: r.ts.strftime("%Y-%m-%d"), custos),
                      key=lambda b: b.key, reverse=True),
        by_provider=agrupar(linhas, lambda r: r.provider, custos),
        by_source=agrupar(linhas, lambda r: r.source, custos),
        by_project=agrupar(linhas, lambda r: r.project, custos),
        by_model=agrupar(linhas, lambda r: pricing.canonizar(r.model), custos),
        by_kind=[KindBucket(kind=t, **kinds[t]) for t in TIPOS],
        rates=sorted(rates.values(), key=lambda r: r.model),
        sem_tarifa=sorted(sem_tarifa),
        custo_sem_cache=sem_cache,
        equivalente_cobrado=int(equivalente),
        anterior=anterior,
        applied=Applied(period=period),
        usd_brl=usd_brl(),
    )


def report(period: str = "all", now: datetime | None = None) -> CostReport:
    return montar(coletar(), period=period, now=now)


# Cotação USD/BRL: cache em memória de 1h. Falha também "conta" como tentativa (atualiza o
# timestamp) — senão cada request offline pagaria os 3s de timeout até a rede voltar.
_RATE_URL = "https://economia.awesomeapi.com.br/json/last/USD-BRL"
_rate: float | None = None
_rate_at: float = 0.0


def usd_brl() -> float | None:
    global _rate, _rate_at
    now = time.monotonic()
    if _rate_at and now - _rate_at < 3600:
        return _rate
    _rate_at = now
    try:
        with urllib.request.urlopen(_RATE_URL, timeout=3) as r:
            _rate = float(json.load(r)["USDBRL"]["bid"])
    except Exception as e:
        # Mantém a última cotação conhecida (ou None) — front cai pra USD. O log distingue
        # timeout de mudança de schema da API (senão os dois falham idênticos pra sempre).
        logging.getLogger(__name__).warning("cotação USD/BRL falhou: %r", e)
    return _rate
