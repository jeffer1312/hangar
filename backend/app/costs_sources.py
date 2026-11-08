"""Os quatro formatos de uso (Claude Code, Codex, Pi, Kimi) normalizados num UsageRow só.

A armadilha central: cada fonte ACUMULA de um jeito diferente. Usar a regra errada não quebra
nada — devolve um número plausível e errado.

  Claude  por resposta          -> dedup por identidade, separado por dia/modelo
  Codex   por resposta          -> token_usage_record; deltas de token_count no legado
  Pi      por mensagem          -> SOMA de todos os usage
  Kimi    por evento/turno      -> SOMA dos usage.record
"""
from __future__ import annotations

import itertools
import json
import logging
import threading
import time
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app import codex_contas, costs_cache, costs_claude_transcript, pricing, uso_codex
from app.adapters.kimi import sessions as kimi_sessions
from app.adapters.pi import sessions as pi_sessions
from app.config import list_config_dirs

LOCAL = timezone(timedelta(hours=-3))
PROJETO_DESCONHECIDO = "desconhecido"
# Suba ao mudar o que `uso_codex` grava por rollout.
_USO_CODEX_VERSAO = 3
_log = logging.getLogger("hangar.costs")
# Raízes já avisadas: `coletar()` roda a cada abertura da tela de custos, e o aviso é um só.
_AVISOU_RAIZ_UNICA: set[str] = set()


@dataclass(frozen=True, slots=True)
class UsageRow:
    ts: datetime
    source: str        # "claude" | "codex" | "pi" | "omp" | "kimi"
    provider: str      # onde a fatura cai: "anthropic:<uuid>" | "openai" | "kimi-coding" | ...
    model: str         # id CRU do log; quem canoniza é o pricing
    project: str       # caminho absoluto REAL, ou PROJETO_DESCONHECIDO
    session_id: str
    input: int
    output: int
    cache_write: int
    cache_read: int
    subagente: bool = False   # transcript de subagente (Task tool), não de conversa
    account_id: str | None = None
    codex_long_context: bool = False
    cache_write_1h: int = 0
    fast: bool = False        # modo rápido do Claude: a mesma resposta custa o dobro
    # Parte do cache_write gravada de novo porque o cache tinha expirado (só o Claude sabe).
    regravado: int = 0
    regravado_1h: int = 0


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


def _dict_da_linha(bruta: bytes) -> dict | None:
    """Mesma regra do `_ler_jsonl`, para uma linha crua lida pelo índice."""
    bruta = bruta.strip()
    if not bruta:
        return None
    try:
        d = json.loads(bruta.decode("utf-8", "replace"))
    except json.JSONDecodeError:
        return None
    return d if isinstance(d, dict) else None


def _usage_row(t: tuple) -> UsageRow:
    """Linha do índice (`costs_cache.CAMPOS_CUSTO`) -> UsageRow."""
    return UsageRow(datetime.fromisoformat(t[0]), t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8],
                    t[9], bool(t[10]), t[11], bool(t[12]), t[13], bool(t[14]), t[15], t[16])


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
    costs_claude_transcript.sincronizar(raiz)
    return _linhas_claude_do_indice(raiz, account_id)


def _linhas_claude_do_indice(raiz: Path, account_id: str, desde: str | None = None,
                             carimbar_conta: bool = False) -> list[UsageRow]:
    out: list[UsageRow] = []
    provedores: dict[str, str] = {}
    for t in costs_cache.ler_custos(costs_claude_transcript.escopo(raiz), desde):
        modelo = (t[3] or "").strip()
        if modelo in pricing.IGNORADOS:
            continue
        prov = provedores.get(modelo)
        if prov is None:
            prov = pricing.canonizar_provedor(pricing.provider_for(modelo) or "")
            # Modelo da própria Anthropic (ou sem tarifa) -> a conta é o provedor.
            # Outro provedor -> é sessão de motor, e o modelo é quem entrega, porque o
            # CP_ENGINE só existe em processo vivo.
            if not prov or prov == "anthropic":
                prov = account_id
            provedores[modelo] = prov
        out.append(UsageRow(
            ts=datetime.fromisoformat(t[0]), source="claude", provider=prov, model=modelo,
            project=t[4] or PROJETO_DESCONHECIDO, session_id=t[5],
            input=t[6], output=t[7], cache_write=t[8], cache_read=t[9],
            cache_write_1h=t[13], regravado=t[15], regravado_1h=t[16],
            subagente=bool(t[10]), fast=bool(t[14]),
            account_id=account_id if carimbar_conta else None,
        ))
    return out


