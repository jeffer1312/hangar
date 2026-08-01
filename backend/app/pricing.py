"""Tarifa por modelo. Núcleo: o catálogo e como uma entrada do models.dev vira Rate.

REGRA DURA: modelo sem tarifa devolve None. Nunca zero — zero afirma "não custou nada", que é
mentira diferente de "não sei o preço". Foi o fallback silencioso pra Sonnet (costs.py:29) que
motivou este módulo.

REDE NUNCA NO CAMINHO DO REQUEST: rate_for() lê disco. Quem baixa é
atualizar_em_background(), chamado no startup. fetchCostsForServer aborta em 4s
(frontend/src/lib/api.ts:166).
"""
from __future__ import annotations

import json
import logging
import os
import threading
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path

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
            # Sem preço de INPUT a entrada não serve pra nada aqui: cache e "equivalente cobrado"
            # são ambos relativos a ele. E a linha seguinte faz float(c["input"]) — com `and`, um
            # {"output": 5} sem input estourava KeyError. Esta é a primeira tarefa a passar dado
            # AO VIVO pro slim(), então a fragilidade sai de teórica.
            if not c.get("input"):
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


_log = logging.getLogger(__name__)

URL = "https://models.dev/api.json"
SNAPSHOT = Path(__file__).with_name("pricing_data.json")
_CACHE_DIR = Path.home() / ".claude" / ".claude-pocket-pricing"
_TTL = 24 * 3600

# Prefixos de provedor que o Pi e os gateways grudam no id. A ordem não importa: só um casa.
_PREFIXOS = (
    "anthropic/", "openai/", "moonshot/", "moonshotai/", "deepseek/",
    "cline-pass/", "clinepass/", "openrouter/", "zhipuai/", "google/", "cx/",
)
# Apelido -> id do models.dev. O log grava o nome do MOTOR, o catálogo conhece o do MODELO.
_APELIDOS = {
    "k3": "kimi-k3",
    "k3-256k": "kimi-k3",
    "kimi-for-coding": "kimi-k3",
    "gpt-5.6-sol-high": "gpt-5.6-sol",
}
# Não são modelos: não entram no relatório nem viram "sem tarifa" (traço misterioso sugere
# preço faltando, que é outra coisa).
IGNORADOS = frozenset({"<synthetic>", "unknown", "mock-engine-1", ""})


_lock = threading.Lock()
_cat: dict[str, Rate] | None = None
_overrides: dict[str, dict] | None = None


def invalidar_cache() -> None:
    global _cat, _overrides
    with _lock:
        _cat = None
        _overrides = None


def catalogo_de_bruto(bruto: dict) -> dict[str, Rate]:
    """api.json cru do models.dev -> catálogo de Rate. Usa o slim() da Task 1: uma regra só,
    num lugar só — o script e o backend não podem divergir sobre o que entra no catálogo."""
    return {k: _rate(v, "models.dev") for k, v in slim(bruto).items()}


def _ler_json(p: Path) -> dict | None:
    """None em qualquer problema, inclusive JSON válido do tipo errado. `null` e lista não
    levantam ValueError, e um .get() em cima disso já derrubou o app inteiro uma vez."""
    try:
        d = json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    return d if isinstance(d, dict) else None


def _carregar() -> dict[str, Rate]:
    global _cat
    if _cat is not None:
        return _cat
    with _lock:
        if _cat is not None:
            return _cat
        cache = _ler_json(_CACHE_DIR / "models.dev.json")
        if cache and isinstance(cache.get("modelos"), dict):
            _cat = {k: _rate(v, "models.dev") for k, v in cache["modelos"].items()}
        else:
            snap = _ler_json(SNAPSHOT) or {"modelos": {}}
            _cat = {k: _rate(v, "snapshot") for k, v in (snap.get("modelos") or {}).items()}
        return _cat


def _carregar_overrides() -> dict[str, dict]:
    global _overrides
    if _overrides is None:
        with _lock:
            if _overrides is None:
                d = _ler_json(_CACHE_DIR / "overrides.json") or {}
                _overrides = {k: v for k, v in d.items() if isinstance(v, dict)}
    return _overrides


def gravar_override(model: str, tarifa: dict) -> None:
    _CACHE_DIR.mkdir(parents=True, exist_ok=True)
    p = _CACHE_DIR / "overrides.json"
    atual = _ler_json(p) or {}
    atual[canonizar(model)] = {**tarifa, "provider": tarifa.get("provider", "override")}
    tmp = p.with_suffix(f".{os.getpid()}.tmp")   # pid no tmp: dois writers não se entrelaçam
    tmp.write_text(json.dumps(atual, indent=1, ensure_ascii=False))
    tmp.replace(p)
    invalidar_cache()


