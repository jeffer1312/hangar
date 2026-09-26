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

CACHE EM DISCO, e POR RAIZ (`costs_cache`): a varredura fria mede 13,6s sobre 3.202 arquivos e
5,2 GB, contra o `AbortSignal.timeout(4000)` do cliente. Cache só em memória pagaria isso a cada
restart; um cache global de raiz única seria apagado pela segunda conta configurada.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import costs_cache, pricing, uso_areas, uso_claude
from app.uso_claude import UsoLinha

# `LOCAL` é cópia proposital: `costs_sources` vai importar ESTE módulo, então importar de lá
# fecharia ciclo. Não "arrume" unificando — quebra o import.
LOCAL = timezone(timedelta(hours=-3))

# Suba isto ao mudar o formato do resumo, senão o cache velho é servido pra sempre.
CACHE_VERSAO = 11

# Marcador do subagente. O caminho é `<projeto>/<sessionId>/subagents/agent-*.jsonl`.
# Medido em 01/08/2026: 2.714 arquivos assim, contra 446 de conversa — cresce toda semana.
_DIR_SUBAGENTE = "subagents"


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
    fast: bool = False     # `usage.speed == "fast"`: a Anthropic cobra o dobro nesse modo.
    # Cache escrito em respostas que PERDERAM o cache (expirou na pausa ou o contexto mudou):
    # o que foi regravado e poderia ter sido relido.
    regravado: int = 0
    regravado_1h: int = 0


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


@dataclass(frozen=True)
class Leitura:
    """As duas leituras de um transcript, feitas numa passada só: tokens (custo) e uso
    (tools/skills/contexto). Uma entrada de cache por arquivo guarda as duas."""
    usos: list[UsoSessao]
    uso: list[UsoLinha]


def ler_transcript(path: Path) -> list[UsoSessao]:
    return ler_completo(path).usos


def _quando(ts) -> datetime | None:
    if not isinstance(ts, str):
        return None
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).astimezone(LOCAL)
    except ValueError:
        return None


