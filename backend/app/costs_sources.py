"""Os três formatos de uso (Claude Code, Codex, Pi) normalizados num UsageRow só.

A armadilha central: cada fonte ACUMULA de um jeito diferente. Usar a regra errada não quebra
nada — devolve um número plausível e errado.

  Claude  cumulativo por sessão -> ÚLTIMA linha por session_id
  Codex   cumulativo            -> ÚLTIMO evento token_count
  Pi      por mensagem          -> SOMA de todos os usage
"""
from __future__ import annotations

import json
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import pricing

LOCAL = timezone(timedelta(hours=-3))
PROJETO_DESCONHECIDO = "desconhecido"


@dataclass(frozen=True)
class UsageRow:
    ts: datetime
    source: str        # "claude" | "codex" | "pi"
    provider: str      # onde a fatura cai: "anthropic:<uuid>" | "openai" | "kimi-coding" | ...
    model: str         # id CRU do log; quem canoniza é o pricing
    project: str       # caminho absoluto REAL, ou PROJETO_DESCONHECIDO
    session_id: str
    input: int
    output: int
    cache_write: int
    cache_read: int


def _ler_jsonl(path: Path) -> Iterator[dict]:
    """Linha inválida é o caso NORMAL: o Codex escreve o rollout enquanto lemos, então a última
    linha truncada é rotina. E exigir dict é obrigatório — `null` e lista são JSON válido, não
    levantam ValueError, e um .get() em cima disso já derrubou o app inteiro (ver statusline)."""
    try:
        # encoding explícito: sem isto o Python usa o locale, e o errors="replace" MASCARA o
        # estrago (acento/espaço no cwd viram outro caminho, calado). Mesmo padrão de
        # transcript.py.
        f = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return
    with f:
        for linha in f:
            linha = linha.strip()
            if not linha:
                continue
            try:
                d = json.loads(linha)
            except json.JSONDecodeError:
                continue
            if isinstance(d, dict):
                yield d


def _quando(iso: str | None) -> datetime | None:
    if not iso:
        return None
    try:
        return datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(LOCAL)
    except (ValueError, TypeError):
        return None


def cwd_do_transcript(path: str) -> str:
    """O caminho REAL do projeto, lido de dentro do transcript.

    O nome do diretório do Claude (`-home-jefferson--rea-de-trabalho-…`) NÃO é invertível: 'Á' e
    espaço viraram '-', igual à barra. Desmanglar é adivinhação. O transcript carrega o cwd de
    verdade — medido: terceira linha, junto com gitBranch.
    """
    if not path:
        return ""
    for i, d in enumerate(_ler_jsonl(Path(path))):
        cwd = d.get("cwd")
        if isinstance(cwd, str) and cwd:
            return cwd
        if i > 200:   # cwd aparece nas primeiras linhas; sem o corte, um transcript de 100 MB
            break     # seria lido inteiro só pra descobrir que não tem
    return ""


def _int(v) -> int:
    try:
        return int(v or 0)
    except (TypeError, ValueError):
        return 0


def linhas_claude(config_dir: Path, account_id: str) -> list[UsageRow]:
    """~/.claude/metrics/costs.jsonl — cumulativo por sessão, última linha vence.

    O provedor é a conta Anthropic, EXCETO em sessão de motor: ali o modelo entrega
    ('k3' só existe na Moonshot), e é o único caminho possível, porque CP_ENGINE só existe
    em processo vivo.
    """
    src = config_dir / "metrics" / "costs.jsonl"
    ultimo: dict[str, dict] = {}
    for d in _ler_jsonl(src):
        if not d.get("timestamp"):
            continue
        chave = d.get("session_id") or d.get("transcript_path") or d["timestamp"]
        ultimo[chave] = d

    cache_cwd: dict[str, str] = {}
    out: list[UsageRow] = []
    for d in ultimo.values():
        modelo = (d.get("model") or "").strip()
        if modelo in pricing.IGNORADOS:
            continue
        ts = _quando(d.get("timestamp"))
        if ts is None:
            continue
        tp = d.get("transcript_path") or ""
        if tp not in cache_cwd:
            cache_cwd[tp] = cwd_do_transcript(tp)
        prov = pricing.provider_for(modelo)
        # Modelo da própria Anthropic -> a conta é o provedor. Outro provedor -> é motor.
        if prov is None or prov == "anthropic":
            prov = account_id
        out.append(UsageRow(
            ts=ts, source="claude", provider=prov, model=modelo,
            project=cache_cwd[tp] or PROJETO_DESCONHECIDO,
            session_id=str(d.get("session_id") or ""),
            input=_int(d.get("input_tokens")), output=_int(d.get("output_tokens")),
            cache_write=_int(d.get("cache_write_tokens")),
            cache_read=_int(d.get("cache_read_tokens")),
        ))
    return out
