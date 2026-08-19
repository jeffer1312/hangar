"""Catálogo de modelos do Kimi + leitura da confirmação de troca no pane.

Por que o caminho é diferente do Pi: o Pi tem extensão com API (`pi.setModel`, sidecar JSON) e o
Kimi não tem canal nenhum de volta — só hooks stdin->stdout. O que sobra é o caminho do Claude
(dirigir a TUI), medido ao vivo em 19/08/2026 (Kimi Code 0.37.2):

  * `/model <alias>` com argumento NÃO troca direto: alias completo (`apikey/k3-256k`) abre o
    picker interativo do mesmo jeito; alias curto desconhecido imprime "Unknown model alias".
  * O picker tem BUSCA ("type to search") e a busca casa o ALIAS completo, não só o nome de
    exibição — `apikey/k3` filtra até sobrar um item. É o que torna a escolha determinística: os
    itens do picker NÃO aparecem no capture-pane (a lista renderiza num overlay que a captura lê
    como linhas em branco), então navegar contando Down seria às cegas.
  * Enter aplica e grava o alias como `default_model` GLOBAL no config.toml (medido: o arquivo foi
    reescrito na hora). Alt+S aplica SÓ na sessão ("Switched to K3 with thinking high for this
    session only.") e não toca o arquivo — é a tecla que este módulo usa, sempre.
  * A confirmação é a linha "Switched to <display> with thinking <nível>[ for this session only]."
    que aparece no scrollback — o read-back daqui, em vez do sidecar do Pi.

O catálogo NÃO vem da TUI: mora no `~/.kimi-code/config.toml`, seções `[models."<alias>"]`, que já
trazem display_name, max_context_size e os níveis de esforço de cada modelo. Não existe
`kimi --list-models` (o equivalente do `pi --list-models` do pi_catalog.py).
"""

import re
import tomllib
from pathlib import Path

CONFIG = Path.home() / ".kimi-code" / "config.toml"


class KimiModelError(Exception):
    """Falha de catálogo/entrada. `status` vira o HTTP correspondente na camada de API."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def read_catalog(path: Path | None = None) -> dict | None:
    """Catálogo do config.toml: {"default": alias, "models": [...]}, ou None se ausente/ilegível.

    None NÃO é erro técnico e sim "esta máquina não sabe se apresentar": sem o config não há lista
    real, e inventar uma faria o app oferecer aliases que a busca do picker não encontraria. O
    caller traduz pra 409. Relido a cada chamada de propósito: o arquivo é pequeno e editá-lo é o
    jeito oficial de mexer em modelos (`kimi provider`), então cache ficaria velho justo no cenário
    de uso.
    """
    try:
        data = tomllib.loads((path or CONFIG).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    models = data.get("models")
    if not isinstance(models, dict) or not models:
        return None
    out = []
    for alias, m in sorted(models.items()):
        if not isinstance(m, dict):
            continue
        provider, _, mid = alias.partition("/")
        out.append({
            "alias": alias,
            "provider": provider,
            "id": m.get("model") or mid,
            "name": m.get("display_name") or mid,
            # Mesma chave do ModelOption do front (context_length), pra etiqueta "256K" da tela de
            # abertura sair sem formato novo — ver os QUATRO formatos comentados em api.ts.
            "context_length": m.get("max_context_size"),
            "efforts": m.get("support_efforts") or [],
            "default_effort": m.get("default_effort"),
        })
    return {"default": data.get("default_model"), "models": out}


def clean_alias(value: str) -> str:
    """Token que vai VIRAR TECLA na TUI (a busca do picker): sem espaço e sem caractere de
    controle. Validado ANTES de qualquer send-keys. Mesma regra do pi_models._clean."""
    v = value.strip()
    if not v or any(c.isspace() for c in v) or any(ord(c) < 32 for c in v):
        raise KimiModelError(422, f"alias invalido: {value!r}")
    return v


def check_known(catalog: dict, alias: str) -> dict:
    """Devolve a entrada do catálogo pro alias, ou recusa: um alias fora da lista filtraria a busca
    até ZERO itens e o Alt+S não aplicaria nada — o app reportaria sucesso sobre um no-op."""
    alias = clean_alias(alias)
    for m in catalog.get("models", []):
        if m.get("alias") == alias:
            return m
    raise KimiModelError(422, f"modelo fora do catalogo: {alias}")


_SWITCHED = re.compile(r"Switched to (.+?) with thinking .+?( for this session only)?\.\s*$")


def parse_switched(pane: str) -> dict | None:
    """A troca MAIS RECENTE visível no pane (a última linha "Switched to …"), ou None.

    O scrollback acumula as trocas anteriores, então quem confirma compara isto ANTES e DEPOIS de
    dirigir — igualdade com a linha velha não prova nada (o mesmo bug do `k3-256k`->`k3` comentado
    no set_engine_model).
    """
    achado = None
    for line in pane.splitlines():
        m = _SWITCHED.search(line)
        if m:
            achado = {"name": m.group(1), "session_only": bool(m.group(2)), "raw": line.strip()}
    return achado


def confirms(switched: dict | None, antes: dict | None, display: str) -> bool:
    """A linha nova PROVA que a troca pedida pegou: apareceu DEPOIS da baseline e nomeia o display
    do alias pedido. (Displays repetem entre providers — `K3` existe no apikey e no kimi-code —,
    então o nome confirma o MODELO; quem garante o provider é a busca pelo alias completo.)"""
    if switched is None or switched == antes:
        return False
    return switched.get("name") == display
