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
import threading
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import costs_claude_transcript, pricing
from app.adapters.pi import sessions as pi_sessions
from app.config import list_config_dirs

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
    subagente: bool = False   # transcript de subagente (Task tool), não de conversa


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


def _int(v) -> int:
    try:
        return int(v or 0)
    except (TypeError, ValueError):
        return 0


def linhas_claude(config_dir: Path, account_id: str) -> list[UsageRow]:
    """Uso do Claude Code lido do TRANSCRIPT (`<config>/projects/**/*.jsonl`).

    Era o `costs.jsonl` do plugin ECC. Trocou por três motivos medidos: o resumo não enxerga
    subagente (medido em 01/08/2026: 15,5% do cache lido — cresce toda semana, é foto, não
    constante), o app não pode depender de plugin de terceiro para função própria, e o plugin
    só cobre a partir de 27/06 enquanto os transcripts vão a 12/06.

    A raiz vem do `config_dir` RECEBIDO: `coletar()` chama esta função uma vez por diretório
    de configuração, e ignorar o argumento leria a mesma raiz N vezes.
    """
    raiz = costs_claude_transcript.raiz_projetos(config_dir)
    out: list[UsageRow] = []
    for u in costs_claude_transcript.varrer(raiz):
        modelo = (u.model or "").strip()
        if modelo in pricing.IGNORADOS:
            continue
        prov = pricing.canonizar_provedor(pricing.provider_for(modelo) or "")
        # Modelo da própria Anthropic (ou sem tarifa) -> a conta é o provedor.
        # Outro provedor -> é sessão de motor, e o modelo é quem entrega, porque o
        # CP_ENGINE só existe em processo vivo.
        if not prov or prov == "anthropic":
            prov = account_id
        out.append(UsageRow(
            ts=u.ts, source="claude", provider=prov, model=modelo,
            project=u.cwd or PROJETO_DESCONHECIDO, session_id=u.session_id,
            input=u.input, output=u.output,
            cache_write=u.cache_write, cache_read=u.cache_read,
            subagente=u.subagente,
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
                ts=ts, source="codex",
                provider=pricing.canonizar_provedor(prov) or "openai", model=modelo or "?",
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
            ts=ts, source="pi", provider=pricing.canonizar_provedor(prov) or "?",
            model=modelo or "?",
            project=cwd or PROJETO_DESCONHECIDO, session_id=sid,
            input=acc["input"], output=acc["output"],
            cache_write=acc["cacheWrite"], cache_read=acc["cacheRead"],
        ))
    return out


# Cache por ARQUIVO, chaveado por (mtime_ns, st_size) — mesmo padrão do planprog.py.
# Guarda só Codex e Pi (o Claude saiu daqui, foi pro cache em disco de costs_claude_transcript.py).
# Rollout e sessão de Pi fechados nunca mudam e ficam aqui pra sempre.
_cache: dict[str, tuple[tuple[int, int], list[UsageRow]]] = {}
# O endpoint é `def` e roda no threadpool: celular + desktop + peer batendo juntos com cache frio
# fariam N parses simultâneos do mesmo arquivo. Precedente: engines.py:58.
# ponytail: a trava cobre o CORPO INTEIRO de coletar() (walk de Codex/Pi + parse das três
# fontes), não só o parse duplicado -- serializa qualquer coleta concorrente, mesmo com cache
# quente. Aceitável num app single-user de LAN; trava por fonte se um dia isso virar gargalo medido.
_cache_lock = threading.Lock()


def invalidar_cache() -> None:
    with _cache_lock:
        _cache.clear()


def _assinatura(p: Path) -> tuple[int, int] | None:
    try:
        st = p.stat()
    except OSError:
        return None
    return (st.st_mtime_ns, st.st_size)


def account_info(config_dir: Path, fallback_label: str) -> tuple[str, str | None, str]:
    """(uuid, email, label) da conta Anthropic. Era costs._account_info e MUDOU DE MÓDULO:
    ler o config dir é trabalho de leitor de fonte, não de agregador — e deixá-la no costs.py
    criaria ciclo (costs importa costs_sources; _config_dirs precisaria de costs)."""
    for f in (config_dir / ".claude.json", Path.home() / ".claude.json"):
        try:
            oa = (json.loads(f.read_text()).get("oauthAccount") or {})
        except (OSError, json.JSONDecodeError, AttributeError, TypeError):
            # .claude.json existe mas a raiz não é dict (corrompido) -> tenta o próximo
            continue
        uuid = oa.get("accountUuid")
        if uuid:
            email = oa.get("emailAddress")
            return uuid, email, (email or fallback_label)
    return fallback_label, None, fallback_label


# Chave de provedor -> rótulo legível. Preenchido por _config_dirs() e lido pelo costs.py na hora
# de montar o by_provider. Existe porque a CHAVE tem que continuar sendo o uuid (é o que não
# colide e o que a malha soma entre servidores), mas o uuid como texto na tela é ilegível: a linha
# de topo do painel "Por provedor", com 87% do gasto, aparecia como
# 'anthropic:758a9521-e2ef-435b-8738-bc502547c24c'. Antes da reescrita a tela mostrava o e-mail.
_ROTULOS: dict[str, str] = {}


def rotulo_de_provedor(chave: str) -> str | None:
    """Rótulo legível de uma chave de provedor, ou None (o front cai pra própria chave)."""
    return _ROTULOS.get(chave)


def _config_dirs() -> list[tuple[str, str]]:
    """(caminho, account_id) de cada config dir do Claude. O prefixo 'anthropic:' evita colisão
    com nome de provedor ('openai', 'kimi-coding', …), que vivem no mesmo espaço de chaves."""
    out = []
    for cfg in list_config_dirs():
        uuid, email, label = account_info(Path(cfg.path), cfg.label)
        chave = f"anthropic:{uuid}"
        # O e-mail já foi lido aqui; jogá-lo fora era o que obrigava a tela a exibir o uuid cru.
        _ROTULOS[chave] = email or label
        out.append((cfg.path, chave))
    return out


def coletar() -> list[UsageRow]:
    """Todas as linhas das três fontes. NUNCA vai à rede."""
    out: list[UsageRow] = []
    with _cache_lock:
        # O Claude não tem entrada própria aqui (era `_assinatura` do costs.jsonl): quem
        # cacheia agora é o costs_claude_transcript, por RAIZ e por ARQUIVO (Task 1) — uma
        # segunda camada de cache aqui só duplicaria a invalidação sem ganhar nada.
        for caminho, account_id in _config_dirs():
            out.extend(linhas_claude(Path(caminho), account_id))

        for nome, raiz, leitor in (("codex", raiz_codex(), linhas_codex),
                                   ("pi", raiz_pi(), linhas_pi)):
            if not raiz.is_dir():
                _cache.pop(nome, None)
                continue
            # O caro do Codex e do Pi é o walk, e é preciso andar pra descobrir os mtimes — então
            # a chave é o conjunto de (arquivo, mtime, tamanho), calculado no próprio walk.
            padrao = "rollout-*.jsonl" if nome == "codex" else "*.jsonl"
            sig_dir = tuple(sorted(
                (str(p), *(_assinatura(p) or (0, 0))) for p in raiz.rglob(padrao)))
            hit = _cache.get(nome)
            if hit is None or hit[0] != sig_dir:
                hit = (sig_dir, leitor())
                _cache[nome] = hit
            out.extend(hit[1])
    return out
