"""Os três formatos de uso (Claude Code, Codex, Pi) normalizados num UsageRow só.

A armadilha central: cada fonte ACUMULA de um jeito diferente. Usar a regra errada não quebra
nada — devolve um número plausível e errado.

  Claude  cumulativo por sessão -> ÚLTIMA linha por session_id
  Codex   cumulativo            -> ÚLTIMO evento token_count
  Pi      por mensagem          -> SOMA de todos os usage
"""
from __future__ import annotations

import json
import os
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import pricing
from app.adapters.pi import sessions as pi_sessions

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


def raiz_codex() -> Path:
    return Path(os.environ.get("CODEX_HOME") or (Path.home() / ".codex")) / "sessions"


def linhas_codex() -> list[UsageRow]:
    """~/.codex/sessions/**/rollout-*.jsonl — cumulativo, último token_count vence.

    Dois detalhes medidos em 30/07/2026 que só se descobre olhando o arquivo:
      - `input_tokens` INCLUI o cacheado -> input real = input_tokens - cached_input_tokens
      - `reasoning_output_tokens` é SUBCONJUNTO de `output_tokens` (16242+18=16260) -> não somar
    O adapter já registra que este campo é cumulativo (adapters/codex/adapter.py:144); lá isso é
    problema, aqui é exatamente o que queremos.

    Thread encerrada é MOVIDA (não copiada) para `archived_sessions/`, diretório IRMÃO de
    `sessions/` — sem varrer os dois, todo gasto de thread arquivada some do painel. Como é
    move e não copy, os `session_id` dos dois diretórios não se sobrepõem: ler os dois não
    duplica nada.
    """
    viva = raiz_codex()
    arquivada = viva.parent / "archived_sessions"
    out: list[UsageRow] = []
    for raiz in (viva, arquivada):
        if not raiz.is_dir():
            continue
        for arq in raiz.rglob("rollout-*.jsonl"):
            cwd = prov = modelo = sid = ""
            ts = None
            ultimo: dict | None = None
            for d in _ler_jsonl(arq):
                p = d.get("payload")
                if not isinstance(p, dict):
                    continue
                if d.get("type") == "session_meta":
                    cwd = p.get("cwd") or cwd
                    prov = p.get("model_provider") or prov
                    sid = p.get("session_id") or sid
                    ts = _quando(d.get("timestamp")) or ts
                # `model` só é confiável vindo de `turn_context` — não amarrar ao tipo faria
                # o valor vazar de qualquer evento futuro que ganhe um campo `model` incidental.
                if d.get("type") == "turn_context" and isinstance(p.get("model"), str):
                    modelo = p["model"]
                if p.get("type") == "token_count":
                    info = p.get("info")
                    if isinstance(info, dict) and isinstance(info.get("total_token_usage"), dict):
                        ultimo = info["total_token_usage"]
            if ultimo is None or ts is None:
                continue
            cr = _int(ultimo.get("cached_input_tokens"))
            out.append(UsageRow(
                ts=ts, source="codex", provider=prov or "openai", model=modelo or "?",
                project=cwd or PROJETO_DESCONHECIDO, session_id=sid,
                input=max(0, _int(ultimo.get("input_tokens")) - cr),
                output=_int(ultimo.get("output_tokens")),
                cache_write=0, cache_read=cr,
            ))
    return out


def raiz_pi() -> Path:
    return pi_sessions.sessions_root()


def linhas_pi() -> list[UsageRow]:
    """~/.pi/agent/sessions/**/*.jsonl — POR MENSAGEM, soma tudo.

    Glob RECURSIVA de propósito: o subagente do Pi mora em
    `<sessao>/<taskId>/run-N/session.jsonl` (é o que adapters/pi/sessions.py:41-47 já documenta,
    acima de `is_subagent_transcript`).
    Medido em 01/08/2026 num par pai/filho: na janela de 20:52:12–20:58:10 em que o filho
    registrou 19 eventos de uso, o pai registrou ZERO, e nenhum totalTokens coincide. O uso do
    subagente NÃO está no pai — somar os dois não duplica, e ignorar o filho perde 18 sessões.

    O `usage.cost` que o Pi já calcula é DESCARTADO: o custo é recalculado com a mesma tabela das
    outras fontes, senão as três não estão na mesma régua.
    """
    raiz = raiz_pi()
    if not raiz.is_dir():
        return []
    out: list[UsageRow] = []
    for arq in raiz.rglob("*.jsonl"):
        cwd = modelo = prov = ""
        ts = None
        acc = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
        viu = False
        for d in _ler_jsonl(arq):
            t = d.get("type")
            if t == "session":
                cwd = d.get("cwd") or cwd
                ts = _quando(d.get("timestamp")) or ts
            elif t == "model_change":
                prov = d.get("provider") or prov
                modelo = d.get("modelId") or modelo
            elif t == "message":
                msg = d.get("message")
                u = msg.get("usage") if isinstance(msg, dict) else None
                if isinstance(u, dict):
                    viu = True
                    for k in acc:
                        acc[k] += _int(u.get(k))
        if not viu or ts is None:
            continue
        # session_id pelo caminho RELATIVO, não pelo `arq.stem`: todo subagente se chama
        # `session.jsonl`, então o stem seria a string "session" para TODOS eles, de todas as
        # sessões — indistinguíveis. Hoje não corrompe soma (não há dedup entre linhas do Pi),
        # mas deixaria o campo inútil pra qualquer drill-down.
        sid = str(arq.relative_to(raiz).with_suffix(""))
        out.append(UsageRow(
            ts=ts, source="pi", provider=prov or "?", model=modelo or "?",
            project=cwd or PROJETO_DESCONHECIDO, session_id=sid,
            input=acc["input"], output=acc["output"],
            cache_write=acc["cacheWrite"], cache_read=acc["cacheRead"],
        ))
    return out
