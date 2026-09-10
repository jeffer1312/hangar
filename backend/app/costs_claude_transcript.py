"""Uso do Claude Code lido do TRANSCRIPT, não do resumo de plugin nenhum.

Três motivos, medidos em 01/08/2026 nesta máquina (números movem toda semana — são foto, não
constante; refazer a medição do Step 6 da Task 2 antes de confiar neles de novo):

1. O gasto de SUBAGENTE não está na conta. Numa sessão com 14 subagentes, o `costs.jsonl`
   registra 183.855.995 de cache lido — idêntico ao transcript do pai sozinho —, enquanto os
   subagentes somam outros 12.809.654 que o plugin nunca viu. No total, medido em 01/08/2026:
   4,80 Bi contra 26,15 Bi de conversa — 15,5% do volume, em 3.160 registros (446 de conversa +
   2.714 de subagente).
2. O app dependia de plugin de terceiro (`cost-tracker.js` do ECC) para função própria.
   Codex e Pi já leem o transcript original; o Claude era o único fora do padrão.
3. O plugin só cobre a partir de 27/06; os transcripts começam em 12/06.

REGRA DE ACUMULAÇÃO: aqui é SOMA por turno. O `costs.jsonl` era cumulativo (última linha
vence). Trocar a regra entre as fontes não quebra nada e devolve número plausível e errado.

CACHE EM DISCO, e POR RAIZ: a varredura fria mede 13,6s sobre 3.202 arquivos e 5,2 GB, contra
o `AbortSignal.timeout(4000)` do cliente. Cache só em memória pagaria isso a cada restart; um
cache global de raiz única seria apagado pela segunda conta configurada.
"""
from __future__ import annotations

import hashlib
import json
import logging
import os
import threading
from dataclasses import dataclass, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import atomico, pricing

_log = logging.getLogger(__name__)

# `LOCAL` é cópia proposital: `costs_sources` vai importar ESTE módulo, então importar de lá
# fecharia ciclo. Não "arrume" unificando — quebra o import.
LOCAL = timezone(timedelta(hours=-3))

# Suba isto ao mudar o formato do resumo, senão o cache velho é servido pra sempre.
CACHE_VERSAO = 2

# Marcador do subagente. O caminho é `<projeto>/<sessionId>/subagents/agent-*.jsonl`.
# Medido em 01/08/2026: 2.714 arquivos assim, contra 446 de conversa — cresce toda semana.
_DIR_SUBAGENTE = "subagents"

_CACHE_DIR = Path.home() / ".claude" / ".hangar-custos"
_lock = threading.Lock()
_mem: dict[str, dict[str, tuple[tuple[int, int], list[dict]]]] = {}


@dataclass(frozen=True)
class UsoSessao:
    session_id: str       # id ÚNICO, derivado do CAMINHO relativo (ver `varrer`)
    ts: datetime          # Primeira resposta deste segmento diário.
    model: str
    cwd: str
    subagente: bool
    input: int
    output: int
    cache_write: int
    cache_read: int
    cache_write_1h: int = 0


def raiz_projetos(config_dir: Path | None = None) -> Path:
    """Onde o Claude Code guarda os transcripts: `<config>/projects/`, diretório GLOBAL — não
    dentro do repositório. Cada subpasta tem o caminho do projeto com barras viradas em traço.

    `config_dir` é o que importa na prática: o app suporta MAIS DE UM diretório de configuração
    e `coletar()` chama o leitor uma vez por diretório. Ignorar esse argumento leria a mesma
    raiz N vezes e contaria o gasto em dobro, dividido entre contas erradas.
    """
    if config_dir is not None:
        return Path(config_dir) / "projects"
    base = os.environ.get("CLAUDE_CONFIG_DIR")
    return (Path(base) if base else Path.home() / ".claude") / "projects"


def _int(v) -> int:
    try:
        return int(v or 0)
    except (TypeError, ValueError):
        return 0


def ler_transcript(path: Path) -> list[UsoSessao]:
    """Uso por resposta, separado por dia/modelo; blocos da mesma resposta não somam novamente."""
    respostas: dict[tuple, UsoSessao] = {}
    try:
        f = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return []
    with f:
        for numero, linha in enumerate(f):
            # Pré-filtro barato: 5,2 GB de transcript e só a minoria das linhas tem uso.
            # Sem isto o json.loads roda em tudo e a varredura triplica.
            if '"usage"' not in linha:
                continue
            try:
                d = json.loads(linha)
            except json.JSONDecodeError:
                continue
            if not isinstance(d, dict) or d.get("type") != "assistant":
                continue
            msg = d.get("message")
            u = msg.get("usage") if isinstance(msg, dict) else None
            if not isinstance(u, dict):
                continue
            m = msg.get("model")
            if isinstance(m, str) and m.strip() in pricing.IGNORADOS:
                continue
            ts = d.get("timestamp")
            if not isinstance(ts, str):
                continue
            try:
                quando = datetime.fromisoformat(ts.replace("Z", "+00:00")).astimezone(LOCAL)
            except ValueError:
                continue
            # Sem identidade não há prova de repetição: preserva as linhas antigas.
            key = (d.get("requestId"), msg["id"]) if msg.get("id") else (numero,)
            criacao = u.get("cache_creation")
            cache_1h = _int(criacao.get("ephemeral_1h_input_tokens")) if isinstance(criacao, dict) else 0
            respostas[key] = UsoSessao(
                session_id="", ts=quando, model=m or "?", cwd=d.get("cwd") or "",
                subagente=(_DIR_SUBAGENTE in path.parts),
                input=_int(u.get("input_tokens")), output=_int(u.get("output_tokens")),
                cache_write=_int(u.get("cache_creation_input_tokens")),
                cache_read=_int(u.get("cache_read_input_tokens")),
                cache_write_1h=min(max(0, cache_1h), max(0, _int(u.get("cache_creation_input_tokens")))))
    grupos: dict[tuple, UsoSessao] = {}
    for uso in respostas.values():
        key = (uso.ts.date(), uso.model, uso.cwd)
        antes = grupos.get(key)
        grupos[key] = uso if antes is None else replace(
            antes, input=antes.input + uso.input, output=antes.output + uso.output,
            cache_write=antes.cache_write + uso.cache_write,
            cache_read=antes.cache_read + uso.cache_read,
            cache_write_1h=antes.cache_write_1h + uso.cache_write_1h)
    return sorted(grupos.values(), key=lambda u: (u.ts, u.model, u.cwd))