def raiz_codex(home: Path | str | None = None) -> Path:
    base = codex_contas.default_home() if home is None else Path(home)
    return base.expanduser().absolute() / "sessions"


def linhas_codex(home: Path | str | None = None, account_id: str | None = None,
                 *, only_paths: set[Path] | None = None) -> list[UsageRow]:
    """Uso por resposta; rollouts antigos usam deltas dos contadores cumulativos."""
    base = (codex_contas.default_home() if home is None else Path(home)).expanduser().absolute()
    viva = raiz_codex() if home is None else raiz_codex(base)
    arquivada = viva.parent / "archived_sessions"
    out: list[UsageRow] = []
    vistos: set[Path] = set()
    for raiz in (viva, arquivada):
        if not raiz.is_dir():
            continue
        for arq in raiz.rglob("rollout-*.jsonl"):
            try:
                canonical = arq.resolve(strict=True)
            except OSError:
                continue
            if canonical in vistos:
                continue
            if only_paths is not None and canonical not in only_paths:
                continue
            vistos.add(canonical)
            out.extend(_linhas_rollout_codex(
                arq, account_id or f"codex:{base.resolve(strict=False)}"))
    return out


class RespostasCodex:
    """Uso de cada resposta do rollout, por turn_id, já sem o que veio herdado de um fork.
    Lê registro a registro e é retomável (vai no pickle da `DobraCodex`)."""

    def __init__(self, sid: str, account_id: str | None) -> None:
        self.account_id = account_id
        self.cwd = self.prov = self.modelo = self.turno = ""
        self.sid = sid
        self.inicio = None
        self.subagente = False
        self.meta_lida = False
        self.herdado = False
        self.anterior = {k: 0 for k in ("input_tokens", "cached_input_tokens",
                                        "cache_write_input_tokens", "output_tokens")}
        self.respostas: dict[str, list[UsageRow]] = {}
        self.legado: dict[str, list[UsageRow]] = {}
        self.vistos: set[str] = set()

    def registro(self, d: dict) -> None:
        p = d.get("payload")
        if not isinstance(p, dict):
            return
        tipo = d.get("type")
        if tipo == "session_meta" and self.meta_lida:
            identidade = p.get("id") or p.get("session_id")
            if identidade:
                self.herdado = identidade != self.sid
        if tipo == "session_meta" and not self.meta_lida:
            # Forks também contêm a metadata do pai; só a primeira identifica este arquivo.
            self.meta_lida = True
            self.sid = p.get("id") or p.get("session_id") or self.sid
            self.cwd = p.get("cwd") or ""
            self.prov = p.get("model_provider") or ""
            self.inicio = _quando(d.get("timestamp"))
            source = p.get("source")
            self.subagente = (isinstance(source, dict) and "subagent" in source) or source == "subagent"
        if tipo == "turn_context":
            contexto_ts = _quando(d.get("timestamp"))
            if self.herdado and self.inicio and contexto_ts and contexto_ts >= self.inicio:
                self.herdado = False
            self.turno = p.get("turn_id") or self.turno
            if isinstance(p.get("model"), str):
                self.modelo = p["model"]
        ts = _quando(d.get("timestamp")) or self.inicio
        if ts is None:
            return
        if tipo == "token_usage_record":
            if p.get("thread_id") and p["thread_id"] != self.sid:
                self.respostas.setdefault(p.get("turn_id") or self.turno, [])
                return
            if p.get("thread_id") == self.sid:
                self.herdado = False
            u = p.get("usage")
            if not isinstance(u, dict):
                return
            response_id = p.get("response_id")
            if response_id and response_id in self.vistos:
                return
            if response_id:
                self.vistos.add(response_id)
            destino = self.respostas.setdefault(p.get("turn_id") or self.turno, [])
        elif tipo == "event_msg" and p.get("type") == "token_count":
            info = p.get("info")
            total = info.get("total_token_usage") if isinstance(info, dict) else None
            if not isinstance(total, dict):
                return
            anterior = self.anterior
            atual = {k: max(0, _int(total.get(k))) for k in anterior}
            if atual == anterior:
                return
            # Retomar pode reiniciar os contadores: a primeira resposta do trecho é uso novo.
            reiniciou = any(atual[k] < anterior[k] for k in anterior)
            u = {k: atual[k] - (0 if reiniciou else anterior[k]) for k in anterior}
            self.anterior = atual
            if self.herdado:
                return
            destino = self.legado.setdefault(self.turno, [])
        else:
            return
        entrada = max(0, _int(u.get("input_tokens")))
        cache = min(entrada, max(0, _int(u.get("cached_input_tokens"))))
        escrita = min(entrada - cache, max(0, _int(u.get("cache_write_input_tokens"))))
        destino.append(UsageRow(
            ts=ts, source="codex", provider=pricing.canonizar_provedor(self.prov) or "openai",
            model=self.modelo or "?", project=self.cwd or PROJETO_DESCONHECIDO, session_id=self.sid,
            input=entrada - cache - escrita, output=max(0, _int(u.get("output_tokens"))),
            cache_write=escrita, cache_read=cache, subagente=self.subagente,
            account_id=self.account_id, codex_long_context=entrada > 272_000,
        ))

    def por_turno(self) -> dict[str, list[UsageRow]]:
        """Não muda o estado: pode ser chamada a cada retomada."""
        respostas, legado = self.respostas, self.legado
        campos = ("input", "cache_read", "cache_write", "output")

        def assinatura(r: UsageRow) -> tuple:
            return (r.model, *(getattr(r, campo) for campo in campos))

        por_turno: dict[str, list[UsageRow]] = {}
        # Legado seguido de moderno conserva a inserção de cada dicionário.
        for key in dict.fromkeys((*legado, *respostas)):
            modernos = respostas.get(key, [])
            linhas = por_turno.setdefault(key, [])
            linhas.extend(modernos)
            if key in respostas and not modernos:
                continue
            # Registros por resposta podem chegar depois do contador; só retiramos o uso coberto.
            cobertos: dict[tuple, int] = {}
            for r in modernos:
                sig = assinatura(r)
                cobertos[sig] = cobertos.get(sig, 0) + 1
            pendentes = []
            for r in legado.get(key, []):
                sig = assinatura(r)
                if cobertos.get(sig, 0):
                    cobertos[sig] -= 1
                else:
                    pendentes.append(r)
            saldo = {campo: sum(sig[i + 1] * n for sig, n in cobertos.items())
                     for i, campo in enumerate(campos)}
            for r in pendentes:
                valores = {}
                for campo in campos:
                    abatido = min(getattr(r, campo), saldo[campo])
                    saldo[campo] -= abatido
                    valores[campo] = getattr(r, campo) - abatido
                if any(valores.values()):
                    linhas.append(replace(r, **valores))
        return por_turno


