"""Um registro de time por contexto; o grupo passa a apontar para o mesmo Markdown."""
from __future__ import annotations

import hashlib
import os
import re
from dataclasses import dataclass
from pathlib import Path

from app import atomico, orq_identity, orq_md, pair
from app.names import sanitize_session_name
from app.orq_identity import IdentityUnavailable, identity

_GROUP = re.compile(r"^<!-- hangar-orq-group: ([a-zA-Z0-9_-]+) -->$", re.M)
_PREFIX = re.compile(r"^<!-- hangar-orq-prefix: ([a-zA-Z0-9_-]+) -->$", re.M)


class PromotionConflict(ValueError):
    pass


@dataclass(frozen=True)
class Context:
    gid: str
    grouped: bool
    session_prefix: str
    path: Path


def draft_gid(name: str) -> str:
    return "draft-" + hashlib.sha256(identity(name).encode("utf-8")).hexdigest()[:24]


def active_gid(name: str) -> str | None:
    """O nome e a identidade registrada precisam pertencer à execução viva."""
    from app.adapters.orq import runs
    from app.orq_start import _orq

    try:
        dirs = sorted(runs.root().iterdir())
    except FileNotFoundError:
        return None
    try:
        current_identity = identity(name)
    except IdentityUnavailable:
        return None
    found: set[str] = set()
    for d in dirs:
        run = runs._auto_run(d, strict=True)
        if not run or run[2]:
            continue
        state = _orq().state(d)
        names = {state["arbiter"]}
        names.update(n for t in state["open"] for n in state["roles"].get(t, {}).values() if n)
        identities = {run[0].get("arbiter"): run[0].get("arbiter_identity")}
        for event in _orq().events(d):
            if event.get("tipo") == "task_inicio":
                captured = event.get("session_identities")
                for member in (event.get("executor"), event.get("par")):
                    identities[member] = captured.get(member) if isinstance(captured, dict) else None
            elif event.get("tipo") == "sessao_trocada":
                identities.pop(event.get("de"), None)
                identities[event.get("para")] = event.get("session_identity")
        if name in names and identities.get(name) == current_identity:
            found.add(run[1])
    if len(found) > 1:
        raise ValueError("a sessão está registrada em mais de uma execução viva")
    return next(iter(found), None)


def real_group(gid: str, *, orchestration: bool = False) -> bool:
    from app.adapters.orq import runs

    linked = any((link := pair.PairLink(p.stem).get()) and link["gid"] == gid
                 and (not orchestration or link["orq"])
                 for p in pair._pair_dir().glob("*.json"))
    return linked or runs.group_phase(gid) == "live"


def group_ended(gid: str) -> bool:
    from app.adapters.orq import runs

    phase = runs.group_phase(gid)
    if phase in ("live", "unknown") or real_group(gid):
        return False
    return phase == "ended" or any(pair._arquivo_dir().glob(f"regras-{gid}-*.md"))


def resolve(name: str) -> Context:
    from app.orq_papeis import regras_path

    with orq_md._TRAVA:
        link = pair.PairLink(name).get()
        gid = (link or {}).get("gid") or active_gid(name)
        prefix = sanitize_session_name(name) or "hangar"
        if gid:
            path = regras_path(gid)
            text, _ = orq_md.ler_arquivo(path)
            saved = _PREFIX.search(text)
            return Context(gid, True, saved[1] if saved else prefix, path)
        gid = draft_gid(name)
        path = regras_path(gid)
        text, _ = orq_md.ler_arquivo(path)
        target = _GROUP.search(text)
        if target:
            target_path = regras_path(target[1])
            text, _ = orq_md.ler_arquivo(target_path)
            # Um apontamento antigo não ressuscita contratos já arquivados.
            if text and real_group(target[1]):
                saved = _PREFIX.search(text)
                return Context(target[1], True, saved[1] if saved else prefix, target_path)
            if group_ended(target[1]):
                return Context(gid, False, prefix, path)
            raise PromotionConflict("não foi possível confirmar o encerramento do grupo deste trabalho")
        saved = _PREFIX.search(text)
        return Context(gid, False, saved[1] if saved else prefix, path)


def write(context: Context, text: str, mtime: float) -> float:
    with orq_md._TRAVA:
        current, _ = orq_md.ler_arquivo(context.path)
        target = _GROUP.search(current)
        if target:
            if context.grouped or not group_ended(target[1]):
                raise orq_md.Conflito(str(context.path))
            text = _GROUP.sub("", text).lstrip("\n")
        if not context.grouped and not _PREFIX.search(text):
            text = f"<!-- hangar-orq-prefix: {context.session_prefix} -->\n\n" + text
        return orq_md.gravar(context.path, text, mtime)


def promote(name: str, gid: str, mtime: float | None = None) -> None:
    """Move o registro escolhido e deixa só um apontamento no contexto de origem."""
    from app.orq_papeis import regras_path

    if not re.fullmatch(r"[a-zA-Z0-9_-]+", gid) or gid.startswith("draft-") or gid == "padrao":
        raise PromotionConflict("grupo de destino inválido")
    with orq_md._TRAVA:
        source = regras_path(draft_gid(name))
        text, current_mtime = orq_md.ler_arquivo(source)
        if mtime is not None and abs(current_mtime - mtime) > 1e-6:
            raise orq_md.Conflito(str(source))
        target = _GROUP.search(text)
        if target:
            if group_ended(target[1]):
                return
            if target[1] != gid:
                raise PromotionConflict("o time já pertence a outro grupo")
            return
        if not text:
            return
        dest = regras_path(gid)
        existing, _ = orq_md.ler_arquivo(dest)
        if existing and existing != text:
            raise PromotionConflict("o grupo já possui outro contrato; associe o time antes de escrevê-lo")
        moved = not existing
        if moved:
            dest.parent.mkdir(parents=True, exist_ok=True)
            atomico.substituir(source, dest)
        try:
            orq_md.gravar(source, f"<!-- hangar-orq-group: {gid} -->\n", 0.0 if moved else current_mtime)
        except (OSError, orq_md.Conflito) as e:
            restoration = "não necessária"
            if moved:
                try:
                    # Criação exclusiva: nunca substitui uma edição externa na origem.
                    os.link(dest, source)
                    restoration = "origem restaurada sem substituir outro arquivo"
                except OSError as restore_error:
                    restoration = f"origem não restaurada: {restore_error}"
            raise PromotionConflict(f"a promoção não foi concluída ({e}); {restoration}. "
                                    f"Confira {source} e {dest} antes de associar novamente") from e


def associate(name: str, gid: str, mtime: float) -> Context:
    with pair._LOCK:
        return associate_unlocked(name, gid, mtime)


def associate_unlocked(name: str, gid: str, mtime: float) -> Context:
    """Sem o `pair._LOCK`: no modo Rust quem segura o lock de grupo é o Rust, que chama por
    `/internal/orq/associate`."""
    if not real_group(gid, orchestration=True):
        raise ValueError("grupo de orquestração de destino não existe")
    context = resolve(name)
    if context.grouped:
        if context.gid != gid:
            raise ValueError("o time já pertence a outro grupo")
        if abs(orq_md.ler_arquivo(context.path)[1] - mtime) > 1e-6:
            raise orq_md.Conflito(str(context.path))
        return context
    source_text, _ = orq_md.ler_arquivo(context.path)
    if not source_text or _GROUP.search(source_text):
        raise ValueError("configure o time deste trabalho antes de associá-lo")
    promote(name, gid, mtime)
    return resolve(name)
