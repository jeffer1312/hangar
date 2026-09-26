"""Agregação do relatório de uso (skills, tools, Bash, MCP, agentes, contexto, imagens, plugins).

Quem lê é o `uso_claude` (dentro da passada do `costs_claude_transcript`); quem sabe preço é
o `pricing` via `costs._custo_da_linha`. Aqui só se soma e se corta por período e filtros.
"""
from __future__ import annotations

import logging
import os
import time
from collections import defaultdict
from dataclasses import replace
from datetime import datetime, timedelta
from pathlib import Path

from app import costs, pricing
from app.costs_sources import (LOCAL, PROJETO_DESCONHECIDO, UsageRow, coletar_uso,
                               rotulo_de_provedor)
from app.models import Applied, UsoBucket, UsoReport
from app.uso_claude import UsoLinha, plugin_de, skill_do_caminho

_REPO = Path(__file__).resolve().parents[2]
_log = logging.getLogger("hangar.uso")

# Grupo de skill sem prefixo de plugin. `@` não existe em nome de plugin: a tela traduz.
ORIGEM_REPO = "@repo"
ORIGEM_PESSOAL = "@pessoal"
ORIGEM_AVULSA = "@avulsa"
ORIGEM_EMBUTIDA = "@embutida"


def _origem_da_pasta(pasta: Path, home: Path) -> str:
    real = Path(os.path.realpath(pasta))
    if real.is_relative_to(_REPO / "skills"):
        return ORIGEM_REPO
    plugin = plugin_de((skill_do_caminho(str(real / "SKILL.md")) or ("", True))[0])
    if plugin:
        return plugin
    if real.is_relative_to(Path(os.path.realpath(home / ".agents" / "skills"))):
        return ORIGEM_AVULSA
    return ORIGEM_PESSOAL


def origens_de_skill(home: Path | None = None) -> dict[str, str]:
    """nome da skill -> grupo. Mesma precedência de descoberta do Claude: pasta do usuário, repo,
    avulsas, depois o cache de plugins (onde o Codex acha `brainstorming` sem prefixo).
    Nome que não está em pasta nenhuma é embutido no CLI ou já foi removido."""
    home = home or Path.home()
    rasas = [home / ".claude" / "skills", _REPO / "skills", home / ".agents" / "skills"]
    fundas = [home / ".claude" / "plugins" / "cache", home / ".codex" / "plugins" / "cache",
              home / ".claude" / "plugins" / "marketplaces"]
    achadas: dict[str, str] = {}
    for raiz in rasas + fundas:
        # Pasta ilegível perde só as skills dela; o relatório inteiro não cai por isso.
        try:
            if raiz in rasas:
                pastas = [s for s in sorted(raiz.iterdir()) if (s / "SKILL.md").is_file()] if raiz.is_dir() else []
            else:
                pastas = [md.parent for md in sorted(raiz.rglob("SKILL.md")) if "skills" in md.parts] if raiz.is_dir() else []
            for pasta in pastas:
                achadas.setdefault(pasta.name, _origem_da_pasta(pasta, home))
        except OSError as e:
            _log.warning("uso: origem das skills em %s não lida: %s", raiz, e)
    return achadas

# chars/4 é a régua de "tokens estimados": a tela SEMPRE rotula como estimativa.
_CHARS_POR_TOKEN = 4
_MARCA_SUBAGENTE = "/subagents/agent-"
# `bash`/`mcp` repetem a chamada de `tool`: nos totais cada chamada conta uma vez só.
_TIPOS_CONTADOS = ("tool", "skill", "agente", "contexto", "imagem")


# Texto de skill: medido contra o cache write real das respostas (regressão em 196 cargas,
# correlação 0,996). chars/4 subestimava 60%. Os demais contextos seguem chars/4 (não medidos).
_CHARS_POR_TOKEN_SKILL = 2.5