def respostas_por_turno_codex(arq: Path, account_id: str) -> dict[str, list[UsageRow]]:
    """Uso de cada resposta do rollout, por turn_id, já sem o que veio herdado de um fork."""
    leitor = RespostasCodex(arq.stem, account_id)
    for d in _ler_jsonl(arq):
        leitor.registro(d)
    return leitor.por_turno()


def _agrupar_rollout(por_turno: dict[str, list[UsageRow]]) -> list[UsageRow]:
    agrupadas: dict[tuple, UsageRow] = {}
    for r in (r for linhas in por_turno.values() for r in linhas):
        key = (r.ts.date(), r.model, r.codex_long_context)
        antes = agrupadas.get(key)
        agrupadas[key] = r if antes is None else replace(
            antes, input=antes.input + r.input, output=antes.output + r.output,
            cache_read=antes.cache_read + r.cache_read,
            cache_write=antes.cache_write + r.cache_write)
    return sorted(agrupadas.values(), key=lambda r: (r.ts, r.model))


class DobraCodex:
    """Custo e uso de um rollout numa passada só: cada linha é decodificada uma vez e vai aos
    dois leitores. Linhas de custo saem sem conta; ela entra na leitura, pela dona do escopo."""

    def __init__(self, arq: Path) -> None:
        self.respostas = RespostasCodex(arq.stem, None)
        self.uso = uso_codex.AcumuladorCodex()

    def linha(self, bruta: bytes) -> None:
        d = _dict_da_linha(bruta)
        if d is not None:
            self.respostas.registro(d)
            self.uso.registro(d)

    def fechar(self):
        por_turno = self.respostas.por_turno()
        return (_agrupar_rollout(por_turno), self.uso.linhas_sem_area_codex(),
                self.uso.entradas_de_area_codex(por_turno))


def _linhas_rollout_codex(arq: Path, account_id: str) -> list[UsageRow]:
    return _agrupar_rollout(respostas_por_turno_codex(arq, account_id))


def raiz_pi() -> Path:
    return pi_sessions.sessions_root("pi")


def raiz_omp() -> Path:
    return pi_sessions.sessions_root("omp")


