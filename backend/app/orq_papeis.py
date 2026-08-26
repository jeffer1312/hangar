"""Tabela de papéis do contrato do grupo (`regras-<gid>.md`, seção `## Quem é quem`).

O árbitro é dono do resto do arquivo; o app troca só a linha do papel (orq_md). `sessão` aceita
`*` no fim (`trab-t*`: uma sessão por Task) — casa a viva mais recente.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

from . import orq_md, pair

_log = logging.getLogger(__name__)

CABECALHO = ("papel", "sessão", "provider", "conta", "modelo", "esforço")
SECAO = "Quem é quem"
ARBITRO = "arbitro"


@dataclass(frozen=True)
class Papel:
    papel: str
    sessao: str
    provider: str
    conta: str
    modelo: str
    esforco: str

    def e_arbitro(self) -> bool:
        return orq_md.normalizar(self.papel) == ARBITRO


GID_PADRAO = "padrao"   # regras-padrao.md: o time que o árbitro copia ao montar um grupo novo


def regras_path(gid: str) -> Path:
    return pair._pair_dir() / f"regras-{gid}.md"


def ler(texto: str) -> list[Papel]:
    return [Papel(r["papel"], r["sessão"], r["provider"].lower(), r["conta"], r["modelo"], r["esforço"])
            for r in orq_md.ler_tabela(texto, CABECALHO) if r.get("papel")]


def escrever_papel(texto: str, p: Papel) -> str:
    for v in (p.papel, p.sessao, p.provider, p.conta, p.modelo, p.esforco):
        orq_md.validar_celula(v)
    return orq_md.trocar_linha(texto, CABECALHO, p.papel, {
        "papel": p.papel, "sessão": p.sessao, "provider": p.provider,
        "conta": p.conta, "modelo": p.modelo, "esforço": p.esforco}, SECAO)


def _casa_nome(padrao: str, nome: str) -> bool:
    padrao = padrao.strip()
    if not padrao:
        return False
    return nome.startswith(padrao[:-1]) if padrao.endswith("*") else nome == padrao


def gid_por_sessao(nome: str) -> str | None:
    """Grupo de uma sessão SEM sidecar de pareamento: o contrato já diz quem está nele — a coluna
    `sessão` da tabela. Um grupo tocado fora do `--pair` (já aconteceu num trabalho real) continua visível.
    Mais de um contrato casando → o mais recente."""
    achados: list[tuple[float, str]] = []
    for p in pair._pair_dir().glob("regras-*.md"):
        if p.stem == f"regras-{GID_PADRAO}":
            continue
        try:
            texto, mtime = orq_md.ler_arquivo(p)
        except (OSError, ValueError) as e:
            # Um contrato ilegível (encoding quebrado por edição à mão) não pode tirar a tela de
            # TODAS as sessões — pula este e diz qual foi.
            _log.warning("regras ilegível, ignorado: %s (%s)", p, e)
            continue
        if any(_casa_nome(r.sessao, nome) for r in ler(texto)):
            achados.append((mtime, p.stem.removeprefix("regras-")))
    return max(achados)[1] if achados else None


def casar_viva(papel: Papel, sessoes) -> str | None:
    """Nome da sessão viva deste papel: exato, ou o glob `x*` → a mais recente por last_activity."""
    alvo = papel.sessao.strip()
    if not alvo:
        return None
    if alvo.endswith("*"):
        pref = alvo[:-1]
        cands = [s for s in sessoes if s.name.startswith(pref)]
        if not cands:
            return None
        return max(cands, key=lambda s: s.last_activity or 0).name
    return next((s.name for s in sessoes if s.name == alvo), None)