def _zero() -> dict:
    return {"sessions": set(), "subs": set(), "chamadas": 0, "pedidas": 0, "ctx_chars": 0, "tokens_est": 0,
            "input": 0, "output": 0, "cache_write": 0, "cache_read": 0, "cost": 0.0,
            "cost_input": 0.0, "cost_output": 0.0, "cost_cache_write": 0.0,
            "cost_cache_read": 0.0, "plugin": "",
            "ocupados": 0, "ocupados_eq": 0, "respostas": 0, "regua": _CHARS_POR_TOKEN}


def _sessao(b: dict, l: UsoLinha) -> None:
    (b["subs"] if l.subagente else b["sessions"]).add(l.session_id)


def _custos_reais(l: UsoLinha) -> dict[str, float]:
    if not (l.input or l.output or l.cache_write or l.cache_read):
        return {}
    c = costs._custo_da_linha(UsageRow(
        ts=datetime.fromisoformat(l.dia).replace(tzinfo=LOCAL), source=l.fonte,
        provider="openai" if l.fonte == "codex" else "anthropic", model=l.model, project=l.cwd,
        session_id=l.session_id,
        input=l.input, output=l.output, cache_write=l.cache_write, cache_read=l.cache_read,
        cache_write_1h=l.cache_write_1h, fast=l.fast))
    return c or {}


def _custo_real(l: UsoLinha) -> float:
    return sum(_custos_reais(l).values())


def _custo_dos_agentes(tokens: list[UsageRow]) -> dict[str, dict]:
    """agentId -> tokens e custo do transcript filho (`…/subagents/agent-<id>`)."""
    out: dict[str, dict] = defaultdict(lambda: {"input": 0, "output": 0, "cache_write": 0,
                                                "cache_read": 0, "cost": 0.0,
                                                "cost_input": 0.0, "cost_output": 0.0,
                                                "cost_cache_write": 0.0, "cost_cache_read": 0.0})
    for r in tokens:
        if _MARCA_SUBAGENTE not in r.session_id:
            continue
        agent_id = r.session_id.rsplit("agent-", 1)[1]
        a = out[agent_id]
        a["input"] += r.input
        a["output"] += r.output
        a["cache_write"] += r.cache_write
        a["cache_read"] += r.cache_read
        c = costs._custo_da_linha(r)
        if c:
            a["cost"] += sum(c.values())
            for k, v in c.items():
                a[f"cost_{k}"] += v
    return out


def _custo_linha(l: UsoLinha, agentes: dict[str, dict]) -> float:
    if l.tipo == "skill":
        return _custo_real(l)
    if l.tipo == "agente" and l.detalhe:
        a = agentes.get(l.detalhe)
        return a["cost"] if a else 0.0
    return 0.0


def _custos_linha(l: UsoLinha, agentes: dict[str, dict]) -> dict[str, float]:
    if l.tipo == "skill":
        return {f"cost_{k}": v for k, v in _custos_reais(l).items()}
    if l.tipo == "agente" and l.detalhe:
        a = agentes.get(l.detalhe)
        return {k: a[k] for k in ("cost_input", "cost_output", "cost_cache_write", "cost_cache_read")} if a else {}
    return {}


def _bucket(key: str, v: dict) -> UsoBucket:
    return UsoBucket(key=key, plugin=v["plugin"], sessions=len(v["sessions"]), subagentes=len(v["subs"]),
                     chamadas=v["chamadas"], pedidas=v["pedidas"], ctx_chars=v["ctx_chars"],
                     ctx_tokens_est=int(v["ctx_chars"] / v["regua"]) + v["tokens_est"],
                     input=v["input"], output=v["output"], cache_write=v["cache_write"],
                     cache_read=v["cache_read"], cost=v["cost"],
                     cost_input=v["cost_input"], cost_output=v["cost_output"],
                     cost_cache_write=v["cost_cache_write"], cost_cache_read=v["cost_cache_read"],
                     ocupados_tokens_est=int(v["ocupados"] / _CHARS_POR_TOKEN_SKILL),
                     ocupados_eq_tokens_est=int(v["ocupados_eq"] / _CHARS_POR_TOKEN_SKILL),
                     respostas=v["respostas"])