def linhas_pi(raiz: Path | None = None, source: str = "pi") -> list[UsageRow]:
    """~/.pi/agent/sessions/**/*.jsonl — POR MENSAGEM, soma tudo. Mesmo leitor serve o omp
    (`raiz`/`source` recebidos): mesmo formato JSONL, só muda a raiz e o rótulo da linha.

    Glob RECURSIVA de propósito: o subagente do Pi mora em
    `<sessao>/<taskId>/run-N/session.jsonl` (é o que adapters/pi/sessions.py:41-47 já documenta,
    acima de `is_subagent_transcript`).
    Medido em 01/08/2026 num par pai/filho: na janela de 20:52:12–20:58:10 em que o filho
    registrou 19 eventos de uso, o pai registrou ZERO, e nenhum totalTokens coincide. O uso do
    subagente NÃO está no pai — somar os dois não duplica, e ignorar o filho perde 18 sessões.

    O `usage.cost` que o Pi já calcula é DESCARTADO: o custo é recalculado com a mesma tabela das
    outras fontes, senão as três não estão na mesma régua.
    """
    raiz = raiz if raiz is not None else raiz_pi()
    if not raiz.is_dir():
        return []
    _sincronizar_pi(raiz, source)
    return [_usage_row(t) for t in costs_cache.ler_custos(f"{source}:{raiz}")]


def _sincronizar_pi(raiz: Path, source: str) -> None:
    costs_cache.sincronizar(f"{source}:{raiz}", costs_cache.listar(raiz, lambda n: n.endswith(".jsonl")),
                            lambda arq: _dobra_pi(arq, raiz, source), f"pi:{CACHE_VERSAO}")


class DobraPi:
    def __init__(self, sid: str, source: str) -> None:
        self.sid, self.source = sid, source
        self.cwd = self.modelo = self.prov = ""
        self.ts = None
        self.acc = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
        self.viu = False

    def linha(self, bruta: bytes) -> None:
        d = _dict_da_linha(bruta)
        if d is None:
            return
        t = d.get("type")
        if t == "session":
            self.cwd = d.get("cwd") or self.cwd
            self.ts = _quando(d.get("timestamp")) or self.ts
        elif t == "model_change":
            # Pi: provider + modelId separados. omp: um campo só, "provider/id".
            if d.get("model") and "/" in str(d["model"]):
                self.prov, self.modelo = str(d["model"]).split("/", 1)
            else:
                self.prov = d.get("provider") or self.prov
                self.modelo = d.get("modelId") or self.modelo
        elif t == "message":
            msg = d.get("message")
            u = msg.get("usage") if isinstance(msg, dict) else None
            if isinstance(u, dict):
                self.viu = True
                for k in self.acc:
                    self.acc[k] += _int(u.get(k))

    def fechar(self):
        if not self.viu or self.ts is None:
            return [], [], None
        acc = self.acc
        return [UsageRow(
            ts=self.ts, source=self.source, provider=pricing.canonizar_provedor(self.prov) or "?",
            model=self.modelo or "?",
            project=self.cwd or PROJETO_DESCONHECIDO, session_id=self.sid,
            input=acc["input"], output=acc["output"],
            cache_write=acc["cacheWrite"], cache_read=acc["cacheRead"],
        )], [], None


def _dobra_pi(arq: Path, raiz: Path, source: str) -> DobraPi:
    # session_id pelo caminho RELATIVO, não pelo `arq.stem`: todo subagente se chama
    # `session.jsonl`, então o stem seria a string "session" para TODOS eles, de todas as
    # sessões — indistinguíveis. Hoje não corrompe soma (não há dedup entre linhas do Pi),
    # mas deixaria o campo inútil pra qualquer drill-down.
    return DobraPi(str(arq.relative_to(raiz).with_suffix("")), source)


def _ler_inteiro(arq: Path, dobra):
    try:
        f = arq.open("rb")
    except OSError:
        return dobra.fechar()
    with f:
        for bruta in f:
            dobra.linha(bruta)
    return dobra.fechar()


def _linhas_arquivo_pi(arq: Path, raiz: Path, source: str) -> list[UsageRow]:
    return _ler_inteiro(arq, _dobra_pi(arq, raiz, source))[0]


def linhas_omp() -> list[UsageRow]:
    return linhas_pi(raiz_omp(), "omp")


def raiz_kimi() -> Path:
    return kimi_sessions.kimi_home() / "sessions"


def _kimi_index() -> dict[str, str]:
    """sessionId -> workDir, do session_index.jsonl do Kimi (projeto da linha de uso)."""
    out: dict[str, str] = {}
    try:
        with open(kimi_sessions.kimi_home() / "session_index.jsonl", encoding="utf-8") as fh:
            for line in fh:
                try:
                    o = json.loads(line)
                except ValueError:
                    continue
                if isinstance(o, dict) and o.get("sessionId"):
                    out[o["sessionId"]] = o.get("workDir") or ""
    except OSError:
        pass
    return out