def invalidar_cache() -> None:
    global _mem
    with _lock:
        _mem = {}


def _caminho_cache(raiz: Path) -> Path:
    """Um arquivo por RAIZ. Cache único seria apagado pela segunda conta configurada."""
    h = hashlib.sha256(str(raiz.resolve()).encode()).hexdigest()[:16]
    return _CACHE_DIR / f"transcripts-{h}.json"


def _ler_cache(raiz: Path) -> dict[str, tuple[tuple[int, int], list[dict]]]:
    chave = str(raiz)
    if chave in _mem:
        return _mem[chave]
    bruto = None
    try:
        bruto = json.loads(_caminho_cache(raiz).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        bruto = None
    # Exigir dict: JSON válido do tipo errado (null, lista) não levanta ValueError.
    if not isinstance(bruto, dict) or bruto.get("versao") != CACHE_VERSAO:
        _mem[chave] = {}
        return _mem[chave]
    itens = bruto.get("itens")
    out: dict[str, tuple[tuple[int, int], list[dict]]] = {}
    if isinstance(itens, dict):
        for k, v in itens.items():
            # try por ITEM: `{"sig": ["abc", 1]}` é JSON válido e levantaria ValueError aqui,
            # fora do try do json.loads — mesmo formato de acidente do statusline.read().
            try:
                sig = v["sig"]
                out[k] = ((int(sig[0]), int(sig[1])), v.get("uso"))
            except (KeyError, TypeError, ValueError, IndexError):
                continue
    _mem[chave] = out
    return out


def _gravar_cache(raiz: Path, estado: dict) -> None:
    _CACHE_DIR.mkdir(parents=True, exist_ok=True)
    payload = {"versao": CACHE_VERSAO,
               "itens": {k: {"sig": [s[0], s[1]], "uso": u} for k, (s, u) in estado.items()}}
    destino = _caminho_cache(raiz)
    # pid no tmp: dois processos gravando com nome fixo fariam o rename promover bytes
    # entrelaçados — mesmo furo que o hangar_panel_common.py já corrigiu.
    tmp = destino.with_suffix(f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    atomico.substituir(tmp, destino)


def _serializar(u: UsoSessao) -> dict:
    return {"ts": u.ts.isoformat(), "model": u.model, "cwd": u.cwd,
            "subagente": u.subagente, "input": u.input, "output": u.output,
            "cache_write": u.cache_write, "cache_read": u.cache_read,
            "cache_write_1h": u.cache_write_1h}


def _desserializar(d: dict) -> UsoSessao | None:
    try:
        return UsoSessao(session_id="", ts=datetime.fromisoformat(d["ts"]),
                         model=d["model"], cwd=d.get("cwd", ""),
                         subagente=bool(d.get("subagente")),
                         input=int(d["input"]), output=int(d["output"]),
                         cache_write=int(d["cache_write"]), cache_read=int(d["cache_read"]),
                         cache_write_1h=int(d.get("cache_write_1h", 0)))
    except (KeyError, TypeError, ValueError):
        return None


def varrer(raiz: Path) -> list[UsoSessao]:
    """Todas as sessões de UMA raiz de projetos. Nunca vai à rede."""
    if not raiz.is_dir():
        return []
    with _lock:
        cache = _ler_cache(raiz)
        novo: dict[str, tuple[tuple[int, int], list[dict]]] = {}
        out: list[UsoSessao] = []
        mudou = False
        for p in raiz.rglob("*.jsonl"):
            try:
                st = p.stat()
            except OSError:
                continue
            chave = str(p)
            sig = (st.st_mtime_ns, st.st_size)
            hit = cache.get(chave)
            usos = []
            if hit is not None and hit[0] == sig:
                usos = [_desserializar(item) for item in hit[1]] if isinstance(hit[1], list) else [None]
                # entrada gravada que não desserializa é MISS: mantê-la faria a sessão sumir
                # da conta e nunca ser relida, porque o sig continua batendo.
                if any(uso is None for uso in usos):
                    hit = None
            if hit is None or hit[0] != sig:
                mudou = True
                usos = ler_transcript(p)
                novo[chave] = (sig, [_serializar(uso) for uso in usos])
            else:
                novo[chave] = hit
            for uso in usos:
                # Identidade pelo CAMINHO relativo: o `sessionId` do subagente é o do PAI
                # (medido: 168 de 446 ids repetidos entre arquivos).
                out.append(replace(uso, session_id=str(p.relative_to(raiz).with_suffix(""))))
        if len(novo) != len(cache):
            mudou = True
        if mudou:
            # Cache é otimização: falha aqui vira log, nunca 500 no /api/costs.
            try:
                _gravar_cache(raiz, novo)
            except OSError as e:
                _log.warning("cache de transcript não pôde ser gravado: %r", e)
        _mem[str(raiz)] = novo
    return out
