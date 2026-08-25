"""Leitura e agregação dos eventos de orquestração (eventos.jsonl por execução).

Contrato do arquivo: skills/orchestrating-idea-to-push/references/arbitro.md. Robustez no
molde do planprog: o arquivo é escrito por agente — linha inválida, campo com tipo errado ou
tipo desconhecido são ignorados (com log quando o diretório inteiro fica sem linha válida),
nunca derrubam a listagem.
"""
from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from pathlib import Path

_log = logging.getLogger(__name__)

_TIPOS = {"execucao_inicio", "task_inicio", "entrega", "veredito",
          "sessao_trocada", "execucao_fim"}


@dataclass(frozen=True)
class TaskResumo:
    task: int
    titulo: str
    executor: str
    par: str
    rodadas: int          # 0 = desconhecido (rodada omitida nos eventos), nunca "de primeira"
    resultado: str | None
    inicio: str | None
    fim: str | None
    eventos: tuple[dict, ...]


@dataclass(frozen=True)
class ExecucaoResumo:
    id: str
    plano: str
    branch: str
    gid: str
    inicio: str | None
    fim: str | None
    resultado: str | None
    tasks: tuple[TaskResumo, ...]
    eventos_execucao: tuple[dict, ...]   # sessao_trocada e afins — eventos sem task
    voltas: int
    aprovadas_primeira: int
    reconstruida: bool


def raiz_padrao() -> Path:
    return Path.home() / ".claude" / "orq-retros"


def _int_ou_none(v) -> int | None:
    # bool é subclasse de int; True virar rodada 1 seria numero inventado
    if isinstance(v, bool) or not isinstance(v, int):
        return None
    return v


def _le_eventos(path: Path) -> list[dict]:
    out: list[dict] = []
    try:
        texto = path.read_text(encoding="utf-8")
    except OSError:
        return out
    for linha in texto.splitlines():
        linha = linha.strip()
        if not linha:
            continue
        try:
            ev = json.loads(linha)
        except ValueError:
            continue
        if isinstance(ev, dict) and ev.get("tipo") in _TIPOS:
            out.append(ev)
    return out


def _monta(exec_id: str, eventos: list[dict]) -> ExecucaoResumo:
    inicio = next((e for e in eventos if e["tipo"] == "execucao_inicio"), {})
    fim = next((e for e in eventos if e["tipo"] == "execucao_fim"), None)
    por_task: dict[int, list[dict]] = {}
    soltos: list[dict] = []
    for e in eventos:
        t = _int_ou_none(e.get("task"))
        if t is not None:
            por_task.setdefault(t, []).append(e)
        elif e["tipo"] not in ("execucao_inicio", "execucao_fim"):
            soltos.append(e)
    tasks: list[TaskResumo] = []
    voltas = 0
    aprovadas_primeira = 0
    for n in sorted(por_task):
        evs = por_task[n]
        ini = next((e for e in evs if e["tipo"] == "task_inicio"), {})
        vereditos = [e for e in evs if e["tipo"] == "veredito"]
        final = vereditos[-1] if vereditos else None
        rodadas = max((r for e in evs if e["tipo"] in ("entrega", "veredito")
                       if (r := _int_ou_none(e.get("rodada"))) is not None), default=0)
        voltas += sum(1 for e in vereditos if e.get("resultado") in ("devolvido", "reprova"))
        if final and final.get("resultado") == "aprova" and rodadas == 1:
            aprovadas_primeira += 1
        tasks.append(TaskResumo(
            task=n,
            titulo=str(ini.get("titulo", "")),
            executor=str(ini.get("executor", "")),
            par=str(ini.get("par", "")),
            rodadas=rodadas,
            resultado=final.get("resultado") if final else None,
            inicio=ini.get("ts"),
            fim=final.get("ts") if final else None,
            eventos=tuple(evs),
        ))
    return ExecucaoResumo(
        id=exec_id,
        plano=str(inicio.get("plano", "")),
        branch=str(inicio.get("branch", "")),
        gid=str(inicio.get("gid", "")),
        inicio=inicio.get("ts"),
        fim=fim.get("ts") if fim else None,
        resultado=fim.get("resultado") if fim else None,
        tasks=tuple(tasks),
        eventos_execucao=tuple(soltos),
        voltas=voltas,
        aprovadas_primeira=aprovadas_primeira,
        reconstruida=bool(inicio.get("reconstruido")),
    )


def listar_execucoes(raiz: Path) -> list[ExecucaoResumo]:
    out: list[ExecucaoResumo] = []
    try:
        dirs = [d for d in raiz.iterdir() if d.is_dir()]
    except OSError:
        return out
    for d in dirs:
        arq = d / "eventos.jsonl"
        eventos = _le_eventos(arq)
        if eventos:
            out.append(_monta(d.name, eventos))
        elif arq.exists():
            _log.warning("orq: %s tem eventos.jsonl sem linha valida — fora da lista", d.name)
    # nome <data>-<gid> ordena cronologicamente; `inicio` com offset misto nao (ISO string)
    out.sort(key=lambda e: e.id, reverse=True)
    return out


def detalhe(raiz: Path, exec_id: str) -> ExecucaoResumo | None:
    if "/" in exec_id or "\\" in exec_id or exec_id in (".", ".."):
        return None  # exec_id vem de URL; nunca vira caminho fora da raiz
    eventos = _le_eventos(raiz / exec_id / "eventos.jsonl")
    return _monta(exec_id, eventos) if eventos else None


def fichas(execucoes: list[ExecucaoResumo]) -> list[dict]:
    acc: dict[str, dict] = {}
    for e in execucoes:
        for t in e.tasks:
            if not t.par:
                continue
            f = acc.setdefault(t.par, {"par": t.par, "aceitas": 0, "nao_aceitas": 0,
                                       "aprovadas_primeira": 0, "_rod": 0, "_rod_n": 0})
            if t.resultado == "aprova":
                f["aceitas"] += 1
                if t.rodadas == 1:
                    f["aprovadas_primeira"] += 1
            else:
                f["nao_aceitas"] += 1
            if t.rodadas >= 1:
                f["_rod"] += t.rodadas
                f["_rod_n"] += 1
    out = []
    for f in sorted(acc.values(), key=lambda x: -(x["aceitas"] + x["nao_aceitas"])):
        rod, n = f.pop("_rod"), f.pop("_rod_n")
        f["rodadas_media"] = round(rod / n, 1) if n else 0.0
        out.append(f)
    return out