def linhas_kimi() -> list[UsageRow]:
    """~/.kimi-code/sessions/*/session_*/agents/*/wire.jsonl — eventos `usage.record`, SOMA tudo.

    Medido no 0.34.0: um usage.record por turno com o DELTA (inputOther/output/inputCacheRead/
    inputCacheCreation), nao cumulativo — a regra e a mesma do Pi (somar), nao a do Claude (ultima
    linha). Subagentes (agents/agent-N/wire.jsonl) somam junto, marcados subagente=True: o wire do
    agente principal NAO inclui o uso dos filhos (mesmo fato medido no Pi).
    """
    raiz = raiz_kimi()
    if not raiz.is_dir():
        return []
    _sincronizar_kimi(raiz)
    return _linhas_kimi_do_indice(raiz)


def _sincronizar_kimi(raiz: Path) -> None:
    costs_cache.sincronizar(f"kimi:{raiz}", costs_cache.listar(raiz, lambda n: n == "wire.jsonl"),
                            _dobra_kimi, f"kimi:{CACHE_VERSAO}")


def _linhas_kimi_do_indice(raiz: Path, desde: str | None = None) -> list[UsageRow]:
    # O projeto vem do session_index, não do wire — aplicado na LEITURA, senão uma linha
    # gravada antes de o índice conhecer a sessão ficaria "desconhecido" até o wire mudar.
    index = _kimi_index()
    return [replace(r, project=index.get(r.session_id) or PROJETO_DESCONHECIDO)
            for r in map(_usage_row, costs_cache.ler_custos(f"kimi:{raiz}", desde))]


class DobraKimi:
    def __init__(self, sid: str, subagente: bool) -> None:
        self.sid, self.subagente = sid, subagente
        self.acc = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
        self.modelo = ""
        self.ts = None
        self.viu = False

    def linha(self, bruta: bytes) -> None:
        if b"usage.record" not in bruta:
            return
        d = _dict_da_linha(bruta)
        if d is None or d.get("type") != "usage.record":
            return
        u = d.get("usage")
        if not isinstance(u, dict):
            return
        self.viu = True
        self.modelo = d.get("model") or self.modelo
        t = d.get("time")
        if isinstance(t, (int, float)):
            self.ts = datetime.fromtimestamp(t / 1000.0, LOCAL)
        acc = self.acc
        acc["input"] += _int(u.get("inputOther"))
        acc["output"] += _int(u.get("output"))
        acc["cacheRead"] += _int(u.get("inputCacheRead"))
        acc["cacheWrite"] += _int(u.get("inputCacheCreation"))

    def fechar(self):
        if not self.viu or self.ts is None:
            return [], [], None
        modelo, acc = self.modelo, self.acc
        # Modelo vem como ALIAS ("apikey/k3"); o provedor e o prefixo. Canoniza como os demais.
        prov = modelo.split("/", 1)[0] if "/" in modelo else ""
        return [UsageRow(
            ts=self.ts, source="kimi", provider=pricing.canonizar_provedor(prov) or prov or "?",
            model=modelo or "?",
            project=PROJETO_DESCONHECIDO, session_id=self.sid,
            input=acc["input"], output=acc["output"],
            cache_write=acc["cacheWrite"], cache_read=acc["cacheRead"],
            subagente=self.subagente,
        )], [], None


def _dobra_kimi(arq: Path) -> DobraKimi:
    # session_id = nome do sessionDir (session_<uuid>) — o stem seria "wire" pra TODOS
    # (mesmo caso do session.jsonl do Pi, ver linhas_pi).
    return DobraKimi(arq.parent.parent.parent.name, kimi_sessions.is_subagent_wire(str(arq)))


def _linhas_wire_kimi(arq: Path) -> list[UsageRow]:
    return _ler_inteiro(arq, _dobra_kimi(arq))[0]


# Suba ao mudar o UsageRow ou a regra de um leitor, senão o cache velho é servido pra sempre.
CACHE_VERSAO = 1
# O endpoint é `def` e roda no threadpool: celular + desktop + peer batendo juntos com cache frio
# fariam N parses simultâneos do mesmo arquivo. Precedente: engines.py:58.
# ponytail: a trava cobre o CORPO INTEIRO de coletar() (walk de Codex/Pi + parse das três
# fontes), não só o parse duplicado -- serializa qualquer coleta concorrente, mesmo com cache
# quente. Aceitável num app single-user de LAN; trava por fonte se um dia isso virar gargalo medido.
_cache_lock = threading.Lock()