def _ordenar(agg: dict[str, dict]) -> list[UsoBucket]:
    return sorted((_bucket(k, v) for k, v in agg.items()),
                  key=lambda b: (-b.ocupados_eq_tokens_est, -b.ocupados_tokens_est, -b.cost, -b.ctx_chars, -b.chamadas, b.key))


def _conta_no_total(l: UsoLinha) -> bool:
    """`bash`/`mcp` repetem a chamada de `tool`; `tool:Skill` e `tool:Agent` repetem a linha
    de `skill`/`agente`. Cada chamada entra uma vez."""
    if l.tipo == "tool":
        return l.nome not in ("Skill", "Agent")
    return l.tipo in _TIPOS_CONTADOS


_CAMPOS_TOKENS = ("input", "output", "cache_write", "cache_read")


def _somar_em(b: dict, l: UsoLinha, agentes: dict[str, dict], do_item: bool = False) -> None:
    """Tokens reais do conjunto vêm das linhas de ÁREA, que somadas dão todo o uso do Claude
    sem repetição; `do_item` (série de uma skill/agente) soma os tokens do próprio item."""
    _sessao(b, l)
    if l.tipo == "area":
        if not do_item:
            for k in _CAMPOS_TOKENS:
                b[k] += getattr(l, k)
        return
    if _conta_no_total(l):
        # Contexto injetado (instruções, lembretes, hooks) é ocorrência, não chamada.
        if l.tipo != "contexto":
            b["chamadas"] += l.chamadas
        b["ctx_chars"] += l.ctx_chars
        b["tokens_est"] += l.tokens_est
    b["cost"] += _custo_linha(l, agentes)
    for k, v in _custos_linha(l, agentes).items():
        b[k] += v
    if do_item:
        b["ocupados"] += l.ocupados
        b["ocupados_eq"] += l.ocupados_eq
        b["respostas"] += l.respostas
        fonte =(agentes.get(l.detalhe) if l.tipo == "agente" and l.detalhe
                 else {k: getattr(l, k) for k in _CAMPOS_TOKENS} if l.tipo == "skill" else None)
        for k in _CAMPOS_TOKENS if fonte else ():
            b[k] += fonte[k]


def _por_dimensao(uso: list[UsoLinha], agentes: dict[str, dict], chave,
                  rotulo=None) -> list[UsoBucket]:
    """Totais por conta/projeto/modelo: lista dos seletores. Vem do PERÍODO inteiro, antes dos
    filtros de dimensão, senão a opção escolhida sumiria do próprio seletor."""
    agg: dict[str, dict] = defaultdict(_zero)
    for l in uso:
        _somar_em(agg[chave(l)], l, agentes)
    out = []
    for k, v in agg.items():
        b = _bucket(k, v)
        if rotulo:
            b.label = rotulo(k)
        out.append(b)
    return sorted(out, key=lambda b: (-_tokens(b), -b.chamadas, b.key))


def _tokens(b: UsoBucket) -> int:
    return b.input + b.output + b.cache_write + b.cache_read


def _por_dia(uso: list[UsoLinha], agentes: dict[str, dict], do_item: bool = False) -> list[UsoBucket]:
    agg: dict[str, dict] = defaultdict(_zero)
    for l in uso:
        if l.dia:
            _somar_em(agg[l.dia], l, agentes, do_item)
    return sorted((_bucket(k, v) for k, v in agg.items()), key=lambda b: b.key)


def _somar_area(b: dict, l: UsoLinha) -> None:
    _sessao(b, l)
    b["chamadas"] += l.chamadas
    for k in ("input", "output", "cache_write", "cache_read"):
        b[k] += getattr(l, k)
    b["cost"] += _custo_real(l)


