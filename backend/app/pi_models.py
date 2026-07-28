"""Catalogo de modelos + nivel de raciocinio de uma sessao Pi.

Por que NAO e o caminho do Claude (dirigir o picker com send-keys, ver `model_picker.py`):
o `/model` do Pi e uma lista com CAMPO DE BUSCA de ~300 modelos (rodape "(1/301)", 10 linhas
visiveis). Nao da pra enumerar do pane, nem navegar contando Down. E o nivel de raciocinio nem
comando proprio tem: mora em `/settings` -> "Thinking level" (submenu), e o conjunto de niveis MUDA
por modelo (medido: glm-5.2 = off/low/medium/high/xhigh; k3 = low/high/max).

O caminho aqui e o contrario da raspagem: a extensao `scripts/pi/cp-state.ts` (que ja instalamos
pro estado working/idle) pergunta pro proprio Pi via API publica de extensao e:
  * publica o catalogo num sidecar JSON -> este modulo LE (nada de parse de tela);
  * registra `/cp-model <provider> <id>` e `/cp-think <nivel>` -> este modulo os DIGITA e o Pi
    aplica com `pi.setModel()` / `pi.setThinkingLevel()`.

Sidecar ausente = extensao nao instalada ou desatualizada (o app cai fora com 409 em vez de
inventar uma lista). Depois de aplicar, relemos o sidecar: o Pi CLAMPA o nivel pro que o modelo
suporta (agent-session.js:1277), entao "pedi xhigh" nao prova "ficou xhigh".
"""

import json
import time
from pathlib import Path

# Ordem canonica dos niveis (@earendil-works/pi-ai models.js:391, EXTENDED_THINKING_LEVELS).
# Estatico de proposito: e o UNIVERSO possivel, so pra recusar lixo antes de digitar na TUI. Os
# niveis que ESTE modelo aceita vem do sidecar (`levels`), porque variam por modelo.
LEVELS = ("off", "minimal", "low", "medium", "high", "xhigh", "max")

_SUBDIR = ".claude-pocket-pi"


class PiModelError(Exception):
    """Falha de catalogo/entrada. `status` vira o HTTP correspondente na camada de API."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def sidecar_path(jsonl: str, config_dir: Path | None = None) -> Path:
    """Sidecar do catalogo desta sessao. Chave = stem do .jsonl, a MESMA do marcador de estado
    (o backend ja resolve nome da sessao -> .jsonl), e nao o pane: o tmux reusa %pane_id."""
    base = config_dir or Path.home() / ".claude"
    return base / _SUBDIR / "models" / f"{Path(jsonl).stem}.json"


def read_catalog(jsonl: str, config_dir: Path | None = None) -> dict | None:
    """Catalogo publicado pela extensao, ou None se ausente/ilegivel/sem o formato esperado.

    None NAO e erro tecnico e sim "esta sessao nao sabe se apresentar": Pi velho, extensao nao
    instalada, ou sessao que nunca disparou session_start. O caller traduz pra 409 com instrucao.
    """
    try:
        data = json.loads(sidecar_path(jsonl, config_dir).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or not isinstance(data.get("models"), list):
        return None
    return data


def _clean(value: str, field: str) -> str:
    """Token que vai VIRAR TECLA na TUI: sem espaco (o comando separa os argumentos por espaco) e
    sem caractere de controle. Validado ANTES de qualquer send-keys."""
    v = value.strip()
    if not v or any(c.isspace() for c in v) or any(ord(c) < 32 for c in v):
        raise PiModelError(422, f"{field} invalido: {value!r}")
    return v


def model_command(provider: str, model_id: str) -> str:
    # Separador ESPACO e nao "/": o id do modelo ja tem barra (`cline-pass/glm-5.2` no provedor
    # `clinepass`), entao "provider/id" seria ambiguo pra quem parseia do outro lado.
    return f"/cp-model {_clean(provider, 'provider')} {_clean(model_id, 'model')}"


def think_command(level: str) -> str:
    lv = level.strip().lower()
    if lv not in LEVELS:
        raise PiModelError(422, f"nivel desconhecido: {level!r}")
    return f"/cp-think {lv}"


def confirms(catalog: dict, provider: str | None, model_id: str | None,
             level: str | None) -> bool:
    """O catalogo relido PROVA que o pedido pegou?

    Modelo: igualdade seca (provider+id). O `/cp-model` do Pi pode RECUSAR sem levantar nada — sem
    chave pro provedor, `setModel` devolve false, ele notifica dentro do TUI e o sidecar continua
    com o modelo VELHO (cp-state.ts republica o catalogo de qualquer jeito).

    Nivel: so exigimos igualdade quando o nivel pedido esta entre os `levels` do modelo que ficou.
    Fora deles o Pi CLAMPA (agent-session.js:1277) e cair em outro nivel e o comportamento CERTO —
    exigir igualdade ali transformaria um clamp legitimo em erro.
    """
    if model_id:
        cur = catalog.get("current")
        if not isinstance(cur, dict) or cur.get("provider") != provider or cur.get("id") != model_id:
            return False
    if level:
        lv = level.strip().lower()
        if lv in (catalog.get("levels") or []) and catalog.get("thinking") != lv:
            return False
    return True


def read_back(jsonl: str, config_dir: Path | None, provider: str | None = None,
              model_id: str | None = None, level: str | None = None,
              deadline: float = 2.0, interval: float = 0.15) -> dict | None:
    """Relê o sidecar ATÉ ele confirmar o pedido (ou estourar o prazo) e devolve o que leu.

    Sondagem em vez de um sleep fixo maior: o `/cp-model` é assíncrono do outro lado (`await
    pi.setModel`) e um settle fixo que fique curto lê o catálogo ANTERIOR e reporta "trocou" sobre
    dado velho. Estourar o prazo não é erro aqui — devolve a última leitura e quem chama decide,
    com `confirms()`, entre "o Pi recusou" e "não deu pra confirmar".
    """
    end = time.monotonic() + deadline
    while True:
        cat = read_catalog(jsonl, config_dir)
        if cat is not None and confirms(cat, provider, model_id, level):
            return cat
        if time.monotonic() >= end:
            return cat
        time.sleep(interval)


def check_known(catalog: dict, provider: str, model_id: str) -> None:
    """Recusa um modelo que nao esta no catalogo: o comando na TUI so notificaria 'desconhecido' e
    o app reportaria sucesso sobre um no-op."""
    for m in catalog.get("models", []):
        if isinstance(m, dict) and m.get("provider") == provider and m.get("id") == model_id:
            return
    raise PiModelError(422, f"modelo fora do catalogo: {provider}/{model_id}")