class Aquecendo(Exception):
    """A coleta está ocupada com uma varredura longa (primeira leitura) e o chamador não quis
    esperar. Carrega (lidos, total) pra tela mostrar progresso em vez de 'não respondeu'."""

    def __init__(self, lidos: int, total: int):
        super().__init__(f"aquecendo {lidos}/{total}")
        self.lidos = lidos
        self.total = total


def invalidar_cache() -> None:
    with _cache_lock:
        costs_cache.invalidar()


# Primeira coleta desta subida do backend. Máquina nova paga a varredura inteira (1 GB+) uma
# vez; enquanto ela roda, o endpoint devolve "aquecendo" com progresso em vez de estourar o
# prazo do cliente. Depois disso, cada coleta só relê o que mudou.
_aquecido = threading.Event()
_aquecedor: threading.Thread | None = None
_agendado: threading.Timer | None = None
_aquecer_lock = threading.Lock()
_served_by_rust = False

# O pedido lê o índice como está, na hora. Passado `_FRESCOR_S` desde a última varredura, uma
# nova roda atrás; "Atualizar dados" pede `fresco=True` e varre antes de ler.
_FRESCOR_S = 30.0
_ultima_coleta: float | None = None
_refrescador: threading.Thread | None = None
# Escopos da última varredura: a leitura usa os mesmos, sem refazer o walk nem reler
# `.claude.json` a cada pedido.
_escopos: dict = {"claude": [], "codex": [], "pi": [], "kimi": None}


def _coletar_tudo() -> None:
    sincronizar_tudo()


def _aquecer() -> None:
    try:
        _coletar_tudo()
    except Exception:
        _log.warning("aquecimento dos custos falhou", exc_info=True)
    finally:
        # Mesmo falhando: senão a tela ficaria em "aquecendo" pra sempre. A próxima coleta
        # roda no pedido e o erro aparece lá.
        _aquecido.set()


def aquecer_em_background() -> None:
    """Dispara a primeira coleta numa thread, se ainda não rodou nem está rodando."""
    global _aquecedor
    with _aquecer_lock:
        if _aquecido.is_set() or (_aquecedor is not None and _aquecedor.is_alive()):
            return
        _aquecedor = threading.Thread(target=_aquecer, name="custos-warm", daemon=True)
        _aquecedor.start()


def _refrescar_em_background() -> None:
    """Uma coleta nova atrás do pedido que já foi respondido com a leitura anterior."""
    global _refrescador
    with _aquecer_lock:
        if _refrescador is not None and _refrescador.is_alive():
            return
        _refrescador = threading.Thread(target=_refrescar, name="custos-refresh", daemon=True)
        _refrescador.start()


def _refrescar() -> None:
    try:
        _coletar_tudo()
    except Exception:
        _log.warning("atualização dos custos em segundo plano falhou", exc_info=True)


def _fresco(t: float | None) -> bool:
    return t is not None and time.monotonic() - t < _FRESCOR_S


def set_served_by_rust(on: bool) -> None:
    """Com o hangar-server de pé, o boot não varre: as telas falam com o índice dele."""
    global _served_by_rust
    _served_by_rust = on


def _boot_warmup() -> None:
    # Só o boot pula; um pedido repassado ao Python continua aquecendo por `_pronto`.
    if not _served_by_rust:
        aquecer_em_background()


def agendar_aquecimento(atraso_s: float) -> None:
    """Boot: espera o backend estabilizar (registry, app-server do Codex e hooks disputam o
    disco nos primeiros segundos) antes de varrer."""
    global _agendado
    cancelar_aquecimento()
    _agendado = threading.Timer(atraso_s, _boot_warmup)
    _agendado.daemon = True
    _agendado.name = "custos-warm-timer"
    _agendado.start()


def cancelar_aquecimento() -> None:
    """Shutdown: um Timer pendente varreria o `~/.claude` real depois de a suíte subir o app."""
    global _agendado
    if _agendado is not None:
        _agendado.cancel()
        _agendado = None


def _pronto(fresco: bool, esperar: float | None) -> None:
    """Antes da primeira varredura terminar, dispara o aquecimento e responde `Aquecendo` na
    hora — o pedido nunca paga a varredura fria. Depois, o pedido lê o índice e uma varredura
    velha é refeita atrás; `fresco` (botão Atualizar) varre antes de ler."""
    if not _aquecido.is_set():
        aquecer_em_background()
        raise Aquecendo(*costs_cache.progresso_total())
    if fresco or _ultima_coleta is None:
        # Sem varredura que tenha terminado (o aquecimento falhou): o erro aparece no pedido.
        sincronizar_tudo(esperar)
    elif not _fresco(_ultima_coleta):
        _refrescar_em_background()