def _por_area_dia(uso: list[UsoLinha]) -> list[UsoBucket]:
    """key = `YYYY-MM-DD|área` (chave única pra mescla da malha), label = área."""
    agg: dict[str, dict] = defaultdict(_zero)
    for l in uso:
        if l.tipo == "area" and l.dia:
            _somar_area(agg[f"{l.dia}|{l.nome}"], l)
    out = sorted((_bucket(k, v) for k, v in agg.items()), key=lambda b: b.key)
    for b in out:
        b.label = b.key.split("|", 1)[1]
    return out


Filtro = str | list[str] | None


def _lista(v: Filtro) -> list[str]:
    """Um filtro aceita um valor ou vários; vazio = todos."""
    if not v:
        return []
    return [v] if isinstance(v, str) else [x for x in v if x]


def montar(uso: list[UsoLinha], tokens: list[UsageRow], period: str = "all",
           now: datetime | None = None, conta: Filtro = None,
           projeto: Filtro = None, modelo: Filtro = None,
           plugin: Filtro = None, foco: str | None = None,
           origens: dict[str, str] | None = None) -> UsoReport:
    contas, projetos, modelos, plugins_f = _lista(conta), _lista(projeto), _lista(modelo), _lista(plugin)
    if origens is not None:
        uso = [replace(l, plugin=origens.get(l.nome, ORIGEM_EMBUTIDA))
               if l.tipo == "skill" and not l.plugin else l for l in uso]
    uso = [replace(l, plugin=plugin_de(l.nome)) if l.tipo == "agente" and not l.plugin else l
           for l in uso]

    def filtrar(linhas: list[UsoLinha]) -> list[UsoLinha]:
        return [l for l in linhas
                if (not contas or l.conta in contas)
                and (not projetos or (l.cwd or PROJETO_DESCONHECIDO) in projetos)
                and (not modelos or (pricing.canonizar(l.model) or "?") in modelos)
                and (not plugins_f or l.plugin in plugins_f)]

    now = now or datetime.now(LOCAL)
    dias = costs.PERIODOS.get(period)
    if dias:
        corte = (now - timedelta(days=dias - 1)).date()
        uso =[l for l in uso if l.dia and datetime.fromisoformat(l.dia).date() >= corte]
        tokens = [r for r in tokens if r.ts.date() >= corte]
    agentes = _custo_dos_agentes(tokens)
    por_conta = _por_dimensao(uso, agentes, lambda l: l.conta, rotulo_de_provedor)
    por_projeto = _por_dimensao(uso, agentes, lambda l: l.cwd or PROJETO_DESCONHECIDO)
    por_modelo = _por_dimensao(uso, agentes, lambda l: pricing.canonizar(l.model) or "?")
    uso = filtrar(uso)
    # O custo do agente vem do transcript filho, que tem conta e projeto próprios: o filtro
    # vale pra ele também (o filho de outra conta não entra na soma desta).
    if contas or projetos:
        tokens = [r for r in tokens
                  if (not contas or r.account_id in contas)
                  and (not projetos or (r.project or PROJETO_DESCONHECIDO) in projetos)]
        agentes = _custo_dos_agentes(tokens)

    por_tipo: dict[str, dict[str, dict]] = defaultdict(lambda: defaultdict(_zero))
    plugins: dict[str, dict] = defaultdict(_zero)
    total = _zero()
    for l in uso:
        b = por_tipo[l.tipo][l.nome]
        b["plugin"] = b["plugin"] or l.plugin
        _sessao(b, l)
        b["chamadas"] += l.chamadas
        b["ctx_chars"] += l.ctx_chars
        b["tokens_est"] += l.tokens_est
        if l.origem in ("voce", "pedido"):
            b["pedidas"] += l.chamadas
        if l.tipo == "skill":
            b["regua"] = _CHARS_POR_TOKEN_SKILL
            b["ocupados"] += l.ocupados
            b["ocupados_eq"] += l.ocupados_eq
            b["respostas"] += l.respostas
        if l.tipo in ("skill", "area"):
            b["input"] += l.input
            b["output"] += l.output
            b["cache_write"] += l.cache_write
            b["cache_read"] += l.cache_read
            b["cost"] += _custo_real(l)
            if l.tipo == "skill":
                for k, v in _custos_reais(l).items():
                    b[f"cost_{k}"] += v
        elif l.tipo == "agente" and l.detalhe:
            a = agentes.get(l.detalhe)
            if a:
                for k in ("input", "output", "cache_write", "cache_read", "cost",
                          "cost_input", "cost_output", "cost_cache_write", "cost_cache_read"):
                    b[k] += a[k]
        if l.plugin and l.tipo in ("skill", "contexto", "agente"):
            p = plugins[l.plugin]
            p["plugin"] = l.plugin
            _sessao(p, l)
            p["chamadas"] += l.chamadas
            p["ctx_chars"] += l.ctx_chars
            p["ocupados"] += l.ocupados
            p["ocupados_eq"] += l.ocupados_eq
            p["respostas"] += l.respostas
            if l.tipo == "skill":
                p["input"] += l.input
                p["output"] += l.output
                p["cache_write"] += l.cache_write
                p["cache_read"] += l.cache_read
                p["cost"] += _custo_real(l)
                for k, v in _custos_reais(l).items():
                    p[f"cost_{k}"] += v
            elif l.tipo == "agente" and l.detalhe:
                a = agentes.get(l.detalhe)
                if a:
                    for k in ("input", "output", "cache_write", "cache_read", "cost",
                              "cost_input", "cost_output", "cost_cache_write", "cost_cache_read"):
                        p[k] += a[k]
        _somar_em(total, l, agentes)

    # Série diária: sob todos os filtros e, com `foco`, só do item de nome igual (qualquer
    # tipo) — é o clique numa linha da tabela.
    serie = [l for l in uso if l.nome == foco] if foco else uso
    if foco and por_tipo["area"].get(foco):
        por_dia: dict[str, dict] = defaultdict(_zero)
        for l in serie:
            if l.tipo == "area" and l.dia:
                _somar_area(por_dia[l.dia], l)
        by_day = sorted((_bucket(k, v) for k, v in por_dia.items()), key=lambda b: b.key)
    else:
        by_day = _por_dia(serie, agentes, do_item=bool(foco))

    return UsoReport(
        totals=_bucket("totals", total),
        by_skill=_ordenar(por_tipo["skill"]),
        by_tool=_ordenar(por_tipo["tool"]),
        by_bash=_ordenar(por_tipo["bash"]),
        by_mcp=_ordenar(por_tipo["mcp"]),
        by_agente=_ordenar(por_tipo["agente"]),
        by_contexto=_ordenar(por_tipo["contexto"]),
        by_imagem=_ordenar(por_tipo["imagem"]),
        by_plugin=_ordenar(plugins),
        by_conta=por_conta,
        by_projeto=por_projeto,
        by_modelo=por_modelo,
        by_area=_ordenar(por_tipo["area"]),
        by_area_dia=_por_area_dia(uso),
        by_day=by_day,
        applied=Applied(period=period),
        conta=contas, projeto=projetos, modelo=modelos, plugin=plugins_f, foco=foco or None,
        usd_brl=costs.usd_brl(),
    )


def report(period: str = "all", now: datetime | None = None, fresco: bool = False,
           **filtros) -> UsoReport:
    """Levanta `costs_sources.Aquecendo` enquanto a primeira coleta da subida não terminou.
    `filtros`: conta, projeto, modelo, plugin, foco (ver `montar`)."""
    uso, tokens = coletar_uso(fresco=fresco)
    return montar(uso, tokens, period=period, now=now, origens=_origens_recentes(), **filtros)


_ORIGENS_TTL_S = 300
_origens_cache: tuple[float, dict[str, str]] = (float("-inf"), {})


def _origens_recentes() -> dict[str, str]:
    # A varredura desce no cache de plugins inteiro; filtro e detalhe pedem o relatório de novo.
    global _origens_cache
    if time.monotonic() - _origens_cache[0] > _ORIGENS_TTL_S:
        _origens_cache = (time.monotonic(), origens_de_skill())
    return _origens_cache[1]