def ler_completo(path: Path) -> Leitura:
    """Uso por resposta, separado por dia/modelo; blocos da mesma resposta não somam novamente.
    Na mesma passada, o acumulador de uso vê toda linha de assistant/user/attachment."""
    respostas: dict[tuple, UsoSessao] = {}
    # Respostas depois das quais a próxima gravação grande é esperada, não perda de cache: a
    # primeira do arquivo e a primeira depois de compactar.
    depois_de_compactar: set[tuple] = set()
    compactou = False
    acumulador = uso_claude.Acumulador()
    try:
        f = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return Leitura([], [])
    with f:
        for numero, linha in enumerate(f):
            # Pré-filtro barato: a maior parte das linhas (progresso, fila, títulos) não tem
            # nem uso nem tool nem attachment; sem isto o json.loads roda em tudo.
            if ('"usage"' not in linha and '"user"' not in linha and '"attachment"' not in linha
                    and '"compact_boundary"' not in linha):
                continue
            try:
                d = json.loads(linha)
            except json.JSONDecodeError:
                continue
            if not isinstance(d, dict):
                continue
            quando = _quando(d.get("timestamp"))
            msg = d.get("message")
            m = msg.get("model") if isinstance(msg, dict) else None
            if isinstance(m, str) and m.strip() in pricing.IGNORADOS:
                continue
            acumulador.linha(d, quando.strftime("%Y-%m-%d") if quando else "")
            if d.get("subtype") == "compact_boundary":
                compactou = True
            if d.get("type") != "assistant":
                continue
            u = msg.get("usage") if isinstance(msg, dict) else None
            if not isinstance(u, dict) or quando is None:
                continue
            # Sem identidade não há prova de repetição: preserva as linhas antigas.
            key = (d.get("requestId"), msg["id"]) if msg.get("id") else (numero,)
            if compactou and key not in respostas:
                depois_de_compactar.add(key)
                compactou = False
            criacao = u.get("cache_creation")
            cache_1h = _int(criacao.get("ephemeral_1h_input_tokens")) if isinstance(criacao, dict) else 0
            respostas[key] = UsoSessao(
                session_id="", ts=quando, model=m or "?", cwd=d.get("cwd") or "",
                subagente=(_DIR_SUBAGENTE in path.parts),
                input=_int(u.get("input_tokens")), output=_int(u.get("output_tokens")),
                cache_write=_int(u.get("cache_creation_input_tokens")),
                cache_read=_int(u.get("cache_read_input_tokens")),
                cache_write_1h=min(max(0, cache_1h), max(0, _int(u.get("cache_creation_input_tokens")))),
                fast=u.get("speed") == "fast")
    grupos: dict[tuple, UsoSessao] = {}
    contexto_antes = None
    for chave, uso in respostas.items():
        # Perdido = o contexto da resposta anterior que NÃO veio do cache e teve de ser gravado de
        # novo. Comparar com o cache lido da própria resposta confundia conteúdo novo grande
        # (arquivo lido, diff) com cache expirado.
        if contexto_antes is not None and chave not in depois_de_compactar and uso.cache_write:
            perdido = min(uso.cache_write, max(0, contexto_antes - uso.cache_read))
            # Expirar leva o prefixo quase inteiro; sobra pequena é lembrete que mudou no meio.
            if perdido * 2 >= contexto_antes:
                uso = replace(uso, regravado=perdido,
                              regravado_1h=uso.cache_write_1h * perdido // uso.cache_write)
        contexto_antes = uso.input + uso.cache_write + uso.cache_read
        # `fast` entra na chave porque é o que decide a TARIFA: somado com o padrão, o grupo
        # inteiro seria cobrado por uma das duas e a outra metade sairia errada.
        key = (uso.ts.date(), uso.model, uso.cwd, uso.fast)
        antes = grupos.get(key)
        grupos[key] = uso if antes is None else replace(
            antes, input=antes.input + uso.input, output=antes.output + uso.output,
            cache_write=antes.cache_write + uso.cache_write,
            cache_read=antes.cache_read + uso.cache_read,
            cache_write_1h=antes.cache_write_1h + uso.cache_write_1h,
            regravado=antes.regravado + uso.regravado,
            regravado_1h=antes.regravado_1h + uso.regravado_1h)
    return Leitura(sorted(grupos.values(), key=lambda u: (u.ts, u.model, u.cwd)),
                   acumulador.resultado())


def invalidar_cache() -> None:
    costs_cache.invalidar()


def _serializar_leitura(le: Leitura) -> dict:
    return {"usos": [_serializar(u) for u in le.usos], "uso": [l.para_dict() for l in le.uso]}


def _desserializar_leitura(d: dict) -> Leitura | None:
    try:
        usos = [_desserializar(x) for x in d["usos"]]
        uso = [UsoLinha.de_dict(x) for x in d["uso"]]
    except (KeyError, TypeError):
        return None
    if any(x is None for x in usos) or any(x is None for x in uso):
        return None
    return Leitura(usos, uso)


def _serializar(u: UsoSessao) -> dict:
    return {"ts": u.ts.isoformat(), "model": u.model, "cwd": u.cwd,
            "subagente": u.subagente, "input": u.input, "output": u.output,
            "cache_write": u.cache_write, "cache_read": u.cache_read,
            "cache_write_1h": u.cache_write_1h, "fast": u.fast,
            "regravado": u.regravado, "regravado_1h": u.regravado_1h}


def _desserializar(d: dict) -> UsoSessao | None:
    try:
        return UsoSessao(session_id="", ts=datetime.fromisoformat(d["ts"]),
                         model=d["model"], cwd=d.get("cwd", ""),
                         subagente=bool(d.get("subagente")),
                         input=int(d["input"]), output=int(d["output"]),
                         cache_write=int(d["cache_write"]), cache_read=int(d["cache_read"]),
                         cache_write_1h=int(d.get("cache_write_1h", 0)),
                         fast=bool(d.get("fast")),
                         regravado=int(d.get("regravado", 0)),
                         regravado_1h=int(d.get("regravado_1h", 0)))
    except (KeyError, TypeError, ValueError):
        return None


def _varrer(raiz: Path) -> list[tuple[str, Leitura]]:
    if not raiz.is_dir():
        return []
    # `ler_completo` resolvido na chamada, não capturado: o teste prova o cache trocando o
    # nome no módulo e contando chamadas.
    pares = costs_cache.varrer_cacheado(
        "transcripts", raiz, raiz.rglob("*.jsonl"), lambda p: [ler_completo(p)],
        # O mapa de áreas entra na versão: a área é gravada no cache junto com a leitura.
        _serializar_leitura, _desserializar_leitura,
        f"{CACHE_VERSAO}:{uso_areas.assinatura()}")
    # Identidade pelo CAMINHO relativo: o `sessionId` do subagente é o do PAI
    # (medido: 168 de 446 ids repetidos entre arquivos).
    return [(str(p.relative_to(raiz).with_suffix("")), le) for p, leituras in pares
            for le in leituras]


def varrer(raiz: Path) -> list[UsoSessao]:
    """Uso de tokens de todas as sessões de UMA raiz de projetos. Nunca vai à rede."""
    return [replace(u, session_id=sid) for sid, le in _varrer(raiz) for u in le.usos]


def varrer_uso(raiz: Path) -> list[UsoLinha]:
    """Uso de tools/skills/contexto da mesma raiz — mesma passada, mesmo cache."""
    return [replace(l, session_id=sid, subagente=_DIR_SUBAGENTE in Path(sid).parts)
            for sid, le in _varrer(raiz) for l in le.uso]