def preparar(fresco: bool = False, esperar: float = 3.0) -> None:
    """O que o endpoint chama antes de ler o índice: levanta `Aquecendo` ou varre se pedido."""
    _pronto(fresco, esperar)


def coletar_ou_aquecendo(esperar: float = 3.0, *, fresco: bool = False,
                         desde: str | None = None) -> list[UsageRow]:
    _pronto(fresco, esperar)
    return _ler_custos(desde)


def _ler_uso(desde: str | None = None) -> tuple[Iterable[tuple], list[UsageRow]]:
    """Linhas de uso (tools/skills/contexto, tuplas de `costs_cache.iter_usage_rows`, lidas sob
    demanda) e de tokens do Claude, de todas as contas. As de tokens vêm junto porque o custo
    de um agente é o transcript filho dele, que só existe nelas."""
    escopos = [(costs_claude_transcript.escopo(raiz), account_id)
               for raiz, account_id in _escopos["claude"]]
    escopos += [(identidade, identidade) for identidade in _escopos["codex"]]
    tokens: list[UsageRow] = []
    for raiz, account_id in _escopos["claude"]:
        # `account_id` carimbado aqui (o custo usa `provider`, que em sessão de motor é o
        # provedor do modelo, não a conta): o filtro por conta precisa da conta.
        tokens.extend(_linhas_claude_do_indice(raiz, account_id, desde, carimbar_conta=True))
    return itertools.chain.from_iterable(
        costs_cache.iter_usage_rows(scope, conta, desde) for scope, conta in escopos), tokens


def _codex_do_indice(identidade: str, desde: str | None = None) -> list[UsageRow]:
    out = []
    for t in costs_cache.ler_custos(identidade, desde):
        r = _usage_row(t)
        out.append(replace(r, account_id=identidade,
                           provider=identidade if r.provider == "openai" else r.provider))
    return out


def _ler_custos(desde: str | None = None) -> list[UsageRow]:
    out: list[UsageRow] = []
    for raiz, account_id in _escopos["claude"]:
        out.extend(_linhas_claude_do_indice(raiz, account_id, desde))
    for identidade in _escopos["codex"]:
        out.extend(_codex_do_indice(identidade, desde))
    for raiz, source in _escopos["pi"]:
        out.extend(map(_usage_row, costs_cache.ler_custos(f"{source}:{raiz}", desde)))
    if _escopos["kimi"] is not None:
        out.extend(_linhas_kimi_do_indice(_escopos["kimi"], desde))
    return out


def account_info(config_dir: Path, fallback_label: str) -> tuple[str, str | None, str]:
    """(uuid, email, label) da conta Anthropic. Era costs._account_info e MUDOU DE MÓDULO:
    ler o config dir é trabalho de leitor de fonte, não de agregador — e deixá-la no costs.py
    criaria ciclo (costs importa costs_sources; _config_dirs precisaria de costs)."""
    for f in (config_dir / ".claude.json", Path.home() / ".claude.json"):
        try:
            # encoding EXPLÍCITO: sem ele o Python usa o do locale, que no Windows é cp1252 — e o
            # .claude.json tem caminho de projeto e histórico de prompt dentro, texto do usuário.
            # Ali cp1252 falha dos dois jeitos: byte sem mapa (0x81/0x8D/0x8F/0x90/0x9D) levanta
            # UnicodeDecodeError, que NÃO é json.JSONDecodeError e escapa deste except; e o resto
            # decodifica torto e calado (medido: "café 🚀" volta "cafÃ© ðŸš€").
            oa = (json.loads(f.read_text(encoding="utf-8")).get("oauthAccount") or {})
        except (OSError, UnicodeDecodeError, json.JSONDecodeError, AttributeError, TypeError):
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


def _contas_codex() -> list[codex_contas.Account]:
    try:
        return codex_contas.list_accounts()
    except OSError:
        _log.warning("custos: nao consegui listar contas Codex", exc_info=True)
        return [codex_contas.Account("default", codex_contas.default_home(), True)]


