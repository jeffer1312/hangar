#!/usr/bin/env python3
"""Baixa o catálogo do models.dev e gera o snapshot versionado + a fixture de teste.

Rodar quando quiser atualizar o snapshot do repositório:

    python3 scripts/fetch_model_prices.py

O backend NÃO chama este script: ele lê o JSON gerado. A atualização em produção é o cache
em disco do pricing.py; o snapshot é o piso para máquina que nunca teve rede.
"""
from __future__ import annotations

import json
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

RAIZ = Path(__file__).resolve().parents[1]
# O SCRIPT depende do backend, nunca o contrário: `scripts/` não é pacote e em produção o
# caminho pode nem existir, enquanto rodar a partir da raiz do repo é premissa segura.
sys.path.insert(0, str(RAIZ / "backend"))
from app.pricing import PROVEDORES, slim  # noqa: E402

URL = "https://models.dev/api.json"
SNAPSHOT = RAIZ / "backend" / "app" / "pricing_data.json"
FIXTURE = RAIZ / "backend" / "tests" / "fixtures" / "models_dev_recorte.json"

# Modelos citados pelo NOME nos testes de pricing.py. O corte de 6 abaixo é por posição crua do
# JSON do models.dev — sem isto, reexecutar o script pode apagar um deles em silêncio se ele não
# estiver entre os 6 primeiros daquele provedor (foi o caso do kimi-k3, medido).
_ANCORAS = {"anthropic": "claude-opus-5", "moonshotai": "kimi-k3", "openai": "gpt-5.6-sol"}


def _recorte(bruto: dict) -> dict:
    """Fixture: um pouco de cada provedor canônico + a armadilha de preço zero de uma revenda."""
    rec: dict = {}
    for pid in PROVEDORES:
        p = bruto.get(pid)
        if not isinstance(p, dict):
            continue
        todos = p.get("models") or {}
        modelos = dict(list(todos.items())[:6])
        ancora = _ANCORAS.get(pid)
        if ancora and ancora not in modelos and ancora in todos:
            modelos[ancora] = todos[ancora]
        rec[pid] = {"name": p.get("name", pid), "models": modelos}
    for pid, p in bruto.items():
        if pid in PROVEDORES or not isinstance(p, dict):
            continue
        armadilha = {
            mid: m for mid, m in (p.get("models") or {}).items()
            if isinstance(m, dict) and not (m.get("cost") or {}).get("input")
        }
        if armadilha:
            rec[pid] = {"name": p.get("name", pid), "models": dict(list(armadilha.items())[:2])}
            break
    return rec


def main() -> None:
    # models.dev devolve 403 pro User-Agent default do urllib (confirmado: curl sem UA passa,
    # curl com UA "Python-urllib/..." toma o mesmo 403) — não é bloqueio de rede, é bot-detection.
    req = urllib.request.Request(URL, headers={"User-Agent": "claude-cockpit/fetch_model_prices"})
    with urllib.request.urlopen(req, timeout=30) as r:
        bruto = json.load(r)
    modelos = slim(bruto)
    SNAPSHOT.parent.mkdir(parents=True, exist_ok=True)
    SNAPSHOT.write_text(json.dumps({
        "gerado_em": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "modelos": modelos,
    }, indent=1, sort_keys=True, ensure_ascii=False) + "\n")
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(json.dumps(_recorte(bruto), indent=1, sort_keys=True, ensure_ascii=False) + "\n")
    print(f"{len(modelos)} entradas -> {SNAPSHOT}")
    print(f"recorte -> {FIXTURE}")


if __name__ == "__main__":
    main()