def canonizar(model: str) -> str:
    """Id do log -> id do models.dev.

    Tenta o id CRU primeiro: há modelo cujo nome contém barra, e desmontar antes de olhar faria
    ele sumir.

    Descasca prefixo em LAÇO, não uma vez só: o Pi empilha gateway + provedor
    ('openrouter/deepseek/deepseek-v4-flash'), e parar no primeiro deixaria
    'deepseek/deepseek-v4-flash' — forma que EXISTE no catálogo (o slim() registra tanto o id nu
    quanto 'provedor/id'), então só checar o catálogo depois de esgotar os prefixos garante
    chegar no id nu de verdade. O laço tem teto porque a lista de prefixos é finita e cada volta
    encurta a string.
    """
    m = (model or "").strip()
    if m in _carregar():
        return m
    base = m
    mudou = True
    while mudou:
        mudou = False
        for p in _PREFIXOS:
            if base.startswith(p):
                base = base[len(p):]
                mudou = True
                break
    # Checar o catálogo só DEPOIS de esgotar os prefixos, não a cada volta: o slim() (Task 1)
    # registra tanto 'mid' quanto 'pid/mid', então uma forma intermediária como
    # 'deepseek/deepseek-v4-flash' já é uma chave válida por si só — checar no meio do laço
    # devolveria essa forma prefixada e nunca chegaria no id nu.
    if base in _carregar():
        return base
    return _APELIDOS.get(base, base)


def rate_for(model: str) -> Rate | None:
    if (model or "").strip() in IGNORADOS:
        return None
    mid = canonizar(model)
    ov = _carregar_overrides().get(mid)
    if ov and "input" in ov and "output" in ov:
        return _rate(ov, "override")
    return _carregar().get(mid)


def provider_for(model: str) -> str | None:
    r = rate_for(model)
    return r.provider if r else None


def custo(rate: Rate, entrada: int, saida: int, cw: int, cr: int) -> dict[str, float]:
    return {
        "input": entrada / 1e6 * rate.input,
        "output": saida / 1e6 * rate.output,
        "cache_write": cw / 1e6 * rate.cache_write,
        "cache_read": cr / 1e6 * rate.cache_read,
    }


_ultima_tentativa = 0.0


def atualizar_em_background() -> None:
    """Baixa o catálogo numa thread. Chamado no startup do backend, NUNCA num request.

    CP_PRICING_OFFLINE=1 desliga — máquina sem saída pra internet é justamente o motivo de o
    snapshot existir."""
    if os.environ.get("CP_PRICING_OFFLINE") == "1":
        return
    threading.Thread(target=_baixar, name="pricing-refresh", daemon=True).start()


def _baixar() -> None:
    global _ultima_tentativa
    destino = _CACHE_DIR / "models.dev.json"
    agora = time.time()
    # A tentativa conta mesmo falhando (mesma disciplina do usd_brl em costs.py:165): senão,
    # cada startup offline paga o timeout de novo.
    if _ultima_tentativa and agora - _ultima_tentativa < _TTL:
        return
    try:
        if destino.is_file() and agora - destino.stat().st_mtime < _TTL:
            return
    except OSError:
        pass
    _ultima_tentativa = agora
    # UA próprio é OBRIGATÓRIO: o models.dev responde 403 ao User-Agent default do urllib
    # (`Python-urllib/3.x`) — medido na Task 1, onde o mesmo download só passou com header.
    # Sem isto, toda atualização de tarifa falha calada e o app fica preso no snapshot.
    req = urllib.request.Request(URL, headers={"User-Agent": "claude-cockpit/pricing"})
    try:
        if destino.is_file():
            ts = time.strftime("%a, %d %b %Y %H:%M:%S GMT", time.gmtime(destino.stat().st_mtime))
            req.add_header("If-Modified-Since", ts)   # caso normal vira 304, não 3,2 MB
        with urllib.request.urlopen(req, timeout=30) as r:
            bruto = json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 304:
            destino.touch()
            return
        _log.warning("tarifas: models.dev respondeu %s", e.code)
        return
    except Exception as e:
        _log.warning("tarifas: falha ao baixar models.dev: %r", e)
        return
    try:
        cat = catalogo_de_bruto(bruto)
    except Exception as e:
        # Formato mudou. O snapshot NÃO salva (é o mesmo parser), então o que salva é isto:
        # falhar aqui deixa o cache anterior de pé em vez de gravar lixo.
        _log.warning("tarifas: formato do models.dev não reconhecido: %r", e)
        return
    _CACHE_DIR.mkdir(parents=True, exist_ok=True)
    payload = {"modelos": {k: {
        "provider": v.provider, "input": v.input, "output": v.output,
        "cache_read": None if v.cache_estimado else v.cache_read,
        "cache_write": None if v.cache_estimado else v.cache_write,
    } for k, v in cat.items()}}
    tmp = destino.with_suffix(f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False))
    tmp.replace(destino)
    invalidar_cache()
    _log.info("tarifas: %d modelos atualizados do models.dev", len(cat))