def _rollouts_codex_por_conta(accounts: list[codex_contas.Account]) \
        -> dict[str, tuple[codex_contas.Account, set[Path]]]:
    """Enumera primeiro e atribui pelo caminho canônico; links não trocam o dono do rollout."""
    result: dict[str, tuple[codex_contas.Account, set[Path]]] = {}
    for account in accounts:
        viva = raiz_codex(account.home)
        for raiz in (viva, viva.parent / "archived_sessions"):
            if not raiz.is_dir():
                continue
            for path in costs_cache.listar(raiz, lambda n: n.startswith("rollout-") and n.endswith(".jsonl")):
                try:
                    canonical = path.resolve(strict=True)
                    owner = codex_contas.account_for_rollout(canonical)
                except (OSError, codex_contas.AccountError):
                    continue
                if owner is None:
                    continue
                item = result.setdefault(owner.id, (owner, set()))
                item[1].add(canonical)
    return result


def coletar(esperar: float | None = None) -> list[UsageRow]:
    """Todas as linhas das quatro fontes, depois de uma varredura. NUNCA vai à rede."""
    sincronizar_tudo(esperar)
    return _ler_custos()


def sincronizar_tudo(esperar: float | None = None) -> None:
    """Uma varredura de todas as fontes: custo e uso saem da mesma leitura de cada arquivo.

    `esperar` é quanto tempo aceitar ficar na fila atrás de outra varredura: o endpoint passa
    uns segundos e recebe `Aquecendo` se a primeira (máquina nova) ainda estiver rodando; o
    aquecimento de boot passa None e espera o que precisar.
    """
    global _ultima_coleta
    if not _cache_lock.acquire(timeout=-1 if esperar is None else esperar):
        raise Aquecendo(*costs_cache.progresso_total())
    try:
        _sincronizar()
        _ultima_coleta = time.monotonic()
    finally:
        _cache_lock.release()


def _dobra_codex(arq: Path) -> DobraCodex:
    return DobraCodex(arq)


def _pi_roots() -> list[tuple[Path, str]]:
    out = []
    for source, root in (("pi", raiz_pi()), ("omp", raiz_omp())):
        if source == "omp" and root == raiz_pi():
            # Contar a raiz compartilhada como omp também dobraria o gasto.
            if not _AVISOU_RAIZ_UNICA:
                _AVISOU_RAIZ_UNICA.add(str(root))
                _log.warning("custos: omp e pi na mesma raiz (%s) — gasto do omp somado como pi", root)
            continue
        if root.is_dir():
            out.append((root, source))
    return out




def _sincronizar() -> None:
    global _escopos
    escopos: dict = {"claude": [], "codex": [], "pi": [], "kimi": None}
    costs_cache.zerar_progresso()
    for caminho, account_id in _config_dirs():
        raiz = costs_claude_transcript.raiz_projetos(Path(caminho))
        costs_claude_transcript.sincronizar(raiz)
        escopos["claude"].append((raiz, account_id))

    for owner, caminhos in _rollouts_codex_por_conta(_contas_codex()).values():
        home = owner.home.expanduser().absolute().resolve(strict=False)
        identidade = f"codex:{home}"
        _ROTULOS[identidade] = f"Codex · {owner.id}"
        costs_cache.sincronizar(identidade, sorted(caminhos), _dobra_codex,
                                f"codex:{CACHE_VERSAO}:{_USO_CODEX_VERSAO}")
        escopos["codex"].append(identidade)

    for raiz, nome in _pi_roots():
        _sincronizar_pi(raiz, nome)
        escopos["pi"].append((raiz, nome))
    kimi = raiz_kimi()
    if kimi.is_dir():
        _sincronizar_kimi(kimi)
        escopos["kimi"] = kimi
    costs_cache.esquecer_fora({
        *(costs_claude_transcript.escopo(raiz) for raiz, _a in escopos["claude"]),
        *escopos["codex"], *(f"{nome}:{raiz}" for raiz, nome in escopos["pi"]),
        *([f"kimi:{kimi}"] if escopos["kimi"] is not None else [])})
    if escopos != _escopos:
        costs_cache.mudou()
    _escopos = escopos


def custos_do_rollout(path: Path) -> list[UsageRow] | None:
    """Linhas de custo de UM rollout, pelo índice: só o que cresceu desde a última leitura é
    lido. A conta não entra (quem pede quer só o valor da sessão). `None` = o índice não pôde
    ler agora, o que não é o mesmo que "sem uso"."""
    file_id = costs_cache.sincronizar_arquivo(path, _dobra_codex,
                                              f"codex:{CACHE_VERSAO}:{_USO_CODEX_VERSAO}",
                                              "codex:avulso")
    return None if file_id is None else [_usage_row(t) for t in costs_cache.ler_custos(file_id=file_id)]
